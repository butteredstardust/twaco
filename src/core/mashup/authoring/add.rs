//! `mashup add` and `mashup add-contained`: a widget, function or contained mashup placed in an
//! existing mashup.

use super::{
    cache_entry, contained_parameters, default_value, finish_edit, load_existing,
    navigation_definition, parse_loaded, validate_parameter, AuthoringError, EditPlan,
    ParameterDefinition,
};
use crate::core::config::Solution;
use crate::core::mashup::edit::{MashupDocument, Placement};
use crate::core::{extension_widgets, mashup, widgets};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct AddRequest {
    pub mashup: String,
    pub widget: String,
    /// `name:TYPE` or `name:TYPE=default` inputs of an expression or validator.
    pub inputs: Vec<String>,
    pub placement: Option<Placement>,
    pub label: Option<String>,
    pub properties: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct AddPlan {
    pub edit: EditPlan,
    pub placed: mashup::edit::Placed,
    /// The widget's type, such as `ptcsgrid`.
    pub widget: String,
    /// Composer's label for it, such as `Grid`.
    pub label: String,
    /// A function widget, which goes beside the layout.
    pub function: bool,
}

/// A contained mashup is a `mashupcontainer` whose public properties mirror the child's
/// parameters.  It deliberately has its own request rather than pretending that the catalogue
/// knows the child-specific contract.
#[derive(Clone, Debug)]
pub struct AddContainedRequest {
    pub mashup: String,
    pub child: String,
    pub placement: Option<Placement>,
    pub settings: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct AddContainedPlan {
    pub(crate) edit: EditPlan,
    pub placed: mashup::edit::Placed,
    pub child: String,
    pub parameters: Vec<ContainedParameter>,
    pub(crate) settings: Vec<String>,
}

#[derive(Debug)]
pub struct ContainedParameter {
    pub name: String,
    pub base_type: String,
    pub direction: String,
}

/// Add a function widget under the mashup root, beside the layout, as Composer does; an expression
/// or validator gets its `name:TYPE` inputs as bindable targets.
fn add_function_widget(
    document: &mut MashupDocument,
    widget: &widgets::Widget,
    request: &AddRequest,
) -> Result<mashup::edit::Placed, AuthoringError> {
    if request.placement.is_some() {
        return Err(AuthoringError::Invalid(format!(
            "{} is a function widget: it sits beside the layout, so leave out --in and --right-of/--left-of/--above/--below",
            widget.type_id
        )));
    }
    let takes_inputs = matches!(
        widget.type_id.as_str(),
        "expression" | "expression2" | "validator" | "validator2"
    );
    if !request.inputs.is_empty() && !takes_inputs {
        return Err(AuthoringError::Invalid(format!(
            "--input is for an expression or a validator, not {}",
            widget.type_id
        )));
    }
    let mut definitions = Vec::new();
    for text in &request.inputs {
        let (declaration, default) = match text.split_once('=') {
            Some((declaration, default)) => (declaration, Some(default)),
            None => (text.as_str(), None),
        };
        let (name, base_type) = declaration.split_once(':').ok_or_else(|| {
            AuthoringError::Invalid(format!("--input {text:?} must be name:TYPE"))
        })?;
        validate_parameter(name, base_type, "IN", Some(""))?;
        if definitions
            .iter()
            .any(|existing: &ParameterDefinition| existing.name == name)
        {
            return Err(AuthoringError::Invalid(format!(
                "input {name} is given twice"
            )));
        }
        let default = default
            .map(|value| {
                default_value(base_type, value).map_err(|error| {
                    AuthoringError::Invalid(format!(
                        "--input {text:?} has an invalid {base_type} default: {error}"
                    ))
                })
            })
            .transpose()?;
        definitions.push(ParameterDefinition {
            name: name.into(),
            base_type: base_type.into(),
            direction: "IN".into(),
            description: String::new(),
            default,
            data_shape: None,
            visible: true,
        });
    }
    let number = document.allocate();
    let mut value = mashup::edit::new_widget(widget, number, false);
    if takes_inputs {
        let entries = definitions
            .iter()
            .map(|def| navigation_definition(def, def.default.as_ref()))
            .collect();
        mashup::edit::insert_sorted(
            mashup::edit::properties_mut(&mut value),
            "_currentParameterDefs",
            Value::Array(entries),
        );
    }
    let id = value["Properties"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    document.push_child(&[], value);
    Ok(mashup::edit::Placed {
        id,
        ..mashup::edit::Placed::default()
    })
}

/// A function widget's property that its definition makes at run time: an expression's or
/// validator's inputs and output, an events router's inputs and output, a navigation function's
/// copy of its target's parameters. As (can be a source, can be a target, base type).
pub(crate) fn function_property(
    widget: &Value,
    type_id: &str,
    property: &str,
) -> Option<(bool, bool, String)> {
    let properties = &widget["Properties"];
    let input = || {
        properties["_currentParameterDefs"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["ParameterName"] == property)
            .map(|entry| {
                (
                    entry["isBindingSource"] == true,
                    entry["isBindingTarget"] != false,
                    entry["BaseType"].as_str().unwrap_or_default().to_string(),
                )
            })
    };
    match type_id {
        "expression" | "expression2" if property == "Output" => Some((
            true,
            false,
            properties["OutputBaseType"]
                .as_str()
                .unwrap_or("STRING")
                .to_string(),
        )),
        "expression" | "expression2" | "validator" | "validator2" => input(),
        "eventsrouter" => {
            let data_type = properties["DataType"]
                .as_str()
                .unwrap_or("STRING")
                .to_string();
            if property == "Output" {
                return Some((true, false, data_type));
            }
            let count = match &properties["NumberOfInputs"] {
                Value::String(text) => text.parse().unwrap_or(0),
                other => other.as_u64().unwrap_or(0),
            };
            let n: u64 = property.strip_prefix("Input")?.parse().ok()?;
            (1..=count).contains(&n).then_some((false, true, data_type))
        }
        "navigationfunction" => properties["MashupParameters"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["ParameterName"] == property)
            .map(|entry| {
                let direction = entry["ParmDef"]["aspects"]["bindingDirection"]
                    .as_str()
                    .unwrap_or("BOTH");
                (
                    direction != "IN",
                    direction != "OUT",
                    entry["BaseType"].as_str().unwrap_or_default().to_string(),
                )
            }),
        _ => None,
    }
}

/// Plan adding one catalogue widget to an existing mashup. Nothing is written.
pub(crate) fn plan_add(
    solution: &Solution,
    request: &AddRequest,
) -> Result<AddPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    // A platform widget first, then one of the solution's own extension projects defines.
    let defined;
    let widget = match widgets::catalog().resolve(&request.widget) {
        Ok(widget) => widget,
        Err(error) => match extension_widgets::find(solution, &request.widget) {
            (Some(found), _) => {
                defined = found;
                &defined
            }
            (None, skipped) => {
                let unread = if skipped.is_empty() {
                    String::new()
                } else {
                    format!(" (unread: {})", skipped.join("; "))
                };
                return Err(AuthoringError::Invalid(format!(
                    "{error}; nor does an [[extension]] project of this solution define it, nor an extension `twaco types --platform` saw installed on the server{unread}"
                )));
            }
        },
    };
    let placed = if widget.function {
        add_function_widget(&mut document, widget, request)?
    } else {
        if !request.inputs.is_empty() {
            return Err(AuthoringError::Invalid(format!(
                "--input is for an expression or a validator, not {}",
                widget.type_id
            )));
        }
        let placement =
            match &request.placement {
                Some(placement) => placement.clone(),
                None if document.find("center").is_ok() => Placement::In("center".to_string()),
                None => return Err(AuthoringError::Invalid(
                    "say where: --in <container>, or --right-of/--left-of/--above/--below <widget>"
                        .to_string(),
                )),
            };
        document
            .place(widget, &placement)
            .map_err(AuthoringError::Edit)?
    };
    if let Some(label) = &request.label {
        let property = ["LabelText", "Label", "Text", "Title"]
            .into_iter()
            .find(|name| widget.defaults.contains_key(*name))
            .ok_or_else(|| {
                AuthoringError::Invalid(format!(
                    "{} has no label property; use --prop",
                    widget.type_id
                ))
            })?;
        let path = document
            .find(&placed.id)
            .map_err(AuthoringError::Edit)?
            .path;
        document.set_property(&path, property, Value::String(label.clone()));
    }
    for (name, text) in &request.properties {
        if [
            "Id",
            "Type",
            "DisplayName",
            "__TypeDisplayName",
            "LastContainer",
            "ResponsiveLayout",
        ]
        .contains(&name.as_str())
        {
            return Err(AuthoringError::Invalid(format!("twaco sets {name}")));
        }
        if ["_currentParameterDefs", "MashupParameters"].contains(&name.as_str()) {
            return Err(AuthoringError::Refused(format!(
                "{name} is managed by mashup param / add-contained / the function-widget inputs; use those"
            )));
        }
        let value = match widget.defaults.get(name) {
            Some(_) => property_value(widget, name, Value::String(text.to_string()))?,
            // Extension definitions have historically accepted arbitrary JSON for an opaque
            // property. Keep that compatibility when their base type gives us no scalar shape.
            None if widget.extension && widget.property(name).is_some() => {
                let base = widget
                    .property(name)
                    .and_then(|contract| contract.base_type.as_deref());
                if matches!(
                    base,
                    Some(
                        "BOOLEAN"
                            | "NUMBER"
                            | "INTEGER"
                            | "OBJECT"
                            | "JSON"
                            | "STATEFORMATTING"
                            | "RENDERERWITHSTATE"
                    )
                ) {
                    property_value(widget, name, Value::String(text.to_string()))?
                } else {
                    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
                }
            }
            // Catalogue contracts without literal defaults still have a base type, including
            // structured formatter values Composer leaves unset on a dropped widget.
            None if widget.property(name).is_some() => {
                property_value(widget, name, Value::String(text.to_string()))?
            }
            None if widget.numbered_default(name).is_some() => {
                property_value(widget, name, Value::String(text.to_string()))?
            }
            None => {
                return Err(AuthoringError::Invalid(format!(
                    "{} has no property {name}",
                    widget.type_id
                )))
            }
        };
        if let Some(contract) = widget.property(name) {
            if !contract.editable {
                return Err(AuthoringError::Refused(format!("{name} is not editable")));
            }
            if !contract.options.is_empty() && !contract.options.contains(&value) {
                return Err(AuthoringError::Invalid(format!(
                    "{name} for {} must be one of {}",
                    widget.type_id,
                    contract
                        .options
                        .iter()
                        .map(Value::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        let path = document
            .find(&placed.id)
            .map_err(AuthoringError::Edit)?
            .path;
        document.set_property(&path, name, value);
    }
    Ok(AddPlan {
        edit: finish_edit(loaded, document, "mashup add")?,
        placed,
        widget: widget.type_id.clone(),
        label: widget.type_display_name.clone(),
        function: widget.function,
    })
}

/// Plan adding a child mashup as Composer's contained-mashup builder does.  The child is read
/// from the repository, never from a server, so the cache is deterministic and reviewable.
pub(crate) fn plan_add_contained(
    solution: &Solution,
    request: &AddContainedRequest,
) -> Result<AddContainedPlan, AuthoringError> {
    if request.mashup == request.child {
        return Err(AuthoringError::Refused(
            "a mashup cannot contain itself".into(),
        ));
    }
    let loaded = load_existing(solution, &request.mashup)?;
    let child = load_existing(solution, &request.child)?;
    let child_document = parse_loaded(&child)?;
    if child_document.value()["mashupType"] == "targetmashup" {
        return Err(AuthoringError::Refused(
            "a master mashup is shown by a page container, not contained".into(),
        ));
    }
    let definitions = contained_parameters(&child, &child_document)?;
    let placement = match &request.placement {
        Some(placement) => placement.clone(),
        None if parse_loaded(&loaded)?.find("center").is_ok() => Placement::In("center".into()),
        None => {
            return Err(AuthoringError::Invalid(
                "say where: --in <container>, or --right-of/--left-of/--above/--below <widget>"
                    .into(),
            ))
        }
    };
    let mut document = parse_loaded(&loaded)?;
    let widget = widgets::catalog()
        .by_type("mashupcontainer")
        .expect("the embedded catalogue has mashupcontainer");
    let placed = document
        .place(widget, &placement)
        .map_err(AuthoringError::Edit)?;
    let path = document
        .find(&placed.id)
        .map_err(AuthoringError::Edit)?
        .path;
    document.set_property(&path, "Name", json!(request.child));
    document.set_property(
        &path,
        "MashupParameters",
        Value::Array(
            definitions
                .iter()
                .map(|def| cache_entry(def, def.default.clone()))
                .collect(),
        ),
    );
    let visible = definitions
        .iter()
        .filter(|definition| definition.visible && definition.direction != "OUT")
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    let mut settings = Vec::new();
    for (name, text) in &request.settings {
        let definition = definitions
            .iter()
            .find(|definition| definition.name == *name)
            .ok_or_else(|| {
                AuthoringError::Invalid(format!(
                    "{} has no settable parameter {name}; settable parameters are {}",
                    request.child,
                    visible.join(", ")
                ))
            })?;
        if !definition.visible || definition.direction == "OUT" {
            return Err(AuthoringError::Invalid(format!(
                "{}.{name} is not a settable parameter; settable parameters are {}",
                request.child,
                visible.join(", ")
            )));
        }
        let value = default_value(&definition.base_type, text)?;
        document.set_property(&path, name, value.clone());
        let properties = mashup::edit::properties_mut(document.at_mut(&path));
        if let Some(entry) = properties["MashupParameters"]
            .as_array_mut()
            .and_then(|entries| {
                entries
                    .iter_mut()
                    .find(|entry| entry["ParameterName"] == *name)
            })
        {
            mashup::edit::insert_sorted(
                entry.as_object_mut().expect("cache entries are objects"),
                "DefaultValue",
                value,
            );
        }
        settings.push(name.clone());
    }
    Ok(AddContainedPlan {
        edit: finish_edit(loaded, document, "mashup add-contained")?,
        placed,
        child: request.child.clone(),
        parameters: definitions
            .iter()
            .map(|definition| ContainedParameter {
                name: definition.name.clone(),
                base_type: definition.base_type.clone(),
                direction: definition.direction.clone(),
            })
            .collect(),
        settings,
    })
}

/// Parse and check a value set by `--prop` or `mashup set`. Strings have CLI syntax; a non-string
/// value arrived from MCP and is already JSON.
pub(crate) fn property_value(
    widget: &widgets::Widget,
    name: &str,
    supplied: Value,
) -> Result<Value, AuthoringError> {
    let numbered = widget.numbered_default(name);
    let default = widget.defaults.get(name).or(numbered.as_ref());
    let contract = widget.property(name);
    let state_parameters = name
        .strip_suffix("_StateParameters")
        .is_some_and(|property| {
            widget.property(property).is_some_and(|contract| {
                matches!(
                    contract.base_type.as_deref(),
                    Some("STATEFORMATTING" | "RENDERERWITHSTATE")
                )
            })
        });
    let base = contract.and_then(|contract| contract.base_type.as_deref());
    let structured = default
        .is_some_and(|value| value.is_object() || value.is_array() || value.is_null())
        || matches!(
            base,
            Some("OBJECT" | "JSON" | "STATEFORMATTING" | "RENDERERWITHSTATE")
        )
        || state_parameters
        || (!widget.extension && name == "ColumnFormat" && widget.type_id.contains("grid"));
    let array = default.is_some_and(Value::is_array) || state_parameters;
    if structured {
        let value = match supplied {
            Value::String(text) if matches!(base, Some("RENDERERWITHSTATE")) => {
                match serde_json::from_str(&text) {
                    Ok(value @ Value::Object(_)) | Ok(value @ Value::String(_)) => value,
                    Ok(_) => return Err(shape_error(name, false)),
                    Err(_) => Value::String(text),
                }
            }
            Value::String(text) => serde_json::from_str(&text).map_err(|_| {
                AuthoringError::Invalid(format!(
                    "{name} is structured; --prop {name}=<JSON> needs valid JSON"
                ))
            })?,
            value => value,
        };
        if array && !value.is_array() {
            return Err(shape_error(name, true));
        }
        if !array
            && !value.is_object()
            && !(matches!(base, Some("RENDERERWITHSTATE")) && value.is_string())
        {
            return Err(shape_error(name, false));
        }
        return Ok(value);
    }
    let text = match supplied {
        Value::String(text) => text,
        value => return scalar_value(name, default, base, value),
    };
    // The default's own type decides first; the contract's base type only when there is none.
    let boolean =
        matches!(default, Some(Value::Bool(_))) || (default.is_none() && base == Some("BOOLEAN"));
    let number = matches!(default, Some(Value::Number(_)))
        || (default.is_none() && matches!(base, Some("NUMBER" | "INTEGER")));
    match () {
        () if boolean => match text.as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(AuthoringError::Invalid(format!(
                "{name} must be true or false"
            ))),
        },
        () if number => {
            if let Ok(number) = text.parse::<i64>() {
                Ok(Value::from(number))
            } else if let Ok(number) = text.parse::<f64>() {
                serde_json::Number::from_f64(number)
                    .map(Value::Number)
                    .ok_or_else(|| AuthoringError::Invalid(format!("{name} must be a JSON number")))
            } else {
                Err(AuthoringError::Invalid(format!(
                    "{name} must be a JSON number"
                )))
            }
        }
        _ => Ok(Value::String(text)),
    }
}

fn scalar_value(
    name: &str,
    default: Option<&Value>,
    base: Option<&str>,
    value: Value,
) -> Result<Value, AuthoringError> {
    match (default, base, &value) {
        (Some(Value::Bool(_)), _, Value::Bool(_)) | (_, Some("BOOLEAN"), Value::Bool(_)) => {
            Ok(value)
        }
        (Some(Value::Number(_)), _, Value::Number(_))
        | (_, Some("NUMBER" | "INTEGER"), Value::Number(_)) => Ok(value),
        (Some(Value::String(_)), _, Value::String(_)) | (_, _, Value::String(_)) => Ok(value),
        _ => Err(AuthoringError::Invalid(format!(
            "{name} must be a JSON value of its property type"
        ))),
    }
}

fn shape_error(name: &str, array: bool) -> AuthoringError {
    AuthoringError::Invalid(format!(
        "{name} must be a JSON {}",
        if array { "array" } else { "object" }
    ))
}

/// The add plan as `--json` and MCP report it.
pub fn add_plan_json(solution: &Solution, plan: &AddPlan, applied: bool) -> Value {
    let relative = |path: &Path| solution.relative(path);
    let mut files = vec![relative(&plan.edit.entity_path)];
    if let Some(sidecars) = &plan.edit.sidecars {
        files.push(relative(&sidecars.content_path));
    }
    json!({
        "applied": applied,
        "mashup": plan.edit.mashup,
        "widget": plan.widget,
        "id": plan.placed.id,
        "containers": plan.placed.containers,
        "flipped": plan.placed.flipped,
        "wrapped": plan.placed.wrapped,
        "files": files,
    })
}

/// The contained-mashup plan as `--json` and MCP report it.
pub fn add_contained_plan_json(
    solution: &Solution,
    plan: &AddContainedPlan,
    applied: bool,
) -> Value {
    let relative = |path: &Path| solution.relative(path);
    let mut files = vec![relative(&plan.edit.entity_path)];
    if let Some(sidecars) = &plan.edit.sidecars {
        files.push(relative(&sidecars.content_path));
    }
    json!({
        "applied": applied, "mashup": plan.edit.mashup, "child": plan.child,
        "widget": "mashupcontainer", "id": plan.placed.id,
        "containers": plan.placed.containers, "flipped": plan.placed.flipped,
        "wrapped": plan.placed.wrapped, "parameters": plan.parameters.iter().map(|parameter| json!({
            "name": parameter.name, "base_type": parameter.base_type,
            "direction": parameter.direction, "set": plan.settings.contains(&parameter.name),
        })).collect::<Vec<_>>(), "files": files,
    })
}
