use super::source::add_types_refresh;
use super::*;

pub(crate) fn move_member_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let action = required(arguments, "action")?;
    if !matches!(action, "move" | "copy") {
        return Err(ToolError::invalid(format!(
            "unknown action `{action}`; use `move` or `copy`"
        )));
    }
    let kind = required(arguments, "kind")?;
    let member = relocate::Member::from_word(kind).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown kind `{kind}`; use `service` or `property`"
        ))
    })?;
    let request = relocate::Request {
        member,
        copy: action == "copy",
        from: required(arguments, "from")?.to_string(),
        to: required(arguments, "to")?.to_string(),
        name: required(arguments, "name")?.to_string(),
        new_name: text(arguments, "new_name").map(str::to_string),
        leave_delegate: flag(arguments, "leave_delegate", false),
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
    arguments: &Value,
) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let kind_word = text(arguments, "type").unwrap_or("standard");
    let kind = newblock::BlockType::from_word(kind_word).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown type `{kind_word}`; use standard, abstract or implementation"
        ))
    })?;
    let request = newblock::Request {
        name: required(arguments, "name")?.to_string(),
        kind,
        display_name: text(arguments, "display_name").map(str::to_string),
        description: text(arguments, "description")
            .unwrap_or_default()
            .to_string(),
        parent: text(arguments, "parent").map(str::to_string),
        model_logic: flag(arguments, "model_logic", false),
        management_shape: flag(arguments, "management_shape", true),
        root: text(arguments, "root").map(str::to_string),
        base_extension: text(arguments, "base_extension").map(str::to_string),
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

pub(crate) fn retemplate_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let request = retemplate::Request {
        entity: required(arguments, "entity")?.to_string(),
        template: text(arguments, "template").map(str::to_string),
        add_shapes: strings(arguments, "add_shapes"),
        remove_shapes: strings(arguments, "remove_shapes"),
        accept_loss: flag(arguments, "accept_loss", false),
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

pub(crate) fn adopt_apply_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let export = required(arguments, "export")?;
    let export = {
        let path = PathBuf::from(export);
        if path.is_absolute() {
            path
        } else {
            solution.root.join(path)
        }
    };
    let only: Vec<String> = arguments
        .get("entity")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
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

pub(crate) fn adopt_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let export = required(arguments, "export")?;
    let export = {
        let path = PathBuf::from(export);
        if path.is_absolute() {
            path
        } else {
            solution.root.join(path)
        }
    };
    let only: Vec<String> = arguments
        .get("entity")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
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
    let detail = flag(arguments, "detail", false);
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

pub(crate) fn rename_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let word = required(arguments, "kind")?;
    let kind = rename::Kind::from_word(word).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown rename kind `{word}`; use {}",
            rename::Kind::list_words()
        ))
    })?;
    let dry_run = flag(arguments, "dry_run", true);
    let request = rename::Request {
        kind,
        scope: text(arguments, "scope").map(str::to_string),
        service: text(arguments, "service").map(str::to_string),
        old: required(arguments, "old")?.to_string(),
        new: required(arguments, "new")?.to_string(),
        apply: !dry_run,
        include_outside: flag(arguments, "include_outside", false),
        skip_checks: flag(arguments, "skip_checks", false),
        expect_digest: text(arguments, "plan_digest").map(str::to_string),
        database: rename::DatabaseFlags {
            sql: flag(arguments, "sql", false),
            no_sql: flag(arguments, "no_sql", false),
            dir: text(arguments, "sql_dir").map(str::to_string),
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
    let mut result = rename::summary_json(
        solution,
        outcome.outcome(),
        flag(arguments, "include_outside", false),
        10,
    );
    add_notices(&mut result, &notices);
    Ok(result)
}
