//! `mashup collection add`: a cell mashup repeated for every row of a service result.

use super::{
    base, bind_in, existing_data_call, finish_edit, load_existing, parse_loaded, plan_add,
    AuthoringContext, AuthoringError, EditPlan,
};
use crate::core::bindings;
use crate::core::config::Solution;
use crate::core::mashup::address::{parse_address, Address};
use crate::core::mashup::describe::mashup_parameters;
use crate::core::mashup::edit::Placement;
use crate::core::{mashup, types};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct CollectionAddRequest {
    pub mashup: String,
    pub cell: String,
    pub data: String,
    pub mappings: Vec<(String, String)>,
    pub uid: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub placement: Option<Placement>,
}

#[derive(Debug)]
pub struct CollectionAddPlan {
    pub edit: EditPlan,
    pub id: String,
    pub cell: String,
    pub mappings: Vec<(String, String)>,
    pub uid: Option<String>,
    pub binding: String,
    pub notes: Vec<String>,
}

/// Plan a collection insertion and its whole-result binding as one edit.
pub(crate) fn plan_collection_add(
    solution: &Solution,
    request: &CollectionAddRequest,
) -> Result<CollectionAddPlan, AuthoringError> {
    if request.mashup == request.cell {
        return Err(AuthoringError::Refused(
            "a mashup cannot use itself as a collection cell".into(),
        ));
    }
    if request.mappings.is_empty() {
        return Err(AuthoringError::Invalid(
            "give at least one --map <row-field>=<cell-parameter>".into(),
        ));
    }
    if request.width == Some(0) || request.height == Some(0) {
        return Err(AuthoringError::Invalid(
            "--width and --height must be positive integers".into(),
        ));
    }
    let mut fields = BTreeSet::new();
    let mut parameters = BTreeSet::new();
    for (field, parameter) in &request.mappings {
        if field.is_empty() || parameter.is_empty() {
            return Err(AuthoringError::Invalid(
                "--map must be row-field=cell-parameter".into(),
            ));
        }
        if !fields.insert(field) {
            return Err(AuthoringError::Refused(format!(
                "row field {field} is mapped twice"
            )));
        }
        if !parameters.insert(parameter) {
            return Err(AuthoringError::Refused(format!(
                "cell parameter {parameter} is mapped twice"
            )));
        }
    }
    let cell = load_existing(solution, &request.cell)?;
    let cell_parameters =
        mashup_parameters(&cell.entity_before).map_err(AuthoringError::Unreadable)?;
    for (_, parameter) in &request.mappings {
        let found = cell_parameters
            .iter()
            .find(|found| found.name == *parameter)
            .ok_or_else(|| {
                AuthoringError::Unknown(format!("{} has no parameter {parameter}", request.cell))
            })?;
        if found.direction == "OUT" {
            return Err(AuthoringError::Refused(format!(
                "{parameter} is an output of {}; a collection supplies an input",
                request.cell
            )));
        }
    }
    let parent = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&parent)?;
    let address =
        parse_address(&request.data).map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    let Address::Data {
        selected_rows: false,
        entity_name: false,
        member: None,
        ..
    } = address
    else {
        return Err(AuthoringError::Invalid(
            "--data names a whole service result".into(),
        ));
    };
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let parent_parameters =
        mashup_parameters(&parent.entity_before).map_err(AuthoringError::Unreadable)?;
    let context = AuthoringContext {
        parameters: &parent_parameters,
        model: &model,
        platform: platform.as_ref(),
    };
    let source = super::resolve_end(
        &mut document,
        Some(solution),
        &context,
        address,
        true,
        &mut Vec::new(),
    )?;
    if base(&source) != "INFOTABLE" {
        return Err(AuthoringError::Refused(
            "--data must return an INFOTABLE".into(),
        ));
    }
    let (section, call) = existing_data_call(&document, &source)?.ok_or_else(|| {
        AuthoringError::Refused(format!(
            "{} is not a call in {}; add it with `twaco mashup source add` first",
            request.data, request.mashup
        ))
    })?;
    let resolver = bindings::Resolver::new(&model, platform.as_ref());
    let sections = bindings::data_sections(document.value());
    let shape_name =
        bindings::service_at(&sections, &section, &call).and_then(|(section, found)| {
            // `call` is the call's name in the mashup; an aliased call names a different service.
            resolver
                .service(section, &found.target)
                .service
                .and_then(|service| {
                    service
                        .result
                        .base_type
                        .eq_ignore_ascii_case("INFOTABLE")
                        .then(|| service.result.data_shape.clone())
                        .flatten()
                })
        });
    if let Some(shape) = shape_name
        .as_deref()
        .and_then(|name| resolver.checkable_shape(name))
    {
        for (field, parameter) in &request.mappings {
            let found = shape
                .fields
                .iter()
                .find(|known| known.name == *field)
                .ok_or_else(|| {
                    AuthoringError::Refused(format!(
                        "{} returns {}, which has no field {field}",
                        request.data, shape.name
                    ))
                })?;
            let expected = cell_parameters
                .iter()
                .find(|known| known.name == *parameter)
                .expect("checked above");
            if !super::bind::compatible(&found.base_type, &expected.base_type) {
                return Err(AuthoringError::Refused(format!(
                    "field {field} is {}, not {} for {}",
                    found.base_type, expected.base_type, parameter
                )));
            }
        }
        if let Some(uid) = &request.uid {
            if !shape.fields.iter().any(|known| known.name == *uid) {
                return Err(AuthoringError::Refused(format!(
                    "{} returns {}, which has no field {uid}",
                    request.data, shape.name
                )));
            }
        }
    }
    let mut property_binding = Map::new();
    for (field, parameter) in &request.mappings {
        property_binding.insert(field.clone(), Value::String(parameter.clone()));
    }
    let mut properties = vec![
        ("Mashup".into(), request.cell.clone()),
        (
            "MashupPropertyBinding".into(),
            Value::Object(property_binding).to_string(),
        ),
    ];
    if let Some(uid) = &request.uid {
        properties.push(("UIDField".into(), uid.clone()));
    }
    if let Some(width) = request.width {
        properties.push(("CellWidth".into(), width.to_string()));
    }
    if let Some(height) = request.height {
        properties.push(("CellHeight".into(), height.to_string()));
    }
    // `plan_add` owns catalogue defaults, id allocation and responsive placement. Re-open its
    // pure result so the binding joins the same final sync and transaction.
    let added = plan_add(
        solution,
        &super::AddRequest {
            mashup: request.mashup.clone(),
            widget: "ptcscollection".into(),
            inputs: Vec::new(),
            placement: request.placement.clone(),
            label: None,
            properties,
        },
    )?;
    let assets = mashup::extract(&added.edit.entity_after).map_err(AuthoringError::Mashup)?;
    document = mashup::edit::parse(&assets.content).map_err(AuthoringError::Edit)?;
    let target = format!("ui:{}.Data", added.placed.id);
    bind_in(
        &mut document,
        Some(solution),
        &context,
        &request.data,
        &target,
    )?;
    let has_event = document.value()["Events"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|event| {
            event["EventHandlerArea"] == "Data"
                && event["EventHandlerId"] == section
                && event["EventHandlerService"] == call
        });
    let notes = (!has_event)
        .then(|| {
            format!(
                "nothing runs {call}; add `twaco mashup on {} --event mashup:Loaded --invoke {}`",
                request.mashup, request.data
            )
        })
        .into_iter()
        .collect();
    Ok(CollectionAddPlan {
        edit: finish_edit(parent, document, "mashup collection add")?,
        id: added.placed.id,
        cell: request.cell.clone(),
        mappings: request.mappings.clone(),
        uid: request.uid.clone(),
        binding: format!("{} -> {target}", request.data),
        notes,
    })
}

pub fn collection_add_plan_json(
    solution: &Solution,
    plan: &CollectionAddPlan,
    applied: bool,
) -> Value {
    let mut files = vec![solution.relative(&plan.edit.entity_path)];
    if let Some(sidecars) = &plan.edit.sidecars {
        files.push(solution.relative(&sidecars.content_path));
    }
    json!({"applied": applied, "mashup": plan.edit.mashup, "id": plan.id, "cell": plan.cell,
        "mappings": plan.mappings.iter().map(|(field, parameter)| json!({"field":field,"parameter":parameter})).collect::<Vec<_>>(),
        "uid": plan.uid, "binding": plan.binding, "notes": plan.notes, "files": files})
}
