use super::{finish_edit, load_existing, parse_loaded, AuthoringError, EditPlan};
use crate::core::config::Solution;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct GapRequest {
    pub mashup: String,
    pub container: String,
    pub px: i32,
}

#[derive(Debug)]
pub struct GapPlan {
    pub(crate) edit: Option<EditPlan>,
    pub mashup: String,
    pub container: String,
    pub px: i32,
    pub spacers: usize,
}

pub(crate) fn plan_gap(
    solution: &Solution,
    request: &GapRequest,
) -> Result<GapPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let before = document.to_content();
    let spacers = document
        .gap(&request.container, request.px)
        .map_err(AuthoringError::Edit)?;
    let edit = if before == document.to_content() {
        None
    } else {
        Some(finish_edit(loaded, document, "mashup gap")?)
    };
    Ok(GapPlan {
        edit,
        mashup: request.mashup.clone(),
        container: request.container.clone(),
        px: request.px,
        spacers,
    })
}

pub fn gap_plan_json(_solution: &Solution, plan: &GapPlan, applied: bool) -> Value {
    json!({"applied": applied, "dry_run": !applied, "mashup": plan.mashup, "container": plan.container, "px": plan.px, "spacers": plan.spacers})
}
