use super::requests::block::{BlockAction, BlockBump, BlockRequest, BlockVersionRequest};
use super::{add_notices, commands, json, outcome_key, Solution, ToolError, Value};
use crate::core::block::{self, Bump, VersionChange};
use crate::core::server;

pub(crate) fn block_tool(solution: &Solution, request: BlockRequest) -> Result<Value, ToolError> {
    let server_view = if request.server {
        Some(
            commands::block::server(solution, &request.profile, server::Client::new)
                .map_err(ToolError::invalid)?,
        )
    } else {
        None
    };
    match request.action {
        BlockAction::List => {
            let blocks = block::blocks(solution);
            let rows = match &server_view {
                Some((_, server)) => json!(block::compare(blocks, server)),
                None => json!(blocks),
            };
            Ok(json!({ "blocks": rows }))
        }
        BlockAction::Show => {
            let name = super::required_text(&request.block, "block")?;
            let found = block::find(solution, name).map_err(ToolError::invalid)?;
            let mut value = json!({ "block": found });
            if let Some((remote, server)) = &server_view {
                let compared = block::compare(vec![found.clone()], server).remove(0);
                value["notes"] = json!(compared.notes);
                if compared.server.is_some() {
                    match block::deploy_parameters(remote, &found.entry_point) {
                        Ok(parameters) => value["deploy_parameters"] = json!(parameters),
                        Err(error) => value["deploy_parameters_error"] = json!(error),
                    }
                }
                value["server"] = json!(compared.server);
            }
            Ok(value)
        }
    }
}

pub(crate) fn block_version_tool(
    solution: &Solution,
    request: BlockVersionRequest,
) -> Result<Value, ToolError> {
    let change = match (request.bump.as_ref(), request.set.as_ref()) {
        (Some(bump), None) => VersionChange::Bump(match bump {
            BlockBump::Patch => Bump::Patch,
            BlockBump::Minor => Bump::Minor,
            BlockBump::Major => Bump::Major,
        }),
        (None, Some(version)) => VersionChange::Set(version.clone()),
        _ => return Err(ToolError::invalid("give one of bump or set")),
    };
    let mut notices = commands::Notices::default();
    let plan = commands::block::version(
        solution,
        &request.block,
        &change,
        !request.dry_run,
        &mut notices,
    )
    .map_err(ToolError::coded)?;
    let mut value = json!({
        "ok": true, "dry_run": request.dry_run, "block": plan.block, "from": plan.from,
        "to": plan.to, "files": plan.files,
    });
    value[outcome_key(request.dry_run)] = json!(true);
    add_notices(&mut value, &notices);
    Ok(value)
}
