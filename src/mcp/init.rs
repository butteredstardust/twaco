use super::requests::init::InitFromServerRequest;
use super::{json, profile, server, ToolError, Value};
use std::path::Path;

pub(crate) fn init_from_server_tool(
    root: &Path,
    request: InitFromServerRequest,
) -> Result<Value, ToolError> {
    crate::core::init_server::preflight(root, &request.projects).map_err(ToolError::coded)?;
    let profile = profile::load(root, &request.profile).map_err(ToolError::coded)?;
    let outcome = crate::core::init_server::execute(
        &server::Client::new(profile),
        crate::core::init_server::Request {
            root: root.to_path_buf(),
            projects: request.projects,
            apply: request.apply,
            profile: request.profile,
        },
    )
    .map_err(ToolError::coded)?;
    Ok(json!({
        "projects": outcome.projects,
        "files": outcome.files,
        "entities": outcome.entities,
        "toml": outcome.toml,
        "notes": outcome.notes,
        "applied": outcome.applied,
        "added": outcome.added,
        "recorded": outcome.recorded,
        "not_in_sync": outcome.not_in_sync,
        "localization_files": outcome.localization_files,
        "localization_tokens": outcome.localization_tokens,
        "localization_failure": outcome.localization_failure,
    }))
}
