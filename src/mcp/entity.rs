use super::requests::common::Absent;
use super::requests::entity::{
    EntityCarryRequest, EntityDeleteRequest, EntityRestoreRequest, PermissionsApplyRequest,
    PermissionsAuditRequest, PermissionsInitRequest, PermissionsPushRequest, PermissionsRequest,
    PushRequest, StatusRequest,
};
use super::*;

pub(crate) fn status_tool(
    solution: &Solution,
    request: StatusRequest,
    progress: &dyn Progress,
) -> Result<Value, ToolError> {
    let record = request.record;
    let target = match (request.entity.as_deref(), request.all) {
        (Some(_), true) => {
            return Err(ToolError::invalid(
                "name an entity or pass all: true, not both",
            ))
        }
        (Some(name), false) => commands::status::StatusTarget::Names(vec![name.to_string()]),
        (None, true) => commands::status::StatusTarget::All,
        (None, false) => return Err(ToolError::invalid("name an entity, or pass all: true")),
    };
    let command = commands::status::StatusRequest {
        target,
        project: request.project.as_ref().cloned(),
        record,
        profile: request.profile.clone(),
        lock_label: "mcp status --record",
        refuse_unreadable: record,
        refuse_record_failures: record,
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::status::execute(
        solution,
        &command,
        server::Client::new,
        &mut notices,
        progress,
    ) {
        Ok(outcome) => outcome,
        Err(commands::status::StatusCommandError::Unreadable(items)) if record => {
            return Err(ToolError::with(
                ErrorCode::InvalidData,
                format!(
                    "nothing was recorded: {} entity file(s) could not be read: {}",
                    items.len(),
                    items.join("; ")
                ),
            ));
        }
        Err(error) => return Err(ToolError::coded(error)),
    };
    let counts: Map<String, Value> = status::counts(&outcome.statuses)
        .into_iter()
        .map(|(v, n)| (v.label().to_string(), json!(n)))
        .collect();
    let detail = request.detail;
    let listed: Vec<Value> = outcome.statuses
        .iter()
        .filter(|s| detail || s.verdict.is_drift())
        .map(|s| {
            let mut entry = json!({ "entity": format!("{}/{}", s.collection, s.name), "verdict": s.verdict.label() });
            if detail {
                entry["working"] = json!(s.working);
                entry["server"] = json!(s.server);
                entry["baseline_local"] = json!(s.local_baseline);
                entry["baseline_server"] = json!(s.server_baseline);
            }
            entry
        })
        .collect();
    let mut result = json!({
        "ok": outcome.failures.is_empty() && outcome.statuses.iter().all(|s| !s.verdict.is_drift()),
        "recorded": outcome.recorded,
        "entities": outcome.statuses.len(),
        "counts": counts,
        "failures": outcome.failures,
        "unreadable": outcome.unreadable,
    });
    // Summary: only what needs attention. Detail: every entity, with its hashes.
    result[if detail { "statuses" } else { "attention" }] = json!(listed);
    add_notices(&mut result, &notices);
    Ok(result)
}

/// The optional entity spelling that an executor will validate after taking a write lock.
pub(crate) fn tool_target(entity: &Absent<String>) -> Vec<String> {
    entity.as_ref().cloned().into_iter().collect()
}

/// The entities named by `entity` and `entities` together, in that order.
pub(crate) fn tool_targets(entity: &Absent<String>, entities: &Absent<Vec<String>>) -> Vec<String> {
    let mut targets = tool_target(entity);
    targets.extend(entities.items().iter().cloned());
    targets
}

pub(crate) fn refusal_code(refusal: &push::Refusal) -> &'static str {
    match refusal {
        push::Refusal::Conflict { .. } => "server-changed",
        push::Refusal::UnknownAncestor { .. } => "no-baseline",
        push::Refusal::DeletedOnServer => "deleted-on-server",
    }
}

pub(crate) fn push_tool(solution: &Solution, request: PushRequest) -> Result<Value, ToolError> {
    let dry_run = request.dry_run;
    let force = request.force;
    let request = commands::push::PushRequest {
        entity: request.entity,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        force,
        backup: request.backup,
        profile: request.profile,
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::push::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(|error| ToolError {
            code: error.code(),
            message: match error.backup() {
                Some(dir) => {
                    format!("{error}; the server's copy was saved to {dir} before the push")
                }
                None => error.to_string(),
            },
        })?;
    let label = match &outcome {
        commands::push::PushOutcome::Plan { entity, .. }
        | commands::push::PushOutcome::Applied { entity, .. } => entity.to_string(),
    };
    let saved = match &outcome {
        commands::push::PushOutcome::Applied { backup, .. } => backup.clone(),
        commands::push::PushOutcome::Plan { .. } => None,
    };
    let mut result = push_outcome_json(&label, dry_run, force, outcome);
    if let Some(dir) = saved {
        result["backup"] = json!(dir);
    }
    add_notices(&mut result, &notices);
    Ok(result)
}

/// What taking the workspace lock did (files swept, interrupted operations recovered), when it
/// did anything.
pub(crate) fn push_outcome_json(
    label: &str,
    dry_run: bool,
    force: bool,
    outcome: commands::push::PushOutcome,
) -> Value {
    match outcome {
        commands::push::PushOutcome::Plan { decision, .. } => {
            let (would, refusal) = match &decision {
                push::Decision::AlreadyThere => {
                    ("nothing: the server already has this version", None)
                }
                push::Decision::Create => ("create it on the server", None),
                push::Decision::Update => (
                    "update it; the server is unchanged since the last sync",
                    None,
                ),
                push::Decision::Refuse(refusal) => (
                    "refuse",
                    Some((refusal.to_string(), refusal.code().as_str())),
                ),
            };
            match refusal {
                Some((refusal, code)) => {
                    json!({ "entity": label, "dry_run": dry_run, "would": would, "refusal": refusal, "code": code, "force": force })
                }
                None => {
                    json!({ "entity": label, "dry_run": dry_run, "would": would, "refusal": null, "force": force })
                }
            }
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::AlreadyThere,
            ..
        } => {
            json!({ "entity": label, "dry_run": dry_run, "pushed": false, "note": "the server already has this version; baseline recorded" })
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::Pushed { created },
            ..
        } => {
            json!({ "entity": label, "dry_run": dry_run, "pushed": true, "created": created, "note": "read back and matching; baseline recorded" })
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::Refused(refusal),
            ..
        } => {
            json!({ "entity": label, "dry_run": dry_run, "pushed": false, "refusal": refusal.to_string(), "code": refusal.code().as_str(), "note": "nothing was sent; force: true pushes anyway" })
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::WouldDo(_),
            ..
        } => unreachable!("an applied outcome cannot be a plan"),
    }
}

pub(crate) fn entity_delete_tool(
    solution: &Solution,
    request: EntityDeleteRequest,
) -> Result<Value, ToolError> {
    let dry_run = request.dry_run;
    let (acknowledged, force_used) = entity_delete::acknowledged(
        request.force,
        request.allow_repository_defined,
        request.allow_outside_dependents,
        request.allow_file_repository_data_loss,
    );
    let request = commands::delete::EntityDeleteRequest {
        entities: request.entities.items().to_vec(),
        renamed: request.renamed,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        acknowledgements: acknowledged,
        legacy_force_used: force_used,
        backup: request.backup,
        profile: request.profile,
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::delete::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(|error| ToolError {
            code: error.code(),
            message: error.to_string(),
        })?;
    let (report, date, force_used) = match outcome {
        commands::delete::EntityDeleteOutcome::Plan {
            report,
            legacy_force_used,
            ..
        } => (report, None, legacy_force_used),
        commands::delete::EntityDeleteOutcome::Applied {
            report,
            date,
            legacy_force_used,
            ..
        } => (report, Some(date), legacy_force_used),
    };
    let mut result = json!({
        "ok": !report.failed(),
        "entities": report.entities,
        "dependency_limit": report.dependency_limit,
    });
    if let Some(dir) = &report.backup {
        result["backup"] = json!(dir);
    }
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    if report.ledger_changed {
        result["ledger_marked"] = json!(date.expect("an applied delete outcome has a date"));
    }
    if force_used {
        result["deprecated"] = json!(entity_delete::FORCE_DEPRECATION);
    }
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn entity_restore_tool(
    solution: &Solution,
    request: EntityRestoreRequest,
) -> Result<Value, ToolError> {
    let dry_run = request.dry_run;
    let request = commands::restore::RestoreRequest {
        set: request
            .set
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(str::to_string),
        only: request.entities.items().to_vec(),
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: request.profile,
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::restore::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?;
    let mut result = match outcome {
        commands::restore::RestoreOutcome::Sets { sets, .. } => {
            json!({ "ok": true, "sets": sets.iter().map(|set| json!({
                "id": set.id, "created": set.manifest.created, "reason": set.manifest.reason,
                "entities": set.manifest.entities.iter().map(|item| format!("{}/{}", item.collection, item.name)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>() })
        }
        commands::restore::RestoreOutcome::Plan { set, entities, .. }
        | commands::restore::RestoreOutcome::Applied { set, entities, .. } => {
            let failed = entities
                .iter()
                .any(|entry| entry.status == backup::Status::Failed);
            let mut result = json!({ "ok": !failed, "set": set.id, "entities": entities });
            result[if dry_run { "plan" } else { "applied" }] = json!(true);
            result
        }
    };
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn entity_carry_tool(
    solution: &Solution,
    request: EntityCarryRequest,
) -> Result<Value, ToolError> {
    let dry_run = request.dry_run;
    let pairs = entity_carry::pairs_from_names(request.pairs.items()).map_err(ToolError::coded)?;
    let request = commands::carry::CarryRequest {
        pairs,
        renamed: request.renamed,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        detail: request.detail,
        profile: request.profile,
        lock_label: "mcp entity_carry",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::carry::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?;
    let (report, date) = match outcome {
        commands::carry::CarryOutcome::Plan { report, .. } => (report, None),
        commands::carry::CarryOutcome::Applied { report, date, .. } => (report, Some(date)),
    };
    let failed = !dry_run
        && report
            .entities
            .iter()
            .any(|entity| entity.status == entity_carry::Status::Failed);
    let mut result = json!({ "ok": !failed, "entities": report.entities });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    if report.ledger_changed {
        result["ledger_marked"] = json!(date.expect("an applied carry has a date"));
    }
    add_notices(&mut result, &notices);
    Ok(result)
}

fn permissions_target(
    entities: &Absent<Vec<String>>,
    all: bool,
) -> Result<commands::status::StatusTarget, ToolError> {
    match (entities.items(), all) {
        ([], true) => Ok(commands::status::StatusTarget::All),
        ([], false) => Err(ToolError::invalid("name entities, or pass all: true")),
        (_, true) => Err(ToolError::invalid(
            "name entities or pass all: true, not both",
        )),
        (names, false) => Ok(commands::status::StatusTarget::Names(names.to_vec())),
    }
}

fn permissions_result(
    solution: &Solution,
    request: commands::permissions::PermissionsRequest,
    diff: bool,
) -> Result<Value, ToolError> {
    let mut notices = commands::Notices::default();
    let outcome =
        commands::permissions::execute(solution, &request, server::Client::new, &mut notices)
            .map_err(ToolError::coded)?;
    let report = &outcome.report;
    use crate::core::permissions::Status;
    // As the command line: drift in a diff and a target missing from the server in an applied
    // push are not success.
    let push = matches!(request.mode, Mode::Apply);
    let mut result = json!({
        "ok": report.count(Status::Failed) == 0
            && !(push && report.count(Status::NotOnServer) > 0)
            && !(diff && report.count(Status::Differs) > 0),
        "applied": report.applied,
        "differs": report.count(crate::core::permissions::Status::Differs),
        "entities": report.entities,
    });
    if let Some(recorded) = outcome.recorded {
        result["recorded"] = json!(recorded);
    }
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn permissions_tool(
    solution: &Solution,
    request: PermissionsRequest,
) -> Result<Value, ToolError> {
    let target = permissions_target(&request.entities, request.all)?;
    permissions_result(
        solution,
        commands::permissions::PermissionsRequest {
            target,
            project: request.project.as_ref().cloned(),
            mode: Mode::Plan,
            profile: request.profile,
            lock_label: "mcp permissions",
        },
        true,
    )
}

pub(crate) fn permissions_push_tool(
    solution: &Solution,
    request: PermissionsPushRequest,
) -> Result<Value, ToolError> {
    if request.platform {
        if request.all || !request.entities.items().is_empty() {
            return Err(ToolError::invalid(
                "platform pushes the policy's platform entries; push entities separately",
            ));
        }
        use crate::core::permissions::platform::State;
        let report = commands::permissions::execute_platform(
            solution,
            &commands::permissions::PlatformRequest {
                project: request.project.as_ref().cloned(),
                mode: if request.dry_run {
                    Mode::Plan
                } else {
                    Mode::Apply
                },
                profile: request.profile,
            },
            server::Client::new,
        )
        .map_err(ToolError::coded)?;
        return Ok(json!({
            "ok": report.count(State::Failed) == 0,
            "applied": report.applied,
            "missing": report.count(State::Missing),
            "items": report.items,
        }));
    }
    let target = permissions_target(&request.entities, request.all)?;
    permissions_result(
        solution,
        commands::permissions::PermissionsRequest {
            target,
            project: request.project.as_ref().cloned(),
            mode: if request.dry_run {
                Mode::Plan
            } else {
                Mode::Apply
            },
            profile: request.profile,
            lock_label: "mcp permissions_push",
        },
        false,
    )
}

pub(crate) fn permissions_audit_tool(
    solution: &Solution,
    request: PermissionsAuditRequest,
) -> Result<Value, ToolError> {
    use crate::core::permissions::audit::Severity;
    let report = commands::permissions::execute_audit(
        solution,
        &commands::permissions::AuditRequest {
            project: request.project.as_ref().cloned(),
            server: request.server.then(|| request.profile.clone()),
        },
        server::Client::new,
    )
    .map_err(ToolError::coded)?;
    let mut projects = serde_json::to_value(&report.projects).expect("audit report serialises");
    if !request.detail {
        for project in projects.as_array_mut().into_iter().flatten() {
            for finding in project["findings"].as_array_mut().into_iter().flatten() {
                if let Some(object) = finding.as_object_mut() {
                    object.remove("details");
                }
            }
        }
    }
    Ok(json!({
        "ok": report.count(Severity::Error) == 0,
        "server": report.server,
        "errors": report.count(Severity::Error),
        "warnings": report.count(Severity::Warning),
        "projects": projects,
        "without_policy": report.without_policy,
    }))
}

pub(crate) fn permissions_apply_tool(
    solution: &Solution,
    request: PermissionsApplyRequest,
) -> Result<Value, ToolError> {
    let mut notices = commands::Notices::default();
    let outcome = commands::permissions::execute_apply(
        solution,
        &commands::permissions::ApplyRequest {
            project: request.project.as_ref().cloned(),
            mode: if request.dry_run {
                Mode::Plan
            } else {
                Mode::Apply
            },
            lock_label: "mcp permissions_apply",
        },
        &mut notices,
    )
    .map_err(ToolError::coded)?;
    let mut projects = serde_json::to_value(&outcome.plan.projects).expect("apply plan serialises");
    if !request.detail {
        for project in projects.as_array_mut().into_iter().flatten() {
            for list in ["changes", "remaining"] {
                for item in project[list].as_array_mut().into_iter().flatten() {
                    if let Some(object) = item.as_object_mut() {
                        object.remove("details");
                    }
                }
            }
        }
    }
    let mut result = json!({
        "ok": outcome.plan.remaining_errors() == 0,
        "remaining_errors": outcome.plan.remaining_errors(),
        "files": outcome.plan.changes().count(),
        "projects": projects,
        "without_policy": outcome.plan.without_policy,
    });
    result[if outcome.applied { "applied" } else { "plan" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn permissions_init_tool(
    solution: &Solution,
    request: PermissionsInitRequest,
) -> Result<Value, ToolError> {
    let mut notices = commands::Notices::default();
    let outcome = commands::permissions::execute_init(
        solution,
        &commands::permissions::InitRequest {
            project: request.project.as_ref().cloned(),
            from_helper: request.from_helper,
            mode: if request.dry_run {
                Mode::Plan
            } else {
                Mode::Apply
            },
            lock_label: "mcp permissions_init",
        },
        &mut notices,
    )
    .map_err(ToolError::coded)?;
    let drafts: Vec<Value> = outcome
        .drafts
        .iter()
        .map(|draft| {
            json!({
                "project": draft.project,
                "path": draft.path.display().to_string(),
                "source": draft.source,
                "text": draft.text,
                "notes": draft.notes,
            })
        })
        .collect();
    let mut result = json!({ "ok": true, "drafts": drafts });
    result[if outcome.written { "applied" } else { "plan" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}
