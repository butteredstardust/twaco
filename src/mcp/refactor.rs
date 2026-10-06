use super::requests::refactor::{
    AdoptApplyRequest, AdoptReportRequest, MoveMemberRequest, NewBuildingBlockRequest,
    RenameRequest, RetemplateRequest,
};
use super::source::add_types_refresh;
use super::*;

pub(crate) fn move_member_tool(
    solution: &Solution,
    arguments: MoveMemberRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let action = arguments.action.as_str();
    if !matches!(action, "move" | "copy") {
        return Err(ToolError::invalid(format!(
            "unknown action `{action}`; use `move` or `copy`"
        )));
    }
    let kind = arguments.kind.as_str();
    let member = relocate::Member::from_word(kind).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown kind `{kind}`; use `service` or `property`"
        ))
    })?;
    let request = relocate::Request {
        member,
        copy: action == "copy",
        from: nonempty(&arguments.from, "from")?.to_string(),
        to: nonempty(&arguments.to, "to")?.to_string(),
        name: nonempty(&arguments.name, "name")?.to_string(),
        new_name: arguments.new_name.as_ref().cloned(),
        leave_delegate: arguments.leave_delegate,
    };
    let request = commands::relocate::RelocateRequest {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp move_member".to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::relocate::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let plan = outcome.plan();
    let problems = outcome.problems();
    let mut result = json!({
        "ok": problems.is_empty(), "action": action, "member": kind, "from": plan.request.from, "to": plan.request.to,
        "name": plan.request.name, "as": plan.final_name, "files": plan.files(solution),
        "callers": plan.callers, "notes": plan.notes, "out_of_step": problems,
    });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn new_building_block_tool(
    solution: &Solution,
    arguments: NewBuildingBlockRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let kind_word = arguments.r#type.as_str();
    let kind = newblock::BlockType::from_word(kind_word).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown type `{kind_word}`; use standard, abstract or implementation"
        ))
    })?;
    let request = newblock::Request {
        name: nonempty(&arguments.name, "name")?.to_string(),
        kind,
        display_name: arguments.display_name.as_ref().cloned(),
        description: arguments.description.as_ref().cloned().unwrap_or_default(),
        parent: arguments.parent.as_ref().cloned(),
        model_logic: arguments.model_logic,
        management_shape: arguments.management_shape,
        root: arguments.root.as_ref().cloned(),
        base_extension: arguments.base_extension.as_ref().cloned(),
    };
    let result_name = request.name.clone();
    let result_kind = request.kind;
    let request = commands::newblock::NewBlockRequest {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp new_building_block",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::newblock::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let plan = outcome.plan();
    let files: Vec<String> = plan
        .files
        .iter()
        .map(|file| {
            file.path
                .strip_prefix(&solution.root)
                .unwrap_or(&file.path)
                .display()
                .to_string()
                .replace('\\', "/")
        })
        .collect();
    let mut result = json!({
        "ok": true, "name": result_name, "type": result_kind.word(), "root": plan.root,
        "files": files, "twaco_toml": plan.config_addition, "notes": plan.notes,
    });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn retemplate_tool(
    solution: &Solution,
    arguments: RetemplateRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = retemplate::Request {
        entity: nonempty(&arguments.entity, "entity")?.to_string(),
        template: arguments.template.as_ref().cloned(),
        add_shapes: arguments.add_shapes.items().to_vec(),
        remove_shapes: arguments.remove_shapes.items().to_vec(),
        accept_loss: arguments.accept_loss,
    };
    let request = commands::retemplate::RetemplateRequest {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp retemplate",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::retemplate::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?;
    let plan = outcome.plan();
    let mut result = json!({
        "ok": true, "entity": plan.request.entity, "collection": plan.collection, "file": plan.file_relative(solution),
        "affected": plan.affected, "gained": plan.gained, "lost": plan.lost,
        "needs_accept_loss": plan.blocked, "notes": plan.notes,
    });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn adopt_apply_tool(
    solution: &Solution,
    arguments: AdoptApplyRequest,
) -> Result<Value, ToolError> {
    let export = nonempty(&arguments.export, "export")?;
    let export = {
        let path = PathBuf::from(export);
        if path.is_absolute() {
            path
        } else {
            solution.root.join(path)
        }
    };
    let only = arguments.entity.items().to_vec();
    let request = commands::adopt::AdoptRequest {
        export,
        only,
        mode: Mode::Apply,
        lock_label: "mcp adopt_apply",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::adopt::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let commands::adopt::AdoptOutcome::Applied {
        report, outcome, ..
    } = outcome
    else {
        unreachable!("an adopt apply request has an applied outcome")
    };
    let mut result = json!({
        "applied": outcome.lines,
        "reverts_not_applied": report.reverts().count(),
        "next": "run sync, then check",
    });
    add_types_refresh(&mut result, &outcome.types);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn adopt_tool(
    solution: &Solution,
    arguments: AdoptReportRequest,
) -> Result<Value, ToolError> {
    let export = nonempty(&arguments.export, "export")?;
    let export = {
        let path = PathBuf::from(export);
        if path.is_absolute() {
            path
        } else {
            solution.root.join(path)
        }
    };
    let only = arguments.entity.items().to_vec();
    let request = commands::adopt::AdoptRequest {
        export,
        only,
        mode: Mode::Plan,
        lock_label: "mcp adopt_report",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::adopt::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let commands::adopt::AdoptOutcome::Plan { report, .. } = outcome else {
        unreachable!("an adopt report request has a plan outcome")
    };
    let detail = arguments.detail;
    let services: Vec<Value> = report
        .services
        .iter()
        .map(|s| {
            json!({
                "entity": s.entity,
                "service": s.service,
                "generated": s.generated,
                "compared_with": relative(solution, &s.source),
            })
        })
        .collect();
    let changed: Vec<Value> = report
        .with_status(adopt::Status::Changed)
        .map(|e| {
            let mut entry = json!({
                "entity": e.entity.path(),
                "nodes": e.differences.len(),
                "regenerated_ids": e.volatile_ids,
                "ignored": e.ignored,
            });
            if detail {
                entry["differences"] = json!(e
                    .differences
                    .iter()
                    .map(|d| json!({ "path": d.path, "export": d.export, "repo": d.repo }))
                    .collect::<Vec<_>>());
            }
            entry
        })
        .collect();
    let mut result = json!({
        "reverts": report.reverts().count(),
        "services": services,
        "unmatched_services": report.unmatched_services,
        "new": report.with_status(adopt::Status::New).map(|e| json!({ "entity": e.entity.path(), "project": e.project })).collect::<Vec<_>>(),
        "absent": report.absent.iter().map(adopt::EntityRef::path).collect::<Vec<_>>(),
        "changed": changed,
        "identical": report.with_status(adopt::Status::Identical).count(),
    });
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn rename_tool(
    solution: &Solution,
    arguments: RenameRequest,
) -> Result<Value, ToolError> {
    let word = arguments.kind.as_str();
    let kind = rename::Kind::from_word(word).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown rename kind `{word}`; use {}",
            rename::Kind::list_words()
        ))
    })?;
    let dry_run = arguments.dry_run;
    let request = rename::Request {
        kind,
        scope: arguments.scope.as_ref().cloned(),
        service: arguments.service.as_ref().cloned(),
        old: nonempty(&arguments.old, "old")?.to_string(),
        new: nonempty(&arguments.new, "new")?.to_string(),
        apply: !dry_run,
        include_outside: arguments.include_outside,
        skip_checks: arguments.skip_checks,
        expect_digest: arguments.plan_digest.as_ref().cloned(),
        database: rename::DatabaseFlags {
            sql: arguments.sql,
            no_sql: arguments.no_sql,
            dir: arguments.sql_dir.as_ref().cloned(),
        },
    };
    let request = commands::rename::RenameRequest {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        date: jiff::Zoned::now().strftime("%Y-%m-%d").to_string(),
        lock_label: "mcp rename".to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::rename::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result =
        rename::summary_json(solution, outcome.outcome(), arguments.include_outside, 10);
    add_notices(&mut result, &notices);
    Ok(result)
}
