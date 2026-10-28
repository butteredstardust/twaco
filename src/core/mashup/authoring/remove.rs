//! The inverses of mashup authoring: remove a widget, one binding, or one event handler.

use super::bind::existing_data_call;
use super::end::{area, details, property, resolve_end, End};
use super::{finish_edit, load_existing, parse_loaded, AuthoringContext, AuthoringError, EditPlan};
use crate::core::config::Solution;
use crate::core::mashup::address::parse_address;
use crate::core::mashup::describe::mashup_parameters;
use crate::core::mashup::edit::MashupDocument;
use crate::core::{mashup, types};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct RemoveRequest {
    pub mashup: String,
    pub widget: String,
    pub cascade: bool,
}

#[derive(Debug)]
pub struct RemovePlan {
    pub edit: Option<EditPlan>,
    pub widget: String,
    pub removed: Vec<String>,
    pub bindings_removed: usize,
    pub events_removed: usize,
    /// Data calls which no remaining binding or event names. They deliberately stay in Data.
    pub orphaned_data_calls: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct UnbindRequest {
    pub mashup: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug)]
pub struct UnbindPlan {
    pub edit: EditPlan,
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug)]
pub struct OffRequest {
    pub mashup: String,
    pub event: String,
    pub invoke: String,
}

#[derive(Debug)]
pub struct OffPlan {
    pub edit: EditPlan,
    pub event: String,
    pub invoke: String,
}

pub(crate) fn plan_remove(
    solution: &Solution,
    request: &RemoveRequest,
) -> Result<RemovePlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let found = match document.find(&request.widget) {
        Ok(found) => found,
        Err(crate::core::mashup::edit::EditError::NoWidget(_)) if request.cascade => {
            return Ok(RemovePlan {
                edit: None,
                widget: request.widget.clone(),
                removed: Vec::new(),
                bindings_removed: 0,
                events_removed: 0,
                orphaned_data_calls: Vec::new(),
            });
        }
        Err(error) => return Err(AuthoringError::Edit(error)),
    };
    if found.path.is_empty() {
        return Err(AuthoringError::Refused(
            "the mashup root cannot be removed".into(),
        ));
    }
    let removed = document
        .widgets()
        .into_iter()
        .filter(|widget| widget.path.starts_with(&found.path))
        .map(|widget| widget.id)
        .collect::<Vec<_>>();
    let ids = removed.iter().cloned().collect::<BTreeSet<_>>();
    let (bindings, events) = dependencies(&document, &ids);
    let unknown = unknown_references(document.value(), &ids);
    if !unknown.is_empty() {
        return Err(AuthoringError::Refused(format!(
            "{} is named by data-call configuration or extension JSON at {}; remove that reference in Composer first",
            found.id,
            unknown.join(", ")
        )));
    }
    if (!bindings.is_empty() || !events.is_empty()) && !request.cascade {
        let dependencies = [
            (!bindings.is_empty()).then(|| format!("{} binding(s)", bindings.len())),
            (!events.is_empty()).then(|| format!("{} event(s)", events.len())),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" and ");
        return Err(AuthoringError::Refused(format!(
            "{} is named by {dependencies}; use --cascade to remove those proven dependencies",
            found.id
        )));
    }
    let candidates = data_calls(&document, &bindings, &events);
    if request.cascade {
        remove_indexes(document.bindings_mut(), &bindings);
        remove_indexes(document.events_mut(), &events);
    }
    document
        .remove_widget(&found.path)
        .map_err(AuthoringError::Edit)?;
    let orphaned_data_calls = candidates
        .into_iter()
        .filter(|call| !data_call_referenced(&document, call))
        .map(|(section, id)| format!("{section}.{id}"))
        .collect();
    let edit = finish_edit(loaded, document, "mashup remove")?;
    Ok(RemovePlan {
        edit: Some(edit),
        widget: found.id,
        removed,
        bindings_removed: bindings.len(),
        events_removed: events.len(),
        orphaned_data_calls,
    })
}

pub(crate) fn plan_unbind(
    solution: &Solution,
    request: &UnbindRequest,
) -> Result<UnbindPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let parameters =
        mashup_parameters(&loaded.entity_before).map_err(AuthoringError::Unreadable)?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let context = AuthoringContext {
        parameters: &parameters,
        model: &model,
        platform: platform.as_ref(),
    };
    let from = endpoint(&mut document, solution, &context, &request.from, true)?;
    let to = endpoint(&mut document, solution, &context, &request.to, false)?;
    let from_location = endpoint_location(&document, &from)?;
    let to_location = endpoint_location(&document, &to)?;
    let matching = document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, binding)| binding_matches(binding, &from, &from_location, &to, &to_location))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let index = one(
        matching,
        format!("{} is not bound to {}", request.from, request.to),
    )?;
    document.bindings_mut().remove(index);
    Ok(UnbindPlan {
        edit: finish_edit(loaded, document, "mashup unbind")?,
        from: request.from.clone(),
        to: request.to.clone(),
    })
}

pub(crate) fn plan_off(
    solution: &Solution,
    request: &OffRequest,
) -> Result<OffPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let parameters = Vec::new();
    let context = AuthoringContext {
        parameters: &parameters,
        model: &model,
        platform: platform.as_ref(),
    };
    let routed = super::on::existing_child_event(&document, &request.event)?;
    let trigger = super::on::resolve_trigger(
        &document,
        context.model,
        context.platform,
        routed.as_deref().unwrap_or(&request.event),
    )?;
    let handler =
        super::on::resolve_handler(&document, context.model, context.platform, &request.invoke)?;
    let (section, id) = existing_data_call(&document, &handler)?.unwrap_or_else(|| {
        (
            String::new(),
            match &handler {
                End::Ui { id, .. } => id.clone(),
                End::Mashup { .. } => mashup::edit::ROOT_ID.into(),
                End::Data { .. } => String::new(),
            },
        )
    });
    if matches!(handler, End::Data { .. }) && section.is_empty() {
        return Err(AuthoringError::Unknown(format!(
            "{} is not a call this mashup has",
            request.invoke
        )));
    }
    let (handler_id, service) = if matches!(handler, End::Data { .. }) {
        (section, id)
    } else {
        (id, super::on::handler_service(&handler).to_string())
    };
    let matching = document.value()["Events"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, event)| {
            event["EventTriggerArea"] == trigger.area
                && event["EventTriggerSection"] == trigger.section
                && event["EventTriggerId"] == trigger.id
                && event["EventTriggerEvent"] == trigger.event
                && event["EventHandlerArea"] == area(&handler)
                && event["EventHandlerId"] == handler_id
                && event["EventHandlerService"] == service
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let index = one(
        matching,
        format!("{} does not invoke {}", request.event, request.invoke),
    )?;
    document.events_mut().remove(index);
    Ok(OffPlan {
        edit: finish_edit(loaded, document, "mashup off")?,
        event: request.event.clone(),
        invoke: request.invoke.clone(),
    })
}

fn endpoint(
    document: &mut MashupDocument,
    solution: &Solution,
    context: &AuthoringContext<'_>,
    raw: &str,
    source: bool,
) -> Result<End, AuthoringError> {
    let address = parse_address(raw).map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    let mut refreshed = Vec::new();
    resolve_end(
        document,
        Some(solution),
        context,
        address,
        source,
        &mut refreshed,
    )
}

fn endpoint_location(
    document: &MashupDocument,
    end: &End,
) -> Result<(String, String), AuthoringError> {
    match end {
        End::Data { .. } => existing_data_call(document, end)?.ok_or_else(|| {
            AuthoringError::Unknown("the addressed data call is not in this mashup".into())
        }),
        End::Ui { id, .. } => Ok((String::new(), id.clone())),
        End::Mashup { .. } => Ok((String::new(), mashup::edit::ROOT_ID.into())),
    }
}

fn binding_matches(
    binding: &Value,
    from: &End,
    from_location: &(String, String),
    to: &End,
    to_location: &(String, String),
) -> bool {
    let (source_section, source_id) = from_location;
    let (target_section, target_id) = to_location;
    binding["SourceArea"] == area(from)
        && binding["SourceSection"] == *source_section
        && binding["SourceId"] == *source_id
        && binding["SourceDetails"] == details(from)
        && binding["TargetArea"] == area(to)
        && binding["TargetSection"] == *target_section
        && binding["TargetId"] == *target_id
        && binding["PropertyMaps"].as_array().is_some_and(|maps| {
            maps.iter().any(|map| {
                map["SourceProperty"] == property(from) && map["TargetProperty"] == property(to)
            })
        })
}

fn one(matches: Vec<usize>, absent: String) -> Result<usize, AuthoringError> {
    match matches.as_slice() {
        [] => Err(AuthoringError::Unknown(absent)),
        [only] => Ok(*only),
        many => Err(AuthoringError::Ambiguous(format!(
            "{} matching entries; remove one in Composer first",
            many.len()
        ))),
    }
}

fn dependencies(document: &MashupDocument, ids: &BTreeSet<String>) -> (Vec<usize>, Vec<usize>) {
    let bindings = document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, binding)| named(binding, "SourceId", ids) || named(binding, "TargetId", ids))
        .map(|(index, _)| index)
        .collect();
    let events = document.value()["Events"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, event)| {
            named(event, "EventTriggerId", ids) || named(event, "EventHandlerId", ids)
        })
        .map(|(index, _)| index)
        .collect();
    (bindings, events)
}

fn named(value: &Value, key: &str, ids: &BTreeSet<String>) -> bool {
    value[key].as_str().is_some_and(|text| ids.contains(text))
}
fn remove_indexes(values: &mut Vec<Value>, indexes: &[usize]) {
    for &index in indexes.iter().rev() {
        values.remove(index);
    }
}

fn data_calls(
    document: &MashupDocument,
    bindings: &[usize],
    events: &[usize],
) -> BTreeSet<(String, String)> {
    let mut calls = BTreeSet::new();
    for index in bindings {
        for (area_key, section_key, id_key) in [
            ("SourceArea", "SourceSection", "SourceId"),
            ("TargetArea", "TargetSection", "TargetId"),
        ] {
            let value = &document.value()["DataBindings"][*index];
            if value[area_key] == "Data" {
                if let (Some(section), Some(id)) =
                    (value[section_key].as_str(), value[id_key].as_str())
                {
                    calls.insert((section.into(), id.into()));
                }
            }
        }
    }
    for index in events {
        let value = &document.value()["Events"][*index];
        for (area_key, section_key, id_key) in [
            ("EventTriggerArea", "EventTriggerSection", "EventTriggerId"),
            ("EventHandlerArea", "EventHandlerId", "EventHandlerService"),
        ] {
            if value[area_key] == "Data" {
                let section = value[section_key].as_str().unwrap_or("");
                if let Some(id) = value[id_key].as_str() {
                    calls.insert((section.into(), id.into()));
                }
            }
        }
    }
    calls
}

fn data_call_referenced(document: &MashupDocument, call: &(String, String)) -> bool {
    ["DataBindings", "Events"].into_iter().any(|section| {
        document.value()[section]
            .as_array()
            .into_iter()
            .flatten()
            .any(|value| {
                [
                    ("SourceArea", "SourceSection", "SourceId"),
                    ("TargetArea", "TargetSection", "TargetId"),
                    ("EventTriggerArea", "EventTriggerSection", "EventTriggerId"),
                    ("EventHandlerArea", "EventHandlerId", "EventHandlerService"),
                ]
                .into_iter()
                .any(|(area, key, id)| {
                    value[area] == "Data"
                        && value[key].as_str().unwrap_or("") == call.0
                        && value[id].as_str() == Some(&call.1)
                })
            })
    })
}

/// Any non-standard location that names a removed widget is unsafe to rewrite automatically.
fn unknown_references(root: &Value, ids: &BTreeSet<String>) -> Vec<String> {
    let mut found = Vec::new();
    if let Some(data) = root.get("Data") {
        scan(data, ids, "Data", &mut found);
    }
    for (key, value) in root.as_object().into_iter().flatten() {
        if !matches!(key.as_str(), "UI" | "Data" | "DataBindings" | "Events") {
            scan(value, ids, key, &mut found);
        }
    }
    scan_ui(&root["UI"], ids, "UI", &mut found);
    for section in ["DataBindings", "Events"] {
        for (index, value) in root[section].as_array().into_iter().flatten().enumerate() {
            scan_unknown_entry(value, ids, &format!("{section}[{index}]"), &mut found);
        }
    }
    found
}

fn scan_ui(value: &Value, ids: &BTreeSet<String>, path: &str, found: &mut Vec<String>) {
    if let Some(properties) = value.get("Properties").and_then(Value::as_object) {
        for (key, value) in properties {
            if key != "Id" {
                scan(value, ids, &format!("{path}.Properties.{key}"), found);
            }
        }
    }
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if !matches!(key.as_str(), "Properties" | "Widgets") {
                scan(value, ids, &format!("{path}.{key}"), found);
            }
        }
    }
    for (index, child) in value["Widgets"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        scan_ui(child, ids, &format!("{path}.Widgets[{index}]"), found);
    }
}

fn scan_unknown_entry(value: &Value, ids: &BTreeSet<String>, path: &str, found: &mut Vec<String>) {
    let known = [
        "Id",
        "SourceArea",
        "SourceSection",
        "SourceId",
        "SourceDetails",
        "TargetArea",
        "TargetSection",
        "TargetId",
        "EventHandlerArea",
        "EventHandlerId",
        "EventHandlerService",
        "EventHandlerServiceMethod",
        "EventTriggerArea",
        "EventTriggerEvent",
        "EventTriggerId",
        "EventTriggerSection",
        "PropertyMaps",
        "SourceProperty",
        "SourcePropertyBaseType",
        "SourcePropertyType",
        "TargetProperty",
        "TargetPropertyBaseType",
        "TargetPropertyType",
    ];
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if !known.contains(&key.as_str()) {
                scan(value, ids, &format!("{path}.{key}"), found);
            }
        }
    }
}

fn scan(value: &Value, ids: &BTreeSet<String>, path: &str, found: &mut Vec<String>) {
    match value {
        Value::String(text) if ids.contains(text) => found.push(path.into()),
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                scan(value, ids, &format!("{path}[{index}]"), found);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                scan(value, ids, &format!("{path}.{key}"), found);
            }
        }
        _ => {}
    }
}
