//! `mashup on`: an event wired to a service, a widget's handler or a contained mashup's event.

use super::{
    area, data_call, existing_data_call, finish_edit, insert, load_existing, parse_loaded,
    resolve_service, AuthoringContext, AuthoringError, EditPlan, End,
};
use crate::core::config::Solution;
use crate::core::mashup::address::{parse_address, Address};
use crate::core::mashup::edit::MashupDocument;
use crate::core::{mashup, types, widgets};
use serde_json::{json, Map, Value};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct OnRequest {
    pub mashup: String,
    pub event: String,
    pub invoke: String,
}

#[derive(Debug)]
pub struct OnPlan {
    pub edit: EditPlan,
    pub event: String,
    pub invoke: String,
    pub new_source: Option<String>,
}

/// A child mashup's event is a BOTH or OUT parameter whose value it changes (to a timestamp, see
/// `mashup event add`). ThingWorx has no contained-mashup events, so the parent reacts the way real
/// mashups do: the container's parameter feeds an events router whose `Changed` runs the handler.
/// Returns the router's `Changed` address when `event_raw` names such a parameter.
fn route_child_event(
    document: &mut MashupDocument,
    event_raw: &str,
) -> Result<Option<String>, AuthoringError> {
    let Some((widget, name)) = event_raw
        .strip_prefix("ui:")
        .and_then(|rest| rest.rsplit_once('.'))
    else {
        return Ok(None);
    };
    let Ok(found) = document.find_exact(widget) else {
        return Ok(None);
    };
    if found.type_id != "mashupcontainer" {
        return Ok(None);
    }
    let container = &document.at(&found.path)["Properties"];
    let child = container["Name"].as_str().unwrap_or_default().to_string();
    let entries = container["MashupParameters"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let entry = entries
        .iter()
        .find(|entry| entry["ParameterName"] == name)
        .ok_or_else(|| {
            let events: Vec<&str> = entries
                .iter()
                .filter(|entry| entry["ParmDef"]["aspects"]["bindingDirection"] != "IN")
                .filter_map(|entry| entry["ParameterName"].as_str())
                .collect();
            AuthoringError::Unknown(format!(
                "{} shows {child}, which has no parameter {name}; its events are its BOTH or OUT parameters: {}",
                found.id,
                if events.is_empty() { "none".to_string() } else { events.join(", ") }
            ))
        })?;
    if entry["ParmDef"]["aspects"]["bindingDirection"] == "IN" {
        return Err(AuthoringError::Refused(format!(
            "{name} is an input of {child}; an event is a BOTH or OUT parameter"
        )));
    }
    let base = entry["BaseType"].as_str().unwrap_or("STRING").to_string();
    // Reuse a router this parameter already feeds.
    let existing = document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|binding| {
            binding["SourceId"] == found.id
                && binding["PropertyMaps"][0]["SourceProperty"] == name
                && binding["TargetId"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("eventsrouter-"))
        })
        .and_then(|binding| binding["TargetId"].as_str().map(str::to_string));
    if let Some(router) = existing {
        return Ok(Some(format!("ui:{router}.Changed")));
    }
    let catalog = widgets::catalog()
        .by_type("eventsrouter")
        .expect("the catalogue has the events router; a test checks it");
    let number = document.allocate();
    let mut router = mashup::edit::new_widget(catalog, number, false);
    let properties = mashup::edit::properties_mut(&mut router);
    mashup::edit::insert_sorted(properties, "DataType", json!(base));
    mashup::edit::insert_sorted(properties, "NumberOfInputs", json!("1"));
    let router_id = properties["Id"].as_str().unwrap_or_default().to_string();
    document.push_child(&[], router);
    let mut map = Map::new();
    insert(&mut map, "SourceProperty", json!(name));
    insert(&mut map, "SourcePropertyBaseType", json!(base));
    insert(&mut map, "SourcePropertyType", json!("Property"));
    insert(&mut map, "TargetProperty", json!("Input1"));
    insert(&mut map, "TargetPropertyBaseType", json!(base));
    insert(&mut map, "TargetPropertyType", json!("property"));
    let mut binding = Map::new();
    insert(&mut binding, "Id", json!(Uuid::new_v4().to_string()));
    insert(&mut binding, "PropertyMaps", json!([Value::Object(map)]));
    insert(&mut binding, "SourceArea", json!("UI"));
    insert(&mut binding, "SourceDetails", json!(""));
    insert(&mut binding, "SourceId", json!(found.id));
    insert(&mut binding, "SourceSection", json!(""));
    insert(&mut binding, "TargetArea", json!("UI"));
    insert(&mut binding, "TargetId", json!(router_id));
    insert(&mut binding, "TargetSection", json!(""));
    document.bindings_mut().push(Value::Object(binding));
    Ok(Some(format!("ui:{router_id}.Changed")))
}

/// Plan one event handler, adding a Thing service call for a data handler when needed.
pub fn plan_on(solution: &Solution, request: &OnRequest) -> Result<OnPlan, AuthoringError> {
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
    let new_source = on_in(&mut document, &context, &request.event, &request.invoke)?;
    let edit = finish_edit(loaded, document, "mashup on")?;
    Ok(OnPlan {
        edit,
        event: request.event.clone(),
        invoke: request.invoke.clone(),
        new_source,
    })
}

/// Add one checked event handler to an already loaded document.
pub(super) fn on_in(
    document: &mut MashupDocument,
    context: &AuthoringContext<'_>,
    event_raw: &str,
    invoke_raw: &str,
) -> Result<Option<String>, AuthoringError> {
    let routed = route_child_event(document, event_raw)?;
    let event_raw = routed.as_deref().unwrap_or(event_raw);
    let trigger = resolve_trigger(document, context.model, context.platform, event_raw)?;
    let handler = resolve_handler(document, context.model, context.platform, invoke_raw)?;
    let (handler_section, handler_id, new_source) = data_call(document, &handler)?;
    // A data handler is addressed by its section and the call's alias; a widget handler by the
    // widget's id and the widget's own service.
    let (handler_section, handler_service) = if matches!(handler, End::Data { .. }) {
        (handler_section, handler_id.clone())
    } else {
        (handler_id.clone(), handler_service(&handler).to_string())
    };
    if event_exists(
        document,
        &trigger,
        area(&handler),
        &handler_section,
        &handler_service,
    ) {
        return Err(AuthoringError::Exists(format!(
            "{} already invokes {}",
            event_raw, invoke_raw
        )));
    }
    let mut event = Map::new();
    insert(
        &mut event,
        "EventHandlerArea",
        Value::String(area(&handler).into()),
    );
    insert(&mut event, "EventHandlerId", Value::String(handler_section));
    insert(
        &mut event,
        "EventHandlerService",
        Value::String(handler_service),
    );
    insert(
        &mut event,
        "EventHandlerServiceMethod",
        Value::String(String::new()),
    );
    insert(
        &mut event,
        "EventTriggerArea",
        Value::String(trigger.area.into()),
    );
    insert(
        &mut event,
        "EventTriggerEvent",
        Value::String(trigger.event),
    );
    insert(&mut event, "EventTriggerId", Value::String(trigger.id));
    insert(
        &mut event,
        "EventTriggerSection",
        Value::String(trigger.section),
    );
    insert(&mut event, "Id", Value::String(Uuid::new_v4().to_string()));
    document.events_mut().push(Value::Object(event));
    Ok(new_source)
}

struct Trigger {
    area: &'static str,
    section: String,
    id: String,
    event: String,
}

fn resolve_trigger(
    document: &MashupDocument,
    model: &types::Model,
    platform: Option<&types::Platform>,
    raw: &str,
) -> Result<Trigger, AuthoringError> {
    if raw == "mashup:Loaded" {
        return Ok(Trigger {
            area: "Mashup",
            section: String::new(),
            id: mashup::edit::ROOT_ID.into(),
            event: "Loaded".into(),
        });
    }
    if raw.starts_with("mashup:") {
        return Err(AuthoringError::Invalid("only mashup:Loaded".into()));
    }
    if let Some(rest) = raw.strip_prefix("ui:") {
        let (widget, event) = rest
            .rsplit_once('.')
            .ok_or_else(|| AuthoringError::Invalid(format!("invalid mashup address `{raw}`")))?;
        let found = document.find_exact(widget).map_err(AuthoringError::Edit)?;
        if found.type_id == "mashupcontainer" {
            return Err(AuthoringError::Invalid(
                "a contained mashup's events are its BOTH or OUT parameters (see mashup event add)"
                    .into(),
            ));
        }
        let catalog = event_contract(&found.id, &found.type_id)?;
        if !catalog.has_event(event) {
            return Err(AuthoringError::Invalid(format!(
                "widget `{}` has no event {}; its events are {}",
                found.id,
                event,
                catalog
                    .events
                    .as_ref()
                    .map_or(String::new(), |events| events.join(", "))
            )));
        }
        return Ok(Trigger {
            area: "UI",
            section: String::new(),
            id: found.id,
            event: event.into(),
        });
    }
    let rest = raw
        .strip_prefix("data:")
        .ok_or_else(|| AuthoringError::Invalid(format!("invalid mashup address `{raw}`")))?;
    let (call, event) = rest
        .rsplit_once('.')
        .ok_or_else(|| AuthoringError::Invalid(format!("invalid mashup address `{raw}`")))?;
    if !matches!(
        event,
        "ServiceInvokeCompleted" | "AllDataChanged" | "SelectedRowsChanged"
    ) {
        return Err(AuthoringError::Invalid(format!(
            "invalid mashup event `{raw}`"
        )));
    }
    let address = parse_address(&format!("data:{call}"))
        .map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let end = resolve_data_call(model, platform, address)?;
    let (section, id) = existing_data_call(document, &end)?
        .ok_or_else(|| AuthoringError::Invalid(format!("{call} is not a call this mashup has")))?;
    Ok(Trigger {
        area: "Data",
        section,
        id,
        event: event.into(),
    })
}

fn resolve_handler(
    document: &MashupDocument,
    model: &types::Model,
    platform: Option<&types::Platform>,
    raw: &str,
) -> Result<End, AuthoringError> {
    // The mashup's own services: a popup closing itself, a page reloading its data.
    if let Some(service) = raw.strip_prefix("mashup:") {
        if matches!(service, "CloseIfPopup" | "Refresh") {
            return Ok(End::Mashup {
                parameter: service.into(),
                base: String::new(),
            });
        }
        return Err(AuthoringError::Invalid(format!(
            "a mashup's own services are mashup:CloseIfPopup and mashup:Refresh, not {raw}"
        )));
    }
    if let Some(rest) = raw.strip_prefix("ui:") {
        let (widget, service) = rest
            .rsplit_once('.')
            .ok_or_else(|| AuthoringError::Invalid(format!("invalid mashup address `{raw}`")))?;
        let found = document.find_exact(widget).map_err(AuthoringError::Edit)?;
        if found.type_id == "mashupcontainer" {
            if service != "Refresh" {
                return Err(AuthoringError::Invalid(format!(
                    "a contained mashup has no service {service}; only Refresh is supported"
                )));
            }
            return Ok(End::Ui {
                id: found.id,
                property: service.into(),
                base: String::new(),
            });
        }
        let catalog = event_contract(&found.id, &found.type_id)?;
        if !catalog
            .services
            .as_ref()
            .is_some_and(|services| services.iter().any(|known| known == service))
        {
            return Err(AuthoringError::Invalid(format!(
                "widget `{}` has no service {service}",
                found.id
            )));
        }
        return Ok(End::Ui {
            id: found.id,
            property: service.into(),
            base: String::new(),
        });
    }
    let address = parse_address(raw).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    resolve_data_call(model, platform, address)
}

fn event_contract<'a>(id: &str, type_id: &str) -> Result<&'a widgets::Widget, AuthoringError> {
    widgets::catalog().by_type(type_id).filter(|widget| widget.has_contract()).ok_or_else(|| AuthoringError::Refused(format!(
        "widget `{id}` has type `{type_id}`, whose contract is not in the embedded ThingWorx 10.2 catalogue; twaco will not create an event for it"
    )))
}

fn resolve_data_call(
    model: &types::Model,
    platform: Option<&types::Platform>,
    address: Address,
) -> Result<End, AuthoringError> {
    let Address::Data {
        entity_service,
        alias,
        selected_rows,
        member,
    } = address
    else {
        return Err(AuthoringError::Invalid(
            "event data addresses name a service call".into(),
        ));
    };
    if selected_rows || member.is_some() {
        return Err(AuthoringError::Invalid(
            "event data addresses name a service call".into(),
        ));
    }
    let (entity, service) = resolve_service(model, platform, &entity_service)?;
    Ok(End::Data {
        entity: entity.name.clone(),
        collection: entity.collection.clone(),
        service,
        requested_alias: alias,
        selected_rows: false,
        member: None,
        base: String::new(),
        property_type: String::new(),
    })
}

fn handler_service(end: &End) -> &str {
    match end {
        End::Data {
            requested_alias,
            service,
            ..
        } => requested_alias.as_deref().unwrap_or(service),
        End::Ui { property, .. } => property,
        End::Mashup { parameter, .. } => parameter,
    }
}

fn event_exists(
    document: &MashupDocument,
    trigger: &Trigger,
    handler_area: &str,
    handler_id: &str,
    handler_service: &str,
) -> bool {
    document.value()["Events"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|event| {
            event["EventTriggerArea"] == trigger.area
                && event["EventTriggerSection"] == trigger.section
                && event["EventTriggerId"] == trigger.id
                && event["EventTriggerEvent"] == trigger.event
                && event["EventHandlerArea"] == handler_area
                && event["EventHandlerId"] == handler_id
                && event["EventHandlerService"] == handler_service
        })
}
