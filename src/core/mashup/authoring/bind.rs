//! `mashup bind`: a binding from one end to another, through a data call where one is needed.

use super::{
    allowed, area, base, details, finish_edit, insert, load_existing, parse_loaded, property,
    resolve_end, source_property_type, target_property_type, AuthoringContext, AuthoringError,
    EditPlan, End,
};
use crate::core::config::Solution;
use crate::core::mashup::address::parse_address;
use crate::core::mashup::describe::mashup_parameters;
use crate::core::mashup::edit::MashupDocument;
use crate::core::{bindings, mashup, types};
use serde_json::{Map, Value};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct BindRequest {
    pub mashup: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug)]
pub struct BindPlan {
    pub edit: EditPlan,
    pub from: String,
    pub to: String,
    pub new_source: Option<String>,
    /// Said, not refused: the target already has another source.
    pub warnings: Vec<String>,
    /// Contained-mashup caches repaired while resolving an endpoint.
    pub refreshed: Vec<String>,
}

/// Plan one Composer-format data binding.  Calls and sections are created only when an address
/// needs one; the entity content and managed sidecar then travel through the usual edit path.
pub(crate) fn plan_bind(
    solution: &Solution,
    request: &BindRequest,
) -> Result<BindPlan, AuthoringError> {
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
    let result = bind_in(
        &mut document,
        Some(solution),
        &context,
        &request.from,
        &request.to,
    )?;
    let edit = finish_edit(loaded, document, "mashup bind")?;
    Ok(BindPlan {
        edit,
        from: request.from.clone(),
        to: request.to.clone(),
        new_source: result.new_source,
        warnings: result.warnings,
        refreshed: result.refreshed,
    })
}

pub(super) struct BindIn {
    new_source: Option<String>,
    warnings: Vec<String>,
    refreshed: Vec<String>,
}

/// Add one checked binding to an already loaded document.
pub(super) fn bind_in(
    document: &mut MashupDocument,
    solution: Option<&Solution>,
    context: &AuthoringContext<'_>,
    from_raw: &str,
    to_raw: &str,
) -> Result<BindIn, AuthoringError> {
    let from_address =
        parse_address(from_raw).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let to_address = parse_address(to_raw).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let mut refreshed = Vec::new();
    let from = resolve_end(
        document,
        solution,
        context,
        from_address,
        true,
        &mut refreshed,
    )?;
    let to = resolve_end(
        document,
        solution,
        context,
        to_address,
        false,
        &mut refreshed,
    )?;
    if !allowed(&from, &to) {
        return Err(AuthoringError::Invalid(format!(
            "{} cannot bind to {}",
            from_raw, to_raw
        )));
    }
    let from_base = base(&from);
    let to_base = base(&to);
    if from_base != to_base && to_base != "STRING" {
        return Err(AuthoringError::Refused(format!(
            "{} is {} but {} is {}",
            from_raw, from_base, to_raw, to_base
        )));
    }
    let (source_section, source_id, source_added) = data_call(document, &from)?;
    let (target_section, target_id, target_added) = data_call(document, &to)?;
    if binding_exists(
        document,
        &from,
        &to,
        &source_section,
        &source_id,
        &target_section,
        &target_id,
    ) {
        return Err(AuthoringError::Exists(format!(
            "{} is already bound to {}",
            from_raw, to_raw
        )));
    }
    let warnings = if has_source(document, &to, &target_section, &target_id) {
        vec![format!(
            "{} already has another source; ThingWorx applies whichever value arrives last",
            to_raw
        )]
    } else {
        Vec::new()
    };
    let mut binding = Map::new();
    insert(
        &mut binding,
        "Id",
        Value::String(Uuid::new_v4().to_string()),
    );
    let mut map = Map::new();
    insert(
        &mut map,
        "SourceProperty",
        Value::String(property(&from).to_string()),
    );
    insert(
        &mut map,
        "SourcePropertyBaseType",
        Value::String(from_base.to_string()),
    );
    insert(
        &mut map,
        "SourcePropertyType",
        Value::String(source_property_type(&from).to_string()),
    );
    insert(
        &mut map,
        "TargetProperty",
        Value::String(property(&to).to_string()),
    );
    insert(
        &mut map,
        "TargetPropertyBaseType",
        Value::String(to_base.to_string()),
    );
    insert(
        &mut map,
        "TargetPropertyType",
        Value::String(target_property_type(&to).to_string()),
    );
    insert(
        &mut binding,
        "PropertyMaps",
        Value::Array(vec![Value::Object(map)]),
    );
    insert(
        &mut binding,
        "SourceArea",
        Value::String(area(&from).into()),
    );
    insert(
        &mut binding,
        "SourceDetails",
        Value::String(details(&from).into()),
    );
    insert(&mut binding, "SourceId", Value::String(source_id));
    insert(&mut binding, "SourceSection", Value::String(source_section));
    insert(&mut binding, "TargetArea", Value::String(area(&to).into()));
    insert(&mut binding, "TargetId", Value::String(target_id));
    insert(&mut binding, "TargetSection", Value::String(target_section));
    document.bindings_mut().push(Value::Object(binding));
    let new_source = source_added.or(target_added);
    Ok(BindIn {
        new_source,
        warnings,
        refreshed,
    })
}

pub(super) fn data_call(
    document: &mut MashupDocument,
    end: &End,
) -> Result<(String, String, Option<String>), AuthoringError> {
    let End::Data {
        entity,
        collection,
        service,
        requested_alias,
        ..
    } = end
    else {
        return Ok((
            String::new(),
            match end {
                End::Ui { id, .. } => id.clone(),
                End::Mashup { .. } => mashup::edit::ROOT_ID.into(),
                End::Data { .. } => unreachable!(),
            },
            None,
        ));
    };
    if let Some((section, call)) = existing_data_call(document, end)? {
        return Ok((section, call, None));
    }
    if collection != "Things" {
        return Err(AuthoringError::Refused(format!("{entity} is a template/shape: its data source needs the entity it runs on bound first (twaco does not create that yet); add the call in Composer once")));
    }
    let key = format!("Things_{entity}");
    let data = document.data_mut();
    if !data.contains_key(&key) {
        let mut value = Map::new();
        insert(&mut value, "DataName", Value::String(key.clone()));
        insert(&mut value, "EntityName", Value::String(entity.clone()));
        insert(&mut value, "EntityType", Value::String("Things".into()));
        insert(&mut value, "Id", Value::String(Uuid::new_v4().to_string()));
        insert(&mut value, "RefreshInterval", Value::from(0));
        insert(&mut value, "Services", Value::Array(Vec::new()));
        insert(data, &key, Value::Object(value));
    }
    let section = &mut data[&key];
    let services = section["Services"]
        .as_array_mut()
        .ok_or_else(|| AuthoringError::Refused(format!("{key}.Services is not a list")))?;
    if let Some(alias) = requested_alias {
        if let Some(other) = services.iter().find(|call| call["Name"] == *alias) {
            return Err(AuthoringError::Exists(format!(
                "{entity} already has a call named {alias}, for {}",
                other["Target"].as_str().unwrap_or("another service")
            )));
        }
    }
    let alias = requested_alias.clone().unwrap_or_else(|| {
        let mut n = 1;
        loop {
            let name = if n == 1 {
                service.clone()
            } else {
                format!("{service}_{n}")
            };
            if !services.iter().any(|call| call["Name"] == name) {
                break name;
            }
            n += 1;
        }
    });
    let mut call = Map::new();
    insert(&mut call, "APIMethod", Value::String("post".into()));
    insert(
        &mut call,
        "Characteristic",
        Value::String("Services".into()),
    );
    insert(&mut call, "Id", Value::String(Uuid::new_v4().to_string()));
    insert(&mut call, "Name", Value::String(alias.clone()));
    insert(&mut call, "Parameters", Value::Object(Map::new()));
    insert(&mut call, "RefreshInterval", Value::from(0));
    insert(&mut call, "Target", Value::String(service.clone()));
    services.push(Value::Object(call));
    Ok((key, alias, Some(format!("{entity}.{service}"))))
}

/// The mashup's call of `end`'s service, as (section, alias). Several calls of the service and no
/// `[as alias]` to choose between them is ambiguous.
pub(super) fn existing_data_call(
    document: &MashupDocument,
    end: &End,
) -> Result<Option<(String, String)>, AuthoringError> {
    let End::Data {
        entity,
        service,
        requested_alias,
        ..
    } = end
    else {
        return Ok(None);
    };
    let calls: Vec<(String, String)> = bindings::data_sections(document.value())
        .into_iter()
        .filter(|(_, details)| details.entity == *entity)
        .flat_map(|(section, details)| {
            details
                .services
                .iter()
                .filter(|call| {
                    call.target == *service
                        && requested_alias
                            .as_ref()
                            .is_none_or(|alias| call.instance == *alias)
                })
                .map(|call| (section.clone(), call.instance.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    match calls.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only.clone())),
        many => Err(AuthoringError::Ambiguous(format!(
            "{entity}.{service} has several calls in this mashup ({}); name one with [as <alias>]",
            many.iter()
                .map(|(_, alias)| alias.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn binding_exists(
    document: &MashupDocument,
    from: &End,
    to: &End,
    source_section: &str,
    source_id: &str,
    target_section: &str,
    target_id: &str,
) -> bool {
    document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|binding| {
            binding["SourceArea"] == area(from)
                && binding["SourceSection"] == source_section
                && binding["SourceId"] == source_id
                && binding["SourceDetails"] == details(from)
                && binding["TargetArea"] == area(to)
                && binding["TargetSection"] == target_section
                && binding["TargetId"] == target_id
                && binding["PropertyMaps"].as_array().is_some_and(|maps| {
                    maps.iter().any(|map| {
                        map["SourceProperty"] == property(from)
                            && map["TargetProperty"] == property(to)
                    })
                })
        })
}

/// Whether a binding already sets `to`.
fn has_source(document: &MashupDocument, to: &End, target_section: &str, target_id: &str) -> bool {
    document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|binding| {
            binding["TargetArea"] == area(to)
                && binding["TargetSection"] == target_section
                && binding["TargetId"] == target_id
                && binding["PropertyMaps"].as_array().is_some_and(|maps| {
                    maps.iter().any(|map| map["TargetProperty"] == property(to))
                })
        })
}
