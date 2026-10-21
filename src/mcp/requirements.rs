use super::requests::requirements::{CheckRequest, InitRequest};
use super::{add_notices, commands, json, Mode, Solution, ToolError, Value};
use crate::core::progress::Progress;
use crate::core::{commands::requirements, server};

pub(crate) fn init_tool(solution: &Solution, request: InitRequest) -> Result<Value, ToolError> {
    let mut notices = commands::Notices::default();
    let outcome = requirements::execute_init(
        solution,
        &requirements::InitRequest {
            mode: if request.dry_run {
                Mode::Plan
            } else {
                Mode::Apply
            },
            lock_label: "mcp requirements_init",
        },
        &mut notices,
    )
    .map_err(ToolError::coded)?;
    let mut value = json!({"summary": outcome.report.summary(), "dry_run": request.dry_run, "written": outcome.written, "file": "requirements.toml", "draft": outcome.text, "requirements": outcome.report.json(true)});
    add_notices(&mut value, &notices);
    Ok(value)
}

pub(crate) fn check_tool(
    solution: &Solution,
    request: CheckRequest,
    progress: &dyn Progress,
) -> Result<Value, ToolError> {
    let report = requirements::execute_check(
        solution,
        &requirements::CheckRequest {
            server: request.server.then_some(request.profile),
        },
        server::Client::new,
        progress,
    )
    .map_err(ToolError::coded)?;
    Ok(report.json(request.detail))
}
