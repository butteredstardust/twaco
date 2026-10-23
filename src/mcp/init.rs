use super::requests::init::InitFromServerRequest;
use super::{ToolError, Value};
use std::path::Path;

pub(crate) fn init_from_server_tool(
    root: &Path,
    request: InitFromServerRequest,
) -> Result<Value, ToolError> {
    let outcome = crate::core::commands::init_server::init_from_server(
        crate::core::commands::init_server::InitFromServerRequest {
            root: root.to_path_buf(),
            projects: request.projects,
            profile: request.profile,
            apply: request.apply,
        },
    )
    .map_err(ToolError::coded)?;
    Ok(crate::core::commands::init_server::outcome_json(&outcome))
}
