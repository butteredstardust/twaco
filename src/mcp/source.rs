use super::entity::{refusal_code, tool_targets};
use super::requests::source::{
    CheckRequest, DeployRequest, ExtractRequest, FmtRequest, SyncRequest, TypesAction, TypesRequest,
};
use super::*;

pub(crate) fn types_tool(
    solution: &Solution,
    request: TypesRequest,
    progress: &dyn Progress,
) -> Result<Value, ToolError> {
    types_tool_with_compiler(solution, request, None, progress)
}

pub(crate) fn types_tool_with_compiler(
    solution: &Solution,
    arguments: TypesRequest,
    compiler: Option<&dyn types::CompilerRunner>,
    progress: &dyn Progress,
) -> Result<Value, ToolError> {
    let action = match arguments.action {
        TypesAction::Generate => commands::types::TypesAction::Generate,
        TypesAction::Check => commands::types::TypesAction::Check,
        TypesAction::Platform => commands::types::TypesAction::Platform,
    };
    let request = commands::types::TypesRequest {
        action,
        profile: arguments.profile.clone(),
        lock_label: "mcp types",
    };
    let mut notices = commands::Notices::default();
    let result = commands::types::execute(
        solution,
        &request,
        server::Client::new,
        compiler,
        &mut notices,
        progress,
    );
    match result.map_err(ToolError::coded)? {
        commands::types::TypesOutcome::Generated(outcome) => {
            let mut result = json!({
                "ok": true,
                "entities": outcome.entities,
                "data_shapes": outcome.data_shapes,
                "services": outcome.services,
                "files_written": outcome.files_written,
                "skipped": outcome.skipped,
            });
            if !outcome.gitignore_covers_types {
                result["gitignore_note"] = json!("add `.twaco/types/`, `**/services/*/jsconfig.json`, and `**/services/*/twaco-globals.d.ts` to the solution root's .gitignore");
            }
            add_notices(&mut result, &notices);
            Ok(result)
        }
        commands::types::TypesOutcome::Checked(outcome) => {
            let mut by_code = std::collections::BTreeMap::new();
            for finding in &outcome.findings {
                *by_code
                    .entry(format!("TS{}", finding.code))
                    .or_insert(0usize) += 1;
            }
            let findings = outcome.findings.iter().map(|finding| {
                json!({
                    "file": finding.file,
                    "line": finding.line,
                    "column": finding.column,
                    "code": format!("TS{}", finding.code),
                    "message": finding.message,
                })
            });
            let mut result = json!({
                "ok": outcome.findings.is_empty(),
                "findings": outcome.findings.len(),
                "services_with_findings": outcome.affected_services,
                "services": outcome.services,
                "seconds": outcome.elapsed.as_secs_f64(),
                "by_code": by_code,
            });
            if arguments.detail {
                result["findings_list"] = Value::Array(findings.collect());
            } else {
                result["first"] = Value::Array(findings.take(20).collect());
            }
            add_notices(&mut result, &notices);
            Ok(result)
        }
        commands::types::TypesOutcome::Platform(outcome) => {
            let mut result = json!({
                "ok": true,
                "templates": outcome.templates,
                "shapes": outcome.shapes,
                "resources": outcome.resources,
                "skipped": outcome.skipped.into_iter().chain(outcome.types.skipped).collect::<Vec<_>>(),
            });
            add_notices(&mut result, &notices);
            Ok(result)
        }
    }
}

/// Every gate, and the live script parse when `live`: what `check` reports and `deploy` obeys.
fn run_gates(
    solution: &Solution,
    profile: &str,
    live: bool,
    progress: &dyn Progress,
) -> check::CheckReport {
    let mut report = check::run(solution);
    if live {
        let built = client(solution, profile).map_err(|error| error.message);
        let checker = built
            .as_ref()
            .map(|c| c as &dyn check::ScriptChecker)
            .map_err(Clone::clone);
        report
            .gates
            .push(check::live_parse_with_progress(solution, checker, progress));
    }
    report
}

pub(crate) fn check_tool(
    solution: &Solution,
    request: CheckRequest,
    progress: &dyn Progress,
) -> Result<Value, ToolError> {
    let report = run_gates(
        solution,
        &request.profile,
        request.live.unwrap_or(solution.gates.live),
        progress,
    );
    let detail = request.detail;
    let gates: Vec<Value> = report
        .gates
        .iter()
        .map(|gate| {
            let mut entry = json!({ "gate": gate.name, "examined": gate.examined, "findings": gate.findings.len() });
            if let Some(why) = &gate.broken {
                entry["broken"] = json!(why);
            }
            if !gate.gates_the_run {
                entry["advisory"] = json!(true);
            }
            entry
        })
        .collect();
    let mut result = json!({
        "ok": !report.blocks(),
        "gates": gates,
        "findings": report.findings(),
        "broken": report.broken(),
    });
    if detail {
        let findings: Vec<Value> = report
            .gates
            .iter()
            .flat_map(|gate| &gate.findings)
            .map(|f| json!({ "gate": f.gate, "file": f.file, "line": f.line, "rule": f.rule, "message": f.message }))
            .collect();
        result["failures"] = json!(findings);
    }
    Ok(result)
}

pub(crate) fn sync_tool(solution: &Solution, arguments: SyncRequest) -> Result<Value, ToolError> {
    let check = arguments.check;
    let target = tool_targets(&arguments.entity, &arguments.entities);
    let request = commands::sync::SyncRequest {
        target: commands::sync::SyncTarget {
            project: arguments.project.as_ref().cloned(),
            entities: target,
            all: arguments.all,
            reject_entities_with_all: true,
            missing_target: "name an entity (entity or entities), or pass all: true",
        },
        mode: if check { Mode::Plan } else { Mode::Apply },
        allow_structural: arguments.allow_add_remove,
        relayout: arguments.relayout,
        lock_label: "mcp sync",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::sync::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?
        .report;
    let mut result = json!({
        "ok": outcome.failed == 0,
        "check": check,
        "checked": outcome.checked,
        "changed": outcome.changed,
        "failed": outcome.failed,
        "changes": outcome.log.changes().collect::<Vec<_>>(),
        "errors": outcome.log.errors().collect::<Vec<_>>(),
    });
    add_types_refresh(&mut result, &outcome.types);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn extract_tool(
    solution: &Solution,
    arguments: ExtractRequest,
) -> Result<Value, ToolError> {
    let target = tool_targets(&arguments.entity, &arguments.entities);
    let request = commands::extract::ExtractRequest {
        target: commands::extract::ExtractTarget {
            project: arguments.project.as_ref().cloned(),
            entities: target,
            all: arguments.all,
            reject_entities_with_all: true,
            missing_target: "name an entity (entity or entities), or pass all: true",
        },
        lock_label: "mcp extract",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::extract::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?
        .report;
    let mut result = json!({
        "ok": outcome.failed == 0,
        "parts": outcome.written,
        "entities": outcome.entities,
        "failed": outcome.failed,
        "written": outcome.log.changes().collect::<Vec<_>>(),
        "errors": outcome.log.errors().collect::<Vec<_>>(),
    });
    add_types_refresh(&mut result, &outcome.types);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn add_types_refresh(result: &mut Value, refresh: &types::Refresh) {
    if let Some(files) = refresh.files_written {
        result["types_refreshed"] = json!(files);
    }
    if let Some(warning) = &refresh.warning {
        result["types_warning"] = json!(warning);
    }
}

pub(crate) fn fmt_tool(solution: &Solution, arguments: FmtRequest) -> Result<Value, ToolError> {
    let check = arguments.check;
    let request = commands::fmt::FmtRequest {
        mode: if check { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp fmt",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::fmt::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?
        .report;
    let mut result = json!({
        "ok": outcome.failed == 0 && (!check || outcome.changed.is_empty()),
        "check": check,
        "scripts": outcome.files,
        "changed": outcome.changed.iter().map(|p| relative(solution, p)).collect::<Vec<_>>(),
        "failed": outcome.failed,
        "errors": outcome.log.errors().collect::<Vec<_>>(),
    });
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn deploy_tool(
    solution: &Solution,
    arguments: DeployRequest,
    progress: &dyn Progress,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let force = arguments.force;
    let detail = arguments.detail;
    let only = arguments.only.items().to_vec();
    let only_projects = arguments.only_projects.items().to_vec();
    let request = commands::deploy::DeployRequest {
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        force,
        backup: arguments.backup,
        skip_checks: arguments.skip_checks,
        only_projects,
        only,
        backend_only: arguments.backend_only,
        profile: arguments.profile,
        lock_label: "mcp deploy",
    };
    let mut notices = commands::Notices::default();
    let (result, notes, saved) = match commands::deploy::execute(
        solution,
        &request,
        server::Client::new,
        &mut notices,
        progress,
    ) {
        Ok(commands::deploy::DeployOutcome::GatesBlocked { report, .. }) => {
            let failing: Vec<Value> = report.gates.iter().filter(|gate| gate.blocks()).map(
                |gate| json!({ "gate": gate.name, "findings": gate.findings.len(), "broken": gate.broken }),
            ).collect();
            let mut value = json!({
                "ok": false,
                "stage": "offline gates",
                "blocking_gates": failing,
                "note": "nothing was sent; fix these, or pass skip_checks: true",
            });
            add_notices(&mut value, &notices);
            return Ok(value);
        }
        Ok(commands::deploy::DeployOutcome::Complete {
            report,
            notes,
            backup,
            ..
        }) => (Ok(report), notes, backup),
        Err(commands::deploy::DeployCommandError::Deploy { why, backup, .. }) => {
            (Err(*why), Vec::new(), backup)
        }
        Err(error) => return Err(ToolError::coded(error)),
    };
    let plans = |report: &deploy::Report| -> Value {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for plan in &report.plans {
            let key = match &plan.decision {
                push::Decision::AlreadyThere => "unchanged",
                push::Decision::Create => "create",
                push::Decision::Update => "update",
                push::Decision::Refuse(_) => "refuse",
            };
            *counts.entry(key).or_default() += 1;
        }
        json!(counts)
    };
    let calls = |report: &deploy::Report| -> Vec<Value> {
        report
            .calls
            .iter()
            .map(|c| json!({ "project": c.project, "call": c.call.to_string(), "post_import": c.post_import, "skipped": c.skipped }))
            .collect()
    };
    let mut value = match result {
        Ok(report) => {
            let mut value = json!({
                "ok": true,
                "dry_run": dry_run,
                "projects": notes,
                "scripts_parsed": report.scripts_checked,
                "entities": plans(&report),
                "calls": calls(&report),
            });
            if let Some(dir) = &saved {
                value["backup"] = json!(dir);
            }
            if !dry_run {
                value["imported"] = json!(report.imported);
                value["kept"] = json!(report.kept.len());
                value["changed_by_deploy_step"] = json!(report
                    .changed_by_deploy
                    .iter()
                    .map(|(c, n)| format!("{c}/{n}"))
                    .collect::<Vec<_>>());
            }
            if detail {
                value["plans"] = json!(report
                    .plans
                    .iter()
                    .map(|p| json!({ "entity": format!("{}/{}", p.collection, p.name), "decision": format!("{:?}", p.decision) }))
                    .collect::<Vec<_>>());
            }
            value
        }
        Err(deploy::DeployError::ParseFailed(failures)) => json!({
            "ok": false,
            "stage": "server script parse",
            "failures": failures
                .iter()
                .map(|f| json!({ "entity": f.entity, "service": f.service, "line": f.line, "column": f.column, "message": f.message }))
                .collect::<Vec<_>>(),
            "note": "nothing was imported",
        }),
        Err(deploy::DeployError::Conflicts(conflicts)) => json!({
            "ok": false,
            "stage": "conflict check",
            // A short reason per entity; the full sentence, with hashes, is detail.
            "refused": conflicts
                .iter()
                .map(|c| {
                    let (code, full) = match &c.decision {
                        push::Decision::Refuse(reason) => (refusal_code(reason), reason.to_string()),
                        other => ("unexpected", format!("{other:?}")),
                    };
                    let mut entry = json!({ "entity": format!("{}/{}", c.collection, c.name), "reason": code });
                    if detail {
                        entry["explanation"] = json!(full);
                    }
                    entry
                })
                .collect::<Vec<_>>(),
            "note": "nothing was imported. server-changed: the server moved since the last sync. \
                     no-baseline: the sides differ and nothing records which changed (see status). \
                     deleted-on-server: it was removed there. force: true overwrites all of these.",
        }),
        Err(deploy::DeployError::NotKept(report)) => json!({
            "ok": false,
            "stage": "read-back",
            "imported": report.imported,
            "not_kept": report
                .not_kept
                .iter()
                .map(|n| json!({ "entity": format!("{}/{}", n.collection, n.name), "sent": n.sent, "read_back": n.read_back, "error": n.error, "only_permissions": n.only_permissions }))
                .collect::<Vec<_>>(),
        }),
        Err(error) => return Err(ToolError::coded(error)),
    };
    add_notices(&mut value, &notices);
    Ok(value)
}
