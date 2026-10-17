//! The ends of a binding or an event: an address resolved to a widget, function, service,
//! contained mashup or navigation, with what each end can carry.

use super::{
    contained_parameters, ensure_cached, function_property, load_existing, parse_loaded,
    AuthoringContext, AuthoringError,
};
use crate::core::config::Solution;
use crate::core::mashup::address::{parse_address, Address};
use crate::core::mashup::edit::MashupDocument;
use crate::core::{catalog, mashup, types, widgets};

#[derive(Clone)]
pub(super) enum End {
    Ui {
        id: String,
        property: String,
        base: String,
    },
    Mashup {
        parameter: String,
        base: String,
    },
    Data {
        entity: String,
        collection: String,
        service: String,
        requested_alias: Option<String>,
        selected_rows: bool,
        member: Option<String>,
        base: String,
        property_type: String,
    },
}

pub(super) fn resolve_end(
    document: &mut MashupDocument,
    solution: Option<&Solution>,
    context: &AuthoringContext<'_>,
    address: Address,
    source: bool,
    refreshed: &mut Vec<String>,
) -> Result<End, AuthoringError> {
    let (parameters, model, platform) = (context.parameters, context.model, context.platform);
    match address {
        Address::Ui { widget, property } => {
            let found = document.find_exact(&widget).map_err(AuthoringError::Edit)?;
            if found.type_id == "mashupcontainer" {
                return resolve_container_end(
                    document, solution, found, property, source, refreshed,
                );
            }
            if found.type_id == "navigationfunction" {
                if let Some(end) = resolve_navigation_end(
                    document, solution, &found, &property, source, refreshed,
                )? {
                    return Ok(end);
                }
            }
            let catalog = widgets::catalog().by_type(&found.type_id).filter(|w| w.has_contract()).ok_or_else(|| AuthoringError::Refused(format!("widget `{}` has type `{}`, whose contract is not in the embedded ThingWorx 10.2 catalogue; twaco will not create a binding for it", found.id, found.type_id)))?;
            if catalog.function {
                if let Some((can_source, can_target, base)) =
                    function_property(document.at(&found.path), &found.type_id, &property)
                {
                    if !(if source { can_source } else { can_target }) {
                        return Err(AuthoringError::Refused(format!(
                            "ui:{}.{property} is not a {} property",
                            found.id,
                            if source { "source" } else { "target" }
                        )));
                    }
                    return Ok(End::Ui {
                        id: found.id,
                        property,
                        base,
                    });
                }
                // An expression's inputs and an events router's are only those it declares.
                if found.type_id == "eventsrouter" && property.starts_with("Input")
                    || matches!(
                        found.type_id.as_str(),
                        "expression" | "expression2" | "validator" | "validator2"
                    ) && catalog
                        .properties
                        .as_ref()
                        .is_some_and(|own| !own.contains_key(&property))
                {
                    return Err(AuthoringError::Unknown(format!(
                        "{} has no input {property}; {}",
                        found.id,
                        if found.type_id == "eventsrouter" {
                            "raise its NumberOfInputs to use more"
                        } else {
                            "declare it with mashup add --input when adding the widget"
                        }
                    )));
                }
            }
            let contract = catalog.property(&property).ok_or_else(|| {
                AuthoringError::Unknown(format!("{} has no property {property}", found.id))
            })?;
            let valid = if source {
                contract.source
            } else {
                contract.target
            };
            if !valid {
                return Err(AuthoringError::Refused(format!(
                    "ui:{}.{property} is not a {} property",
                    found.id,
                    if source { "source" } else { "target" }
                )));
            }
            Ok(End::Ui {
                id: found.id,
                property,
                base: contract.base_type.clone().unwrap_or_default(),
            })
        }
        Address::Mashup { parameter } => {
            let found = parameters
                .iter()
                .find(|p| p.name == parameter)
                .ok_or_else(|| {
                    AuthoringError::Unknown(format!("mashup has no parameter {parameter}"))
                })?;
            Ok(End::Mashup {
                parameter,
                base: found.base_type.clone(),
            })
        }
        Address::Data {
            entity_service,
            alias,
            selected_rows,
            member,
        } => {
            if !source && selected_rows {
                return Err(AuthoringError::Invalid(
                    "SelectedRows is a data source, not a parameter target".into(),
                ));
            }
            let (entity, service) = resolve_service(model, platform, &entity_service)?;
            let found = catalog::lookup_service(entity, &model.entities, platform, &service)
                .service
                .expect("resolved above");
            let member = member.or_else(|| {
                entity_service
                    .strip_prefix(&format!("{}.{}.", entity.name, service))
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
            });
            if source {
                let (base, property_type) = match member.as_deref() {
                    None => (found.result.base_type.clone(), "InfoTable".to_string()),
                    Some(name) => {
                        // A result field may share a parameter's name; the field is the source.
                        let shape = found.result.data_shape.as_deref().map(|shape_name| {
                            (
                                shape_name,
                                model
                                    .data_shapes
                                    .iter()
                                    .find(|shape| shape.name == shape_name),
                            )
                        });
                        let field = shape
                            .and_then(|(_, shape)| shape)
                            .and_then(|shape| shape.fields.iter().find(|field| field.name == name));
                        match (field, shape) {
                            (Some(field), _) => (field.base_type.clone(), "Field".to_string()),
                            _ if found.parameters.iter().any(|p| p.name == name) => {
                                return Err(AuthoringError::Invalid(format!(
                                    "data:{entity_service}.{name} is a parameter, not a data source"
                                )));
                            }
                            (None, None) => {
                                return Err(AuthoringError::Refused(format!(
                                    "{service} has no result DataShape"
                                )));
                            }
                            (None, Some((shape_name, None))) => {
                                return Err(AuthoringError::Refused(format!(
                                    "result DataShape {shape_name} is not in the repository"
                                )));
                            }
                            (None, Some((shape_name, Some(_)))) => {
                                return Err(AuthoringError::Refused(format!(
                                    "result DataShape {shape_name} has no field {name}"
                                )));
                            }
                        }
                    }
                };
                Ok(End::Data {
                    entity: entity.name.clone(),
                    collection: entity.collection.clone(),
                    service,
                    requested_alias: alias,
                    selected_rows,
                    member,
                    base,
                    property_type,
                })
            } else {
                let name = member.ok_or_else(|| {
                    AuthoringError::Invalid(format!(
                        "data:{entity_service} needs a parameter target"
                    ))
                })?;
                let parameter = found
                    .parameters
                    .iter()
                    .find(|p| p.name == name)
                    .ok_or_else(|| {
                        AuthoringError::Unknown(format!(
                            "{}.{} has no parameter {}",
                            entity.name, service, name
                        ))
                    })?;
                Ok(End::Data {
                    entity: entity.name.clone(),
                    collection: entity.collection.clone(),
                    service,
                    requested_alias: alias,
                    selected_rows,
                    member: Some(name),
                    base: parameter.value.base_type.clone(),
                    property_type: "Parameter".into(),
                })
            }
        }
    }
}

fn resolve_container_end(
    document: &mut MashupDocument,
    solution: Option<&Solution>,
    found: mashup::edit::WidgetRef,
    property: String,
    source: bool,
    refreshed: &mut Vec<String>,
) -> Result<End, AuthoringError> {
    let child = document.at(&found.path)["Properties"]["Name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            AuthoringError::Refused(format!(
                "contained mashup `{}` has no child mashup",
                found.id
            ))
        })?
        .to_string();
    let solution = solution.ok_or_else(|| {
        AuthoringError::Refused(format!(
            "contained mashup `{}` cannot be resolved without the repository",
            found.id
        ))
    })?;
    let loaded = load_existing(solution, &child).map_err(|error| match error {
        AuthoringError::Unknown(_) => AuthoringError::Refused(format!(
            "contained mashup `{}` shows {child}, which is not in the repository",
            found.id
        )),
        other => other,
    })?;
    let child_document = parse_loaded(&loaded)?;
    let definitions = contained_parameters(&loaded, &child_document)?;
    if property == "Name" {
        if source {
            return Err(AuthoringError::Refused(format!(
                "ui:{}.Name is a target-only MASHUPNAME property",
                found.id
            )));
        }
        return Ok(End::Ui {
            id: found.id,
            property,
            base: "MASHUPNAME".into(),
        });
    }
    let definition = definitions.iter().find(|definition| definition.name == property && definition.visible)
        .ok_or_else(|| AuthoringError::Unknown(format!(
            "contained mashup `{}` has no bindable parameter {property}; bindable parameters are {}",
            found.id,
            definitions.iter().filter(|definition| definition.visible).map(|definition| definition.name.as_str()).collect::<Vec<_>>().join(", ")
        )))?;
    if source && definition.direction == "IN" {
        return Err(AuthoringError::Refused(format!(
            "{property} is an input of {child}"
        )));
    }
    if !source && definition.direction == "OUT" {
        return Err(AuthoringError::Refused(format!(
            "{property} is an output of {child}"
        )));
    }
    ensure_cached(
        document,
        &found,
        &child,
        &definitions,
        definition,
        refreshed,
    );
    Ok(End::Ui {
        id: found.id,
        property,
        base: definition.base_type.clone(),
    })
}

/// A navigation function's property that is a parameter of the mashup it opens, when that mashup
/// is in the repository: resolved from the mashup's own definition, its copies refreshed. `None`
/// for the function's own properties and for a target outside the repository.
fn resolve_navigation_end(
    document: &mut MashupDocument,
    solution: Option<&Solution>,
    found: &mashup::edit::WidgetRef,
    property: &str,
    source: bool,
    refreshed: &mut Vec<String>,
) -> Result<Option<End>, AuthoringError> {
    let target = document.at(&found.path)["Properties"]["TargetMashup"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (Some(solution), false) = (solution, target.is_empty()) else {
        return Ok(None);
    };
    let Ok(loaded) = load_existing(solution, &target) else {
        return Ok(None);
    };
    let child_document = parse_loaded(&loaded)?;
    let definitions = contained_parameters(&loaded, &child_document)?;
    let Some(definition) = definitions
        .iter()
        .find(|definition| definition.name == property && definition.visible)
    else {
        return Ok(None);
    };
    if source && definition.direction == "IN" {
        return Err(AuthoringError::Refused(format!(
            "{property} is an input of {target}"
        )));
    }
    if !source && definition.direction == "OUT" {
        return Err(AuthoringError::Refused(format!(
            "{property} is an output of {target}"
        )));
    }
    ensure_cached(
        document,
        found,
        &target,
        &definitions,
        definition,
        refreshed,
    );
    Ok(Some(End::Ui {
        id: found.id.clone(),
        property: property.to_string(),
        base: definition.base_type.clone(),
    }))
}

pub(super) fn resolve_service<'a>(
    model: &'a types::Model,
    platform: Option<&types::Platform>,
    raw: &str,
) -> Result<(&'a types::Entity, String), AuthoringError> {
    let mut matches = Vec::new();
    for entity in &model.entities {
        if !matches!(
            entity.collection.as_str(),
            "Things" | "ThingTemplates" | "ThingShapes"
        ) {
            continue;
        }
        let Some(rest) = raw.strip_prefix(&(entity.name.clone() + ".")) else {
            continue;
        };
        let mut ends = vec![rest];
        ends.extend(rest.match_indices('.').map(|(at, _)| &rest[..at]));
        for service in ends {
            if catalog::lookup_service(entity, &model.entities, platform, service)
                .service
                .is_some()
            {
                matches.push((entity, service.to_string()));
            }
        }
    }
    matches.sort_by(|a, b| (&a.0.name, &a.1).cmp(&(&b.0.name, &b.1)));
    matches.dedup_by(|a, b| a.0.name == b.0.name && a.1 == b.1);
    match matches.as_slice() {
        [] => Err(AuthoringError::Unknown(format!(
            "no entity and service in the solution match data:{raw}"
        ))),
        [only] => Ok((only.0, only.1.clone())),
        _ => Err(AuthoringError::Ambiguous(format!(
            "data:{raw} is ambiguous; name an entity and service that resolve once"
        ))),
    }
}

pub(super) fn base(end: &End) -> &str {
    match end {
        End::Ui { base, .. } | End::Mashup { base, .. } | End::Data { base, .. } => base,
    }
}

/// Sidebar menus receive a complete service result, never a parameter or selected rows.
pub(super) fn menu_service(
    document: &mut MashupDocument,
    context: &AuthoringContext<'_>,
    raw: &str,
) -> Result<(), AuthoringError> {
    let address = parse_address(raw).map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    let Address::Data {
        selected_rows,
        member,
        ..
    } = &address
    else {
        return Err(AuthoringError::Invalid(
            "a menu address names a data service result".into(),
        ));
    };
    if *selected_rows || member.is_some() {
        return Err(AuthoringError::Invalid(
            "a menu address names a service result, not a parameter or SelectedRows".into(),
        ));
    }
    let end = resolve_end(document, None, context, address, true, &mut Vec::new())?;
    if base(&end) != "INFOTABLE" {
        return Err(AuthoringError::Refused(
            "the menu service must return an INFOTABLE".into(),
        ));
    }
    Ok(())
}

pub(super) fn property(end: &End) -> &str {
    match end {
        End::Ui { property, .. } => property,
        End::Mashup { parameter, .. } => parameter,
        End::Data { member, .. } => member.as_deref().unwrap_or(""),
    }
}

pub(super) fn area(end: &End) -> &'static str {
    match end {
        End::Ui { .. } => "UI",
        End::Mashup { .. } => "Mashup",
        End::Data { .. } => "Data",
    }
}

pub(super) fn details(end: &End) -> &'static str {
    match end {
        End::Data {
            selected_rows: true,
            ..
        } => "SelectedRows",
        End::Data { .. } => "AllData",
        _ => "",
    }
}

pub(super) fn source_property_type(end: &End) -> &str {
    match end {
        End::Ui { .. } | End::Mashup { .. } => "Property",
        End::Data { property_type, .. } => property_type,
    }
}

pub(super) fn target_property_type(end: &End) -> &str {
    match end {
        End::Ui { .. } | End::Mashup { .. } => "property",
        End::Data { .. } => "Parameter",
    }
}

pub(super) fn allowed(from: &End, to: &End) -> bool {
    matches!(
        (from, to),
        (
            End::Ui { .. },
            End::Ui { .. } | End::Mashup { .. } | End::Data { .. }
        ) | (End::Mashup { .. }, End::Ui { .. } | End::Data { .. })
            | (End::Data { .. }, End::Ui { .. } | End::Data { .. })
    )
}
