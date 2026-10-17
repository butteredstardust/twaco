//! `mashup event`: an event parameter and the wiring that raises it, in one edit.

use super::{
    bind_in, on_in, plan_parameter, AuthoringContext, AuthoringError, ParameterAction,
    ParameterDefault, ParameterPlan, ParameterRequest,
};
use crate::core::config::Solution;
use crate::core::mashup::describe::mashup_parameters;
use crate::core::{mashup, types, widgets};
use serde_json::json;

/// A request to give a mashup an event its parents can react to.
#[derive(Clone, Debug)]
pub struct EventRequest {
    pub mashup: String,
    pub name: String,
    /// The event in this mashup that fires it, as `mashup on --event` takes it.
    pub when: String,
}

/// Plan a child event: a NUMBER parameter (BOTH), refreshed in every parent's copy, set to the
/// time by an expression that `when` evaluates. Real mashups do it this way because ThingWorx has
/// no events of its own for a contained mashup: a changed value is the signal.
pub fn plan_event(
    solution: &Solution,
    request: &EventRequest,
) -> Result<ParameterPlan, AuthoringError> {
    let mut plan = plan_parameter(
        solution,
        &ParameterRequest {
            mashup: request.mashup.clone(),
            action: ParameterAction::Add,
            name: request.name.clone(),
            base_type: Some("NUMBER".into()),
            direction: Some("BOTH".into()),
            description: Some(format!("fired on {}", request.when)),
            default: ParameterDefault::Unchanged,
            data_shape: None,
            force: false,
        },
    )?;
    let child = plan
        .edits
        .first_mut()
        .expect("the mashup's own edit comes first");
    let content = match &child.sidecars {
        Some(sidecars) => String::from_utf8(sidecars.content_after.clone())
            .map_err(|error| AuthoringError::Invalid(error.to_string()))?,
        None => {
            mashup::extract(&child.entity_after)
                .map_err(AuthoringError::Mashup)?
                .content
        }
    };
    let mut document = mashup::edit::parse(&content).map_err(AuthoringError::Edit)?;
    let catalog = widgets::catalog()
        .by_type("expression2")
        .expect("the catalogue has the expression; a test checks it");
    let number = document.allocate();
    let mut expression = mashup::edit::new_widget(catalog, number, false);
    let id = expression["Properties"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let properties = mashup::edit::properties_mut(&mut expression);
    mashup::edit::insert_sorted(
        properties,
        "DisplayName",
        json!(format!("{}Event", request.name)),
    );
    mashup::edit::insert_sorted(properties, "Expression", json!("Output = Date.now();"));
    mashup::edit::insert_sorted(properties, "OutputBaseType", json!("NUMBER"));
    mashup::edit::insert_sorted(properties, "DataChangeType", json!("ALWAYS"));
    mashup::edit::insert_sorted(properties, "_currentParameterDefs", json!([]));
    document.push_child(&[], expression);
    let parameters = mashup_parameters(&child.entity_after).map_err(AuthoringError::Unreadable)?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let context = AuthoringContext {
        parameters: &parameters,
        model: &model,
        platform: platform.as_ref(),
    };
    on_in(
        &mut document,
        &context,
        &request.when,
        &format!("ui:{id}.Evaluate"),
    )?;
    bind_in(
        &mut document,
        Some(solution),
        &context,
        &format!("ui:{id}.Output"),
        &format!("mashup:{}", request.name),
    )?;
    let content = document.to_content();
    let css = match &child.sidecars {
        Some(sidecars) => sidecars.css.clone(),
        None => {
            mashup::extract(&child.entity_after)
                .map_err(AuthoringError::Mashup)?
                .css
        }
    };
    let (entity_after, _) = mashup::sync(
        &child.entity_after,
        &mashup::Assets {
            content: content.clone(),
            css,
        },
    )
    .map_err(AuthoringError::Mashup)?;
    child.entity_after = entity_after;
    if let Some(sidecars) = &mut child.sidecars {
        sidecars.content_after = content.into_bytes();
    }
    Ok(plan)
}
