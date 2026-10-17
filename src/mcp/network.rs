use super::requests::network::{
    NetworkAction, NetworkRequest, NetworkWriteAction, NetworkWriteRequest,
};
use super::{add_notices, commands, in_path, json, nonempty, Mode, Solution, ToolError, Value};
use crate::core::{network, workspace};

fn read(solution: &Solution, name: &str) -> Result<network::Network, ToolError> {
    let entries: Vec<_> = workspace::entities(solution)
        .into_iter()
        .filter(|entry| entry.info.collection == "Networks")
        .collect();
    let file = workspace::resolve(&entries, name)
        .map_err(|error| ToolError::invalid(error.to_string()))?;
    let bytes = std::fs::read(&file.path)
        .map_err(|error| ToolError::invalid(format!("{}: {error}", file.path.display())))?;
    network::parse(&bytes).map_err(|error| ToolError::invalid(error.to_string()))
}
pub(crate) fn network_tool(
    solution: &Solution,
    request: NetworkRequest,
) -> Result<Value, ToolError> {
    match request.action {
        NetworkAction::List => Ok(json!({"networks": network::list_json(solution)})),
        NetworkAction::Show => {
            let name = super::required_text(&request.network, "network")?;
            let model = read(solution, name)?;
            let depth = request
                .depth
                .as_ref()
                .map(|value| {
                    usize::try_from(*value).map_err(|_| ToolError::invalid("depth is too large"))
                })
                .transpose()?;
            let report = network::show(solution, &model, request.top.as_deref(), depth)
                .map_err(|error| ToolError::invalid(error.to_string()))?;
            let mut value = network::show_json(&model, &report);
            if !request.detail
                && value["tree"]
                    .as_array()
                    .is_some_and(|tree| tree.len() > 200)
            {
                value["tree"].as_array_mut().expect("array").truncate(200);
                value["note"] = json!("tree was cut at 200 rows; pass detail: true for every row");
            }
            Ok(value)
        }
    }
}
pub(crate) fn network_write_tool(
    solution: &Solution,
    request: NetworkWriteRequest,
) -> Result<Value, ToolError> {
    let dry_run = request.dry_run;
    let mode = if dry_run { Mode::Plan } else { Mode::Apply };
    let mut notices = commands::Notices::default();
    if request.action == NetworkWriteAction::Import {
        let file = in_path(solution, super::required_text(&request.file, "file")?)?;
        let outcome = commands::network::import_execute(
            solution,
            &commands::network::ImportRequest {
                network: nonempty(&request.network, "network")?.into(),
                file,
                create_things: request.create_things,
                move_existing: request.r#move,
                outside: request.outside,
                mode,
                lock_label: "mcp network_write import",
            },
            &mut notices,
        )
        .map_err(ToolError::coded)?;
        let created: Vec<_> = outcome.plan.created.iter().map(|plan| json!({"name": plan.request.name, "template": plan.request.template, "project": plan.project})).collect();
        let mut value = json!({"dry_run": dry_run, "applied": outcome.applied, "file": solution.relative(&outcome.plan.path), "created": created, "added": outcome.plan.added, "moved": outcome.plan.moved, "noops": outcome.plan.noops});
        add_notices(&mut value, &notices);
        return Ok(value);
    }
    let action = match request.action {
        NetworkWriteAction::Add => commands::network::Action::Add,
        NetworkWriteAction::Move => commands::network::Action::Move,
        NetworkWriteAction::Remove => commands::network::Action::Remove,
        NetworkWriteAction::Import => unreachable!(),
    };
    let outcome = commands::network::execute(
        solution,
        &commands::network::Request {
            action,
            network: nonempty(&request.network, "network")?.into(),
            thing: super::required_text(&request.thing, "thing")?.into(),
            parent: request.under.as_ref().cloned(),
            root: request.as_root,
            connection_type: request.connection_type.as_ref().cloned(),
            subtree: request.subtree,
            outside: request.outside,
            mode,
            lock_label: "mcp network_write",
        },
        &mut notices,
    )
    .map_err(ToolError::coded)?;
    let mut value = json!({"dry_run": dry_run, "applied": outcome.applied, "changed": outcome.plan.changed, "file": solution.relative(&outcome.plan.path), "removed": outcome.plan.removed});
    add_notices(&mut value, &notices);
    Ok(value)
}
