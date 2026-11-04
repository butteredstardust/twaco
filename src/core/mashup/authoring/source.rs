//! Explicit static and dynamic mashup data sources.

use super::{finish_edit, insert, load_existing, parse_loaded, AuthoringError, EditPlan};
use crate::core::config::Solution;
use crate::core::mashup::address::{parse_address, Address};
use crate::core::mashup::describe::mashup_parameters;
use crate::core::{catalog, types};
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Things,
    Template,
    DynamicTemplate,
    DynamicShape,
}

impl SourceKind {
    /// The kind an entity's collection allows: a Thing is static, a ThingShape dynamic (a
    /// shape has no instance to run on), and a ThingTemplate either, as `dynamic` says.
    fn of(entity: &str, collection: &str, dynamic: bool) -> Result<Self, AuthoringError> {
        match (collection, dynamic) {
            ("Things", false) => Ok(Self::Things),
            ("Things", true) => Err(AuthoringError::Refused(format!(
                "{entity} is a Thing: its source is static; leave out --dynamic"
            ))),
            ("ThingTemplates", false) => Ok(Self::Template),
            ("ThingTemplates", true) => Ok(Self::DynamicTemplate),
            ("ThingShapes", _) => Ok(Self::DynamicShape),
            (other, _) => Err(AuthoringError::Refused(format!(
                "{entity} is in {other}: a data source runs a Thing's, ThingTemplate's or ThingShape's service"
            ))),
        }
    }
    fn entity_type(&self) -> &'static str {
        match self {
            Self::Things => "Things",
            Self::Template => "ThingTemplates",
            Self::DynamicTemplate => "DynamicThingTemplates",
            Self::DynamicShape => "DynamicThingShapes",
        }
    }
    pub(crate) fn dynamic(&self) -> bool {
        matches!(self, Self::DynamicTemplate | Self::DynamicShape)
    }
}

#[derive(Clone, Debug)]
pub struct SourceAddRequest {
    pub mashup: String,
    pub entity: String,
    pub service: String,
    /// A ThingTemplate's call runs on the Thing an EntityName binding names, not on the
    /// template itself.
    pub dynamic: bool,
    pub alias: Option<String>,
    pub entity_from: Option<String>,
    pub refresh: Option<u32>,
    pub cache: bool,
    pub cache_keys: Vec<String>,
    pub cache_strategy: Option<String>,
    pub max_result_sets: Option<u32>,
}
#[derive(Debug)]
pub struct SourceAddPlan {
    pub edit: EditPlan,
    pub section: String,
    pub call: String,
}
#[derive(Clone, Debug)]
pub struct SourceRemoveRequest {
    pub mashup: String,
    pub address: String,
    pub cascade: bool,
}
#[derive(Debug)]
pub struct SourceRemovePlan {
    pub edit: EditPlan,
    pub section: String,
    pub call: String,
    pub bindings_removed: usize,
    pub events_removed: usize,
}
#[derive(Clone, Debug)]
pub struct SourceRetargetRequest {
    pub mashup: String,
    pub from: String,
    pub to: String,
}
#[derive(Debug)]
pub struct SourceRetargetPlan {
    pub edit: EditPlan,
    pub sections: Vec<(String, String)>,
    pub bindings_rewritten: usize,
    pub events_rewritten: usize,
    pub dynamic: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct SourceInfo {
    pub kind: String,
    pub entity: String,
    pub service: String,
    pub alias: String,
    pub entity_from: String,
    pub refresh: u64,
    pub cache: bool,
}

pub(crate) fn dynamic_section(section: &Value) -> bool {
    matches!(
        section["EntityType"].as_str(),
        Some("DynamicThingShapes" | "DynamicThingTemplates")
    )
}
pub(crate) fn entity_slot_binding(binding: &Value, section: &str) -> bool {
    binding["TargetArea"] == "Data"
        && binding["TargetSection"] == section
        && binding["TargetId"] == "EntityName"
}

/// Whether `section` is a dynamic source that nothing names the Thing of yet.
pub(crate) fn unbound_dynamic(
    document: &crate::core::mashup::edit::MashupDocument,
    section: &str,
) -> bool {
    let value = document.value();
    dynamic_section(&value["Data"][section])
        && !value["DataBindings"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|binding| entity_slot_binding(binding, section))
}

pub(crate) fn plan_source_add(
    solution: &Solution,
    request: &SourceAddRequest,
) -> Result<SourceAddPlan, AuthoringError> {
    if !request.cache
        && (!request.cache_keys.is_empty()
            || request.cache_strategy.is_some()
            || request.max_result_sets.is_some())
    {
        return Err(AuthoringError::Invalid("cache options need --cache".into()));
    }
    if let Some(strategy) = &request.cache_strategy {
        if !["INSTANCE", "SESSION", "USER"].contains(&strategy.as_str()) {
            return Err(AuthoringError::Invalid(format!(
                "--cache-strategy is INSTANCE, SESSION or USER, not {strategy}"
            )));
        }
    }
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let entity = model
        .entities
        .iter()
        .find(|entity| entity.name == request.entity)
        .ok_or_else(|| {
            AuthoringError::Unknown(format!("no entity {} in the solution", request.entity))
        })?;
    let kind = SourceKind::of(&request.entity, &entity.collection, request.dynamic)?;
    if request.entity_from.is_some() && !kind.dynamic() {
        return Err(AuthoringError::Refused(format!(
            "--entity-from is for a dynamic source; {} runs on itself",
            request.entity
        )));
    }
    let service =
        catalog::lookup_service(entity, &model.entities, platform.as_ref(), &request.service)
            .service
            .ok_or_else(|| {
                AuthoringError::Unknown(format!(
                    "{}.{} is not a service in the solution",
                    request.entity, request.service
                ))
            })?;
    for key in &request.cache_keys {
        if !service
            .parameters
            .iter()
            .any(|parameter| parameter.name == *key)
        {
            return Err(AuthoringError::Unknown(format!(
                "{}.{} has no parameter {key} for --cache-key",
                request.entity, request.service
            )));
        }
    }
    let key = format!("{}_{}", kind.entity_type(), request.entity);
    if let Some(existing) = document.value()["Data"]
        .as_object()
        .and_then(|data| data.get(&key))
    {
        if existing["EntityType"] != kind.entity_type() {
            return Err(AuthoringError::Refused(format!(
                "{key} has a different EntityType"
            )));
        }
    } else {
        let mut section = Map::new();
        insert(&mut section, "DataName", Value::String(key.clone()));
        // A dynamic section names its shape or template here too; the Thing it runs on comes
        // from the EntityName binding.
        insert(
            &mut section,
            "EntityName",
            Value::String(request.entity.clone()),
        );
        insert(
            &mut section,
            "EntityType",
            Value::String(kind.entity_type().into()),
        );
        insert(
            &mut section,
            "Id",
            Value::String(Uuid::new_v4().to_string()),
        );
        insert(&mut section, "RefreshInterval", Value::from(0));
        insert(&mut section, "Services", Value::Array(Vec::new()));
        document
            .data_mut()
            .insert(key.clone(), Value::Object(section));
    }
    let services = document
        .data_mut()
        .get_mut(&key)
        .and_then(|v| v["Services"].as_array_mut())
        .ok_or_else(|| AuthoringError::Refused(format!("{key}.Services is not a list")))?;
    let call = request
        .alias
        .clone()
        .unwrap_or_else(|| allocate(services, &request.service));
    if let Some(existing) = services.iter().find(|value| value["Name"] == call) {
        return Err(AuthoringError::Exists(format!(
            "{} already has a call named {call}, for {}",
            request.entity,
            existing["Target"].as_str().unwrap_or("another service")
        )));
    }
    let mut value = Map::new();
    insert(&mut value, "APIMethod", Value::String("post".into()));
    insert(
        &mut value,
        "Characteristic",
        Value::String("Services".into()),
    );
    if request.cache {
        // Composer's shape: maxResultSets is a string.
        let mut caching = Map::new();
        insert(&mut caching, "cache", Value::Bool(true));
        insert(
            &mut caching,
            "cacheKeyParams",
            Value::Array(
                request
                    .cache_keys
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        insert(
            &mut caching,
            "cacheStrategy",
            Value::String(
                request
                    .cache_strategy
                    .clone()
                    .unwrap_or_else(|| "INSTANCE".into()),
            ),
        );
        insert(
            &mut caching,
            "maxResultSets",
            Value::String(request.max_result_sets.unwrap_or(10).to_string()),
        );
        insert(&mut value, "Caching", Value::Object(caching));
    }
    insert(&mut value, "Id", Value::String(Uuid::new_v4().to_string()));
    insert(&mut value, "Name", Value::String(call.clone()));
    insert(&mut value, "Parameters", Value::Object(Map::new()));
    insert(
        &mut value,
        "RefreshInterval",
        Value::from(request.refresh.unwrap_or(0)),
    );
    insert(&mut value, "Target", Value::String(request.service.clone()));
    services.push(Value::Object(value));
    if let Some(from) = &request.entity_from {
        let parameters =
            mashup_parameters(&loaded.entity_before).map_err(AuthoringError::Unreadable)?;
        if let Some(name) = from.strip_prefix("mashup:") {
            if !parameters.iter().any(|parameter| parameter.name == name) {
                return Err(AuthoringError::Unknown(format!("mashup has no parameter {name}; add it with `twaco mashup param add {} {name} --type THINGNAME --default <Thing>`", request.mashup)));
            }
        }
        let warnings = super::bind::bind_entity_slot(
            &mut document,
            solution,
            &parameters,
            &model,
            platform.as_ref(),
            from,
            &request.entity,
        )?;
        // A source that cannot run names no Thing: refuse rather than plan a dead call.
        if let Some(warning) = warnings.into_iter().next() {
            return Err(AuthoringError::Refused(format!(
                "--entity-from {from}: {warning}"
            )));
        }
    }
    Ok(SourceAddPlan {
        edit: finish_edit(loaded, document, "mashup source add")?,
        section: key,
        call,
    })
}

fn allocate(services: &[Value], service: &str) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                service.to_string()
            } else {
                format!("{service}_{n}")
            }
        })
        .find(|name| !services.iter().any(|call| call["Name"] == *name))
        .expect("unbounded names")
}

pub(crate) fn list_sources(
    solution: &Solution,
    mashup: &str,
) -> Result<Vec<SourceInfo>, AuthoringError> {
    let loaded = load_existing(solution, mashup)?;
    let document = parse_loaded(&loaded)?;
    let mut out = Vec::new();
    for (section_name, section) in document.value()["Data"].as_object().into_iter().flatten() {
        let entity_type = section["EntityType"].as_str().unwrap_or("");
        let kind = match entity_type {
            "Things" => "things",
            "ThingTemplates" => "template",
            "DynamicThingTemplates" => "dynamic-template",
            "DynamicThingShapes" => "dynamic-shape",
            _ => continue,
        };
        let entity = section_name
            .split_once('_')
            .map(|(_, x)| x)
            .unwrap_or_default()
            .to_string();
        let feed = document.value()["DataBindings"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|binding| entity_slot_binding(binding, section_name))
            .map(
                |binding| match binding["SourceArea"].as_str().unwrap_or("") {
                    "Mashup" => format!(
                        "mashup:{}",
                        binding["PropertyMaps"][0]["SourceProperty"]
                            .as_str()
                            .unwrap_or("")
                    ),
                    "Data" => format!("data:{}", binding["SourceId"].as_str().unwrap_or("")),
                    "UI" => format!("ui:{}", binding["SourceId"].as_str().unwrap_or("")),
                    _ => "UNBOUND".into(),
                },
            )
            .unwrap_or_else(|| {
                if dynamic_section(section) {
                    "UNBOUND".into()
                } else {
                    "fixed".into()
                }
            });
        for call in section["Services"].as_array().into_iter().flatten() {
            out.push(SourceInfo {
                kind: kind.into(),
                entity: entity.clone(),
                service: call["Target"].as_str().unwrap_or("").into(),
                alias: call["Name"].as_str().unwrap_or("").into(),
                entity_from: feed.clone(),
                refresh: call["RefreshInterval"].as_u64().unwrap_or(0),
                cache: call.get("Caching").is_some(),
            });
        }
    }
    Ok(out)
}

pub(crate) fn plan_source_remove(
    solution: &Solution,
    request: &SourceRemoveRequest,
) -> Result<SourceRemovePlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let Address::Data {
        entity_service,
        alias,
        entity_name: false,
        selected_rows: false,
        member: None,
    } = parse_address(&request.address).map_err(|e| AuthoringError::Invalid(e.to_string()))?
    else {
        return Err(AuthoringError::Invalid(
            "source remove takes a data call address, data:<Entity>.<Service>[as <alias>], not a field, selected rows or EntityName".into(),
        ));
    };
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let (entity, service) = super::resolve_service(&model, platform.as_ref(), &entity_service)?;
    // `Entity.Service.field` parses as one dotted name; what resolves must be all of it.
    if entity_service != format!("{}.{service}", entity.name) {
        return Err(AuthoringError::Invalid(
            "source remove takes a data call address, data:<Entity>.<Service>[as <alias>], not a field, selected rows or EntityName".into(),
        ));
    }
    let matches: Vec<(String, String)> = document.value()["Data"]
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(section, data)| {
            let service = &service;
            let alias = &alias;
            data["Services"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(move |call| {
                    call["Target"] == *service
                        && (data["EntityName"].as_str().unwrap_or("") == entity.name
                            || section.ends_with(&format!("_{}", entity.name)))
                        && alias.as_ref().is_none_or(|alias| call["Name"] == *alias)
                })
                .filter_map(move |call| {
                    call["Name"]
                        .as_str()
                        .map(|name| (section.clone(), name.into()))
                })
        })
        .collect();
    let (section, call) = match matches.as_slice() {
        [] => {
            return Err(AuthoringError::Unknown(format!(
                "{} is not a call this mashup has",
                request.address
            )))
        }
        [one] => one.clone(),
        _ => {
            return Err(AuthoringError::Ambiguous(format!(
                "{} has several calls; name one with [as <alias>]",
                request.address
            )))
        }
    };
    // The section's entity binding belongs to the section: it goes with the last call only.
    let last_call = document.value()["Data"][&section]["Services"]
        .as_array()
        .is_some_and(|calls| calls.len() == 1);
    let binding_indexes: Vec<usize> = document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, binding)| {
            (binding["SourceArea"] == "Data"
                && binding["SourceSection"] == section
                && binding["SourceId"] == call)
                || (binding["TargetArea"] == "Data"
                    && binding["TargetSection"] == section
                    && binding["TargetId"] == call)
                || (last_call && entity_slot_binding(binding, &section))
        })
        .map(|(index, _)| index)
        .collect();
    let event_indexes: Vec<usize> = document.value()["Events"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, event)| {
            (event["EventTriggerArea"] == "Data"
                && event["EventTriggerSection"] == section
                && event["EventTriggerId"] == call)
                || (event["EventHandlerArea"] == "Data"
                    && event["EventHandlerId"] == section
                    && event["EventHandlerService"] == call)
        })
        .map(|(index, _)| index)
        .collect();
    if !request.cascade && (!binding_indexes.is_empty() || !event_indexes.is_empty()) {
        return Err(AuthoringError::Refused(format!(
            "{} is named by {} binding(s) and {} event(s); use --cascade",
            request.address,
            binding_indexes.len(),
            event_indexes.len()
        )));
    }
    if request.cascade {
        for index in binding_indexes.iter().rev() {
            document.bindings_mut().remove(*index);
        }
        for index in event_indexes.iter().rev() {
            document.events_mut().remove(*index);
        }
    }
    let services = document.data_mut()[&section]["Services"]
        .as_array_mut()
        .expect("checked");
    services.retain(|source| source["Name"] != call);
    if services.is_empty() {
        document.data_mut().remove(&section);
    }
    Ok(SourceRemovePlan {
        edit: finish_edit(loaded, document, "mashup source remove")?,
        section,
        call,
        bindings_removed: binding_indexes.len(),
        events_removed: event_indexes.len(),
    })
}

/// Change the entity behind complete data-source sections, preserving their Composer ids,
/// calls and dynamic EntityName feeds.
pub(crate) fn plan_source_retarget(
    solution: &Solution,
    request: &SourceRetargetRequest,
) -> Result<SourceRetargetPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let from_entity = model
        .entities
        .iter()
        .find(|entity| entity.name == request.from)
        .ok_or_else(|| {
            AuthoringError::Unknown(format!("no entity {} in the solution", request.from))
        })?;
    let to_entity = model
        .entities
        .iter()
        .find(|entity| entity.name == request.to)
        .ok_or_else(|| {
            AuthoringError::Unknown(format!("no entity {} in the solution", request.to))
        })?;
    if from_entity.collection != to_entity.collection
        || !matches!(
            from_entity.collection.as_str(),
            "Things" | "ThingTemplates" | "ThingShapes"
        )
    {
        return Err(AuthoringError::Refused(format!(
            "{} is in {}; {} is in {}; retarget needs matching Thing, ThingTemplate or ThingShape collections",
            request.from, from_entity.collection, request.to, to_entity.collection
        )));
    }
    let candidates: Vec<(String, String, bool)> = document.value()["Data"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, section)| {
            let kind = section["EntityType"].as_str()?;
            let allowed = match from_entity.collection.as_str() {
                "Things" => kind == "Things",
                "ThingTemplates" => matches!(kind, "ThingTemplates" | "DynamicThingTemplates"),
                "ThingShapes" => kind == "DynamicThingShapes",
                _ => false,
            };
            if allowed && section["EntityName"] == request.from {
                Some((key.clone(), kind.to_string(), dynamic_section(section)))
            } else {
                None
            }
        })
        .collect();
    if candidates.is_empty() {
        return Err(AuthoringError::Unknown(format!(
            "{} has no data source for {}",
            request.mashup, request.from
        )));
    }
    let mut sections = Vec::new();
    for (old, kind, _) in &candidates {
        let new = format!("{kind}_{}", request.to);
        if document.value()["Data"].get(&new).is_some() {
            return Err(AuthoringError::Refused(format!(
                "{new} already exists; merging data sources is out of scope"
            )));
        }
        sections.push((old.clone(), new));
    }
    // Check every service and every value that this mashup supplies before changing any key.
    for (old, _, _) in &candidates {
        let section = &document.value()["Data"][old];
        for call in section["Services"].as_array().into_iter().flatten() {
            let service_name = call["Target"].as_str().unwrap_or("");
            let service = catalog::lookup_service(
                to_entity,
                &model.entities,
                platform.as_ref(),
                service_name,
            )
            .service
            .ok_or_else(|| {
                AuthoringError::Refused(format!(
                    "{} has no service {service_name}, which {} calls",
                    request.to, request.mashup
                ))
            })?;
            let mut used: Vec<String> = call["Parameters"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(name, _)| name.clone())
                .collect();
            for binding in document.value()["DataBindings"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if binding["TargetArea"] == "Data"
                    && binding["TargetSection"] == *old
                    && binding["TargetId"] == call["Name"]
                {
                    for map in binding["PropertyMaps"].as_array().into_iter().flatten() {
                        if let Some(name) = map["TargetProperty"].as_str() {
                            used.push(name.into());
                        }
                    }
                }
            }
            for parameter in used {
                let expected = service
                    .parameters
                    .iter()
                    .find(|p| p.name == parameter)
                    .ok_or_else(|| {
                        AuthoringError::Refused(format!(
                            "{}.{} is missing parameter {}",
                            request.to, service_name, parameter
                        ))
                    })?;
                for binding in document.value()["DataBindings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if binding["TargetArea"] == "Data"
                        && binding["TargetSection"] == *old
                        && binding["TargetId"] == call["Name"]
                    {
                        for map in binding["PropertyMaps"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|map| map["TargetProperty"] == parameter)
                        {
                            if let Some(actual) = map["TargetPropertyBaseType"]
                                .as_str()
                                .filter(|x| !x.is_empty())
                            {
                                if actual != expected.value.base_type {
                                    return Err(AuthoringError::Refused(format!(
                                        "{}.{} parameter {} is {}, not {}",
                                        request.to,
                                        service_name,
                                        parameter,
                                        expected.value.base_type,
                                        actual
                                    )));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut bindings_rewritten = 0;
    let mut events_rewritten = 0;
    let changes: std::collections::BTreeMap<_, _> = sections.iter().cloned().collect();
    for binding in document.bindings_mut() {
        let mut changed = false;
        for field in ["SourceSection", "TargetSection"] {
            if binding["SourceArea"] == "Data" && field == "SourceSection"
                || binding["TargetArea"] == "Data" && field == "TargetSection"
            {
                if let Some(next) = binding[field].as_str().and_then(|old| changes.get(old)) {
                    binding[field] = Value::String(next.clone());
                    changed = true;
                }
            }
        }
        if changed {
            bindings_rewritten += 1;
        }
    }
    for event in document.events_mut() {
        let mut changed = false;
        if event["EventHandlerArea"] == "Data" {
            if let Some(next) = event["EventHandlerId"]
                .as_str()
                .and_then(|old| changes.get(old))
            {
                event["EventHandlerId"] = Value::String(next.clone());
                changed = true;
            }
        }
        if event["EventTriggerArea"] == "Data" {
            if let Some(next) = event["EventTriggerSection"]
                .as_str()
                .and_then(|old| changes.get(old))
            {
                event["EventTriggerSection"] = Value::String(next.clone());
                changed = true;
            }
        }
        if changed {
            events_rewritten += 1;
        }
    }
    for (old, new) in &sections {
        let mut section = document.data_mut().remove(old).expect("identified above");
        section["DataName"] = Value::String(new.clone());
        section["EntityName"] = Value::String(request.to.clone());
        document.data_mut().insert(new.clone(), section);
    }
    Ok(SourceRetargetPlan {
        edit: finish_edit(loaded, document, "mashup source retarget")?,
        sections,
        bindings_rewritten,
        events_rewritten,
        dynamic: candidates.iter().any(|(_, _, dynamic)| *dynamic),
    })
}
