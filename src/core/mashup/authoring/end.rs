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
use serde_json::Value;

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
        entity_name: bool,
        member: Option<String>,
        base: String,
        property_type: String,
        warning: Option<String>,
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
    if let Address::Session {
        kind,
        property,
        base,
    } = &address
    {
        if !source {
            return Err(AuthoringError::Invalid(
                "session data is a binding source, not a target".into(),
            ));
        }
        return Ok(End::Data {
            entity: kind.clone(),
            collection: kind.clone(),
            service: String::new(),
            requested_alias: None,
            selected_rows: false,
            entity_name: false,
            member: Some(property.clone()),
            base: base.clone(),
            property_type: "Field".into(),
            warning: None,
        });
    }
    match address {
        Address::Session { .. } => unreachable!("handled above"),
        Address::Ui { widget, property } => {
            let found = document.find_exact(&widget).map_err(AuthoringError::Edit)?;
            if found.type_id == "mashupcontainer"
                && (property == "Name"
                    || widgets::catalog()
                        .by_type(&found.type_id)
                        .and_then(|widget| widget.property(&property))
                        .is_none())
            {
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
            if document.verbatim()
                && !widgets::catalog()
                    .by_type(&found.type_id)
                    .is_some_and(|w| w.has_contract())
            {
                return Ok(End::Ui {
                    id: found.id,
                    property,
                    base: String::new(),
                });
            }
            let catalog = widgets::catalog().by_type(&found.type_id).filter(|w| w.has_contract()).ok_or_else(|| AuthoringError::Refused(format!("widget `{}` has type `{}`, whose contract is not in the embedded ThingWorx 10.2 catalogue; twaco will not create a binding for it", found.id, found.type_id)))?;
            if let Some(contract) = widgets::dynamic_property(
                &found.type_id,
                &document.at(&found.path)["Properties"],
                &property,
                widgets::property_bound(document.value(), &found.id, "Data"),
            ) {
                if !(if source {
                    contract.source
                } else {
                    contract.target
                }) {
                    return Err(AuthoringError::Refused(format!(
                        "ui:{}.{property} is not a {} property",
                        found.id,
                        if source { "source" } else { "target" }
                    )));
                }
                return Ok(End::Ui {
                    id: found.id,
                    property,
                    base: contract.base_type.unwrap_or_default(),
                });
            }
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
            let Some(contract) = catalog.property(&property) else {
                // A property with a default but no contract of its own (a chart's `HideYAxis1`, a
                // tab's `Tab2Visible`): bindable on a page built as found, typed by its default.
                if let Some(default) = catalog
                    .defaults
                    .get(&property)
                    .cloned()
                    .or_else(|| catalog.numbered_default(&property))
                    .filter(|_| document.verbatim())
                {
                    let base = match default {
                        Value::Bool(_) => "BOOLEAN",
                        Value::Number(_) => "NUMBER",
                        _ => "STRING",
                    };
                    return Ok(End::Ui {
                        id: found.id,
                        property,
                        base: base.into(),
                    });
                }
                // A state formatter's thresholds are properties the widget declares itself, in a
                // parameter definition the catalogue does not list.
                if let Some(base) = declared_parameter(document.at(&found.path), catalog, &property)
                {
                    return Ok(End::Ui {
                        id: found.id,
                        property,
                        base,
                    });
                }
                return Err(AuthoringError::Unknown(
                    catalog.no_property(&found.id, &property),
                ));
            };
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
            entity_name,
            member,
        } => {
            if entity_name {
                if source {
                    return Err(AuthoringError::Invalid(
                        "EntityName is a data target, not a source".into(),
                    ));
                }
                let entity = model
                    .entities
                    .iter()
                    .find(|entity| entity.name == entity_service)
                    .ok_or_else(|| {
                        AuthoringError::Unknown(format!(
                            "no entity {} in the solution",
                            crate::core::workspace::quoted_selector(&entity_service)
                        ))
                    })?;
                return Ok(End::Data {
                    entity: entity.name.clone(),
                    collection: entity.collection.clone(),
                    service: String::new(),
                    requested_alias: alias,
                    selected_rows: false,
                    entity_name: true,
                    member: Some("EntityName".into()),
                    base: "THINGNAME".into(),
                    property_type: "Entity".into(),
                    warning: None,
                });
            }
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
                let mut warning = None;
                let (base, property_type) = match member.as_deref() {
                    None if found.result.base_type != "INFOTABLE" => {
                        let alias = alias
                            .as_ref()
                            .map(|alias| format!("[as {alias}]"))
                            .unwrap_or_default();
                        let source = format!("data:{}.{}{alias}", entity.name, service);
                        return Err(AuthoringError::Refused(format!(
                            "{source} is a whole scalar result; use `{source}.result`"
                        )));
                    }
                    None => (found.result.base_type.clone(), "InfoTable".to_string()),
                    Some(name) => {
                        let (base, unchecked) = result_field(
                            context,
                            solution,
                            &found,
                            &service,
                            &entity_service,
                            name,
                        )?;
                        warning = unchecked;
                        (base, "Field".into())
                    }
                };
                if found.result.base_type == "INFOTABLE"
                    && found.result.data_shape.is_none()
                    && warning.is_none()
                {
                    warning = Some(format!(
                        "{service} has no result DataShape; whole INFOTABLE bindings are supported"
                    ));
                }
                Ok(End::Data {
                    entity: entity.name.clone(),
                    collection: entity.collection.clone(),
                    service,
                    requested_alias: alias,
                    selected_rows,
                    entity_name: false,
                    member,
                    base,
                    property_type,
                    warning,
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
                    entity_name: false,
                    member: Some(name),
                    base: parameter.value.base_type.clone(),
                    property_type: "Parameter".into(),
                    warning: None,
                })
            }
        }
    }
}

/// A known result field is checked against its local, cached or platform contract. Missing
/// metadata is carried only for an unshaped INFOTABLE or a declared server-provided shape.
fn result_field(
    context: &AuthoringContext<'_>,
    solution: Option<&Solution>,
    found: &types::Service,
    service: &str,
    address: &str,
    name: &str,
) -> Result<(String, Option<String>), AuthoringError> {
    let parameter_error = || {
        AuthoringError::Invalid(format!(
            "data:{address}.{name} is a parameter, not a data source"
        ))
    };
    let Some(shape_name) = found.result.data_shape.as_deref() else {
        if found.result.base_type == "INFOTABLE" {
            return Ok((
                String::new(),
                Some(format!(
                    "{service} has no result DataShape; field {name} cannot be type-checked"
                )),
            ));
        }
        if name == "result" {
            return Ok((found.result.base_type.clone(), None));
        }
        if found
            .parameters
            .iter()
            .any(|parameter| parameter.name == name)
        {
            return Err(parameter_error());
        }
        return Err(AuthoringError::Refused(format!(
            "{service} has no result field {name}; use result for a scalar result"
        )));
    };
    let missing =
        || AuthoringError::Refused(format!("result DataShape {shape_name} has no field {name}"));
    // A result field can share a service parameter's name. The result contract wins.
    if let Some(shape) = context
        .model
        .data_shapes
        .iter()
        .find(|shape| shape.name == shape_name)
    {
        if let Some(field) = shape.fields.iter().find(|field| field.name == name) {
            return Ok((field.base_type.clone(), None));
        }
        return Err(
            if found
                .parameters
                .iter()
                .any(|parameter| parameter.name == name)
            {
                parameter_error()
            } else {
                missing()
            },
        );
    }
    if let Some(fields) = context
        .platform
        .and_then(|platform| platform.data_shapes.get(shape_name))
    {
        return fields
            .get(name)
            .map(|field| (field.base_type.clone(), None))
            .ok_or_else(missing);
    }
    if shape_name == "MenuEntry" {
        return menu_entry_field(name)
            .map(|base| (base.into(), None))
            .ok_or_else(missing);
    }
    let declared = solution
        .and_then(|solution| crate::core::requirements::load(solution).ok())
        .is_some_and(|contract| {
            contract
                .entities
                .get("DataShapes")
                .is_some_and(|shapes| shapes.iter().any(|shape| shape == shape_name))
        });
    if declared {
        return Ok((String::new(), Some(format!("result DataShape {shape_name} is declared server-provided; field {name} cannot be type-checked offline"))));
    }
    Err(AuthoringError::Refused(format!("result DataShape {shape_name} is not in the repository; add it or declare it in requirements.toml")))
}

/// The system menu shape ships with both supported Composer versions, even in an export that
/// omits platform entities. A repository or cached definition takes precedence.
fn menu_entry_field(name: &str) -> Option<&'static str> {
    match name {
        "menuId" | "parentMenuId" | "linkType" | "linkDestination" | "linkTarget" | "title"
        | "description" => Some("STRING"),
        "imageURL" => Some("IMAGELINK"),
        "isDefault" | "isDisabled" | "isHidden" => Some("BOOLEAN"),
        _ => None,
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
    // Composer caches the child contract on the widget, including for an external child or
    // a dynamically selected mashup. Prefer the repository definition whenever it is available.
    let target = document.at(&found.path)["Properties"]["Name"]
        .as_str()
        .unwrap_or("");
    let target_present = solution.is_some_and(|s| load_existing(s, target).is_ok());
    if !target_present {
        if let Some(entry) = document.at(&found.path)["Properties"]["MashupParameters"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["ParameterName"] == property)
        {
            let direction = entry["ParmDef"]["aspects"]["bindingDirection"]
                .as_str()
                .or_else(|| entry["Aspects"]["bindingDirection"].as_str())
                .unwrap_or("BOTH");
            if source && direction == "IN" || !source && direction == "OUT" {
                return Err(AuthoringError::Refused(format!(
                    "{property} is an {} of {target}",
                    if source { "input" } else { "output" }
                )));
            }
            let base = entry["BaseType"].as_str().unwrap_or("").to_string();
            return Ok(End::Ui {
                id: found.id,
                property,
                base,
            });
        }
    }
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

/// A call is addressed by its service; its alias goes in brackets and is never part of the name.
const ALIAS_HINT: &str =
    "name the service, with the alias in brackets: data:<Entity>.<Service>[as <alias>]";

pub(super) fn resolve_service<'a>(
    model: &'a types::Model,
    platform: Option<&types::Platform>,
    raw: &str,
) -> Result<(&'a types::Entity, String), AuthoringError> {
    let mut matches = Vec::new();
    let mut unseen_chain = false;
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
            let lookup = catalog::lookup_service(entity, &model.entities, platform, service);
            if lookup.service.is_some() {
                matches.push((entity, service.to_string()));
            } else if !lookup.complete {
                unseen_chain = true;
            }
        }
    }
    matches.sort_by(|a, b| (&a.0.name, &a.1).cmp(&(&b.0.name, &b.1)));
    matches.dedup_by(|a, b| a.0.name == b.0.name && a.1 == b.1);
    match matches.as_slice() {
        [] if unseen_chain => Err(AuthoringError::Unknown(format!(
            "no entity and service twaco can see match data:{raw}; {ALIAS_HINT}; if the service is inherited, {}",
            catalog::INCOMPLETE_CHAIN_HINT
        ))),
        [] => Err(AuthoringError::Unknown(format!(
            "no entity and service in the solution match data:{raw}; {ALIAS_HINT}"
        ))),
        [only] => {
            let head = format!("{}.{}", only.0.name, only.1);
            let tail = raw.strip_prefix(&head).expect("resolved prefix");
            for selector in ["SelectedRows", "AllData"] {
                for wrong in [format!(".{selector}."), format!(".[{selector}].")] {
                    if let Some(field) = tail.strip_prefix(&wrong) {
                        let right = if selector == "AllData" {
                            "."
                        } else {
                            "[SelectedRows]."
                        };
                        let selected = if selector == "AllData" {
                            format!(" for AllData, or data:{head}[SelectedRows].{field} for selected rows")
                        } else {
                            String::new()
                        };
                        return Err(AuthoringError::Invalid(format!(
                            "invalid mashup address `data:{raw}`; use data:{head}{right}{field}{selected}"
                        )));
                    }
                }
            }
            Ok((only.0, only.1.clone()))
        }
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
    // Menu recipes require a table; keep that diagnosis before the scalar binding safeguard.
    if let Address::Data { entity_service, .. } = &address {
        let (entity, service) = resolve_service(context.model, context.platform, entity_service)?;
        let found =
            catalog::lookup_service(entity, &context.model.entities, context.platform, &service)
                .service
                .expect("resolved above");
        if found.result.base_type != "INFOTABLE" {
            return Err(AuthoringError::Refused(
                "the menu service must return an INFOTABLE".into(),
            ));
        }
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
        End::Data { collection, .. } if collection == "Session" => "Session",
        End::Data { collection, .. } if collection == "UserExtensions" => "UserExtensions",
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
        End::Data {
            entity_name: true, ..
        } => "Entity",
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
            | (
                End::Data { .. },
                End::Ui { .. } | End::Mashup { .. } | End::Data { .. }
            )
    )
}

/// The base type of a property a widget declares in its own parameter definitions (a state
/// formatter's `Format_<state>` values). A `<Property>_<state>` name for a state-formatting
/// property the catalogue lists belongs to that formatter even when the definition itself was not
/// carried; a threshold is then a NUMBER and any other state's type is unknown (`""`).
fn declared_parameter(
    widget: &Value,
    contract: &widgets::Widget,
    property: &str,
) -> Option<String> {
    fn find(value: &Value, property: &str) -> Option<String> {
        match value {
            Value::Object(map) => {
                if map.get("ParameterName").and_then(Value::as_str) == Some(property) {
                    return map
                        .get("BaseType")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                map.values().find_map(|v| find(v, property))
            }
            Value::Array(items) => items.iter().find_map(|v| find(v, property)),
            _ => None,
        }
    }
    let stored = widget["Properties"].get(property).is_some();
    let declared = find(&widget["Properties"], property);
    if declared.is_some() {
        return declared;
    }
    let (owner, state) = property.split_once('_')?;
    let formatter = contract
        .property(owner)
        .is_some_and(|p| p.base_type.as_deref() == Some("STATEFORMATTING"))
        || owner.ends_with("Format");
    // A threshold is the only kind of state value Composer's formatters declare as a number.
    let base = if state.starts_with("threshold") {
        "NUMBER"
    } else {
        ""
    };
    (!state.is_empty() && (stored || formatter)).then(|| base.to_string())
}
