//! `mashup param`: a mashup parameter, changed in the mashup and in every container or
//! navigation function that shows it.

use super::{finish_edit, load_existing, parse_loaded, AuthoringError, EditPlan, ExistingMashup};
use crate::core::config::Solution;
use crate::core::lock::WorkspaceLock;
use crate::core::mashup::describe::mashup_parameters;
use crate::core::mashup::edit::MashupDocument;
use crate::core::transaction::Transaction;
use crate::core::{mashup, scan, splice, workspace};
use serde_json::{json, Map, Value};

/// One edit to a mashup parameter definition.
#[derive(Clone, Debug)]
pub struct ParameterRequest {
    pub mashup: String,
    pub action: ParameterAction,
    pub name: String,
    pub base_type: Option<String>,
    pub direction: Option<String>,
    pub description: Option<String>,
    pub default: ParameterDefault,
    pub data_shape: Option<String>,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterAction {
    Add,
    Set,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParameterDefault {
    Unchanged,
    Set(String),
    Clear,
}

#[derive(Debug)]
pub struct ParameterPlan {
    pub(crate) edits: Vec<EditPlan>,
    pub(crate) action: ParameterAction,
    pub mashup: String,
    pub name: String,
    pub base_type: String,
    pub direction: String,
    pub refreshed: Vec<(String, String)>,
    pub retyped: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct ParameterDefinition {
    pub(super) name: String,
    pub(super) base_type: String,
    pub(super) direction: String,
    pub(super) description: String,
    pub(super) default: Option<Value>,
    pub(super) data_shape: Option<String>,
    pub(super) visible: bool,
}

/// `name:TYPE[:DIRECTION]`, or `name:INFOTABLE:DataShape[:DIRECTION]`.
pub(super) fn parse_new_parameter(text: &str) -> Result<ParameterDefinition, AuthoringError> {
    let shape = || {
        AuthoringError::Invalid(format!(
            "--param {text:?} must be name:TYPE[:DIRECTION] or name:INFOTABLE:DataShape[:DIRECTION]"
        ))
    };
    let mut parts = text.split(':');
    let name = parts.next().unwrap_or_default();
    let base_type = parts.next().ok_or_else(shape)?;
    let data_shape = if base_type == "INFOTABLE" {
        Some(
            parts
                .next()
                .filter(|name| !name.is_empty())
                .ok_or_else(shape)?,
        )
    } else {
        None
    };
    let direction = parts.next().unwrap_or("BOTH");
    if parts.next().is_some() {
        return Err(shape());
    }
    validate_parameter(name, base_type, direction, data_shape)?;
    Ok(ParameterDefinition {
        name: name.into(),
        base_type: base_type.into(),
        direction: direction.into(),
        description: String::new(),
        default: None,
        data_shape: data_shape.map(Into::into),
        visible: !name.starts_with("ConfigForm_"),
    })
}

/// The platform's base types, and the ones only a mashup parameter has.
fn known_base_type(name: &str) -> bool {
    crate::core::entity::edit::BASE_TYPES.contains(&name)
        || matches!(
            name,
            "BASE64"
                | "BLOB"
                | "BOOLEAN"
                | "DATASHAPENAME"
                | "DATETIME"
                | "GUID"
                | "HTML"
                | "HYPERLINK"
                | "IMAGE"
                | "IMAGELINK"
                | "INFOTABLE"
                | "INTEGER"
                | "JSON"
                | "LOCATION"
                | "LONG"
                | "MASHUPNAME"
                | "MEDIAENTITYNAME"
                | "MENUNAME"
                | "NAME"
                | "NUMBER"
                | "PASSWORD"
                | "PROJECTNAME"
                | "QUERY"
                | "SCHEDULE"
                | "STRING"
                | "TAG"
                | "TEXT"
                | "THINGCODE"
                | "THINGNAME"
                | "THINGSHAPENAME"
                | "THINGTEMPLATENAME"
                | "USERNAME"
                | "VARIANT"
                | "XML"
        )
}

pub(super) fn validate_parameter(
    name: &str,
    base_type: &str,
    direction: &str,
    data_shape: Option<&str>,
) -> Result<(), AuthoringError> {
    // A 10.2 server keeps `order-id`, `ünï` and `1st` as mashup parameters (2026-10-30).
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(AuthoringError::Invalid(format!(
            "parameter name {name:?} must be letters, digits, _ and -"
        )));
    }
    if matches!(name, "Entity" | "ConfigurationMashup") {
        return Err(AuthoringError::Invalid(format!(
            "parameter name {name} is reserved"
        )));
    }
    if !known_base_type(base_type) {
        return Err(AuthoringError::Invalid(format!(
            "unknown ThingWorx base type {base_type}"
        )));
    }
    if !matches!(direction, "IN" | "OUT" | "BOTH") {
        return Err(AuthoringError::Invalid(
            "direction must be IN, OUT or BOTH".into(),
        ));
    }
    if base_type == "INFOTABLE" && data_shape.is_none() {
        return Err(AuthoringError::Invalid(
            "INFOTABLE parameters require --data-shape".into(),
        ));
    }
    Ok(())
}

pub(super) fn default_value(base_type: &str, value: &str) -> Result<Value, AuthoringError> {
    match base_type {
        "INFOTABLE" => Err(AuthoringError::Invalid(
            "an INFOTABLE parameter's default is a table; set it in Composer".into(),
        )),
        // A 10.2 server keeps a JSON default written as an object (2026-10-30).
        "JSON" => serde_json::from_str::<Value>(value).map_err(|error| {
            AuthoringError::Invalid(format!("--default {value:?} is not JSON: {error}"))
        }),
        "BOOLEAN" => match value {
            "true" => Ok(json!(true)),
            "false" => Ok(json!(false)),
            _ => Err(AuthoringError::Invalid(
                "default must be true or false".into(),
            )),
        },
        "NUMBER" => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| AuthoringError::Invalid("default must be a JSON number".into())),
        "INTEGER" | "LONG" => value
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| AuthoringError::Invalid("default must be a whole number".into())),
        _ => Ok(Value::String(value.into())),
    }
}

fn definition_from_request(
    request: &ParameterRequest,
    existing: Option<&ParameterDefinition>,
) -> Result<ParameterDefinition, AuthoringError> {
    let base_type = request
        .base_type
        .clone()
        .or_else(|| existing.map(|p| p.base_type.clone()))
        .ok_or_else(|| {
            AuthoringError::Invalid("--type is required when adding a parameter".into())
        })?;
    let direction = request
        .direction
        .clone()
        .or_else(|| existing.map(|p| p.direction.clone()))
        .unwrap_or_else(|| "BOTH".into());
    let description = request
        .description
        .clone()
        .or_else(|| existing.map(|p| p.description.clone()))
        .unwrap_or_default();
    let data_shape = request
        .data_shape
        .clone()
        .or_else(|| existing.and_then(|p| p.data_shape.clone()));
    validate_parameter(&request.name, &base_type, &direction, data_shape.as_deref())?;
    let default = match &request.default {
        ParameterDefault::Set(value) => Some(default_value(&base_type, value)?),
        ParameterDefault::Clear => None,
        ParameterDefault::Unchanged => existing.and_then(|p| p.default.clone()),
    };
    Ok(ParameterDefinition {
        name: request.name.clone(),
        base_type,
        direction,
        description,
        default,
        data_shape,
        visible: !request.name.starts_with("ConfigForm_"),
    })
}

fn content_parameter(document: &MashupDocument, name: &str) -> Option<ParameterDefinition> {
    let properties = document.value()["UI"]["Properties"].as_object()?;
    properties
        .get("_currentParameterDefs")?
        .as_array()?
        .iter()
        .find_map(|entry| {
            (entry["ParameterName"] == name).then(|| ParameterDefinition {
                name: name.into(),
                base_type: entry["BaseType"].as_str().unwrap_or_default().into(),
                direction: entry["Aspects"]["bindingDirection"]
                    .as_str()
                    .unwrap_or("BOTH")
                    .into(),
                description: entry["Description"].as_str().unwrap_or_default().into(),
                default: root_default(properties, entry, name),
                data_shape: entry["Aspects"]["dataShape"].as_str().map(str::to_string),
                visible: entry["Aspects"]["isVisible"] != false && !name.starts_with("ConfigForm_"),
            })
        })
}

/// A root parameter's default: the root property of its name, as Composer writes it, or the
/// `DefaultValue` an earlier twaco wrote into its definition.
pub(crate) fn root_default(
    properties: &Map<String, Value>,
    entry: &Value,
    name: &str,
) -> Option<Value> {
    properties
        .get(name)
        .cloned()
        .or_else(|| entry.get("DefaultValue").cloned())
}

fn parameter_entry(def: &ParameterDefinition) -> Value {
    let mut aspects = Map::new();
    aspects.insert("bindingDirection".into(), json!(def.direction));
    if let Some(shape) = &def.data_shape {
        aspects.insert("dataShape".into(), json!(shape));
    }
    aspects.insert("isMandatory".into(), json!(false));
    let mut item = Map::new();
    item.insert("Aspects".into(), Value::Object(aspects));
    item.insert("BaseType".into(), json!(def.base_type));
    // Composer keeps a mashup's own parameter default as the root property of its name, never
    // as a `DefaultValue` here (0 of 52 root definitions in real repositories, and a JSON value
    // Composer saved on 10.2, 2026-10-30); `set_content_parameter` writes that property.
    item.insert("Description".into(), json!(def.description));
    item.insert("ParameterName".into(), json!(def.name));
    Value::Object(item)
}

pub(super) fn set_content_parameter(
    document: &mut MashupDocument,
    def: &ParameterDefinition,
    remove: bool,
) {
    let properties = document.root_properties_mut();
    // The server keeps these keys sorted; appended, the read-back differs from what was sent.
    if !properties.contains_key("_currentParameterDefs") {
        mashup::edit::insert_sorted(
            properties,
            "_currentParameterDefs",
            Value::Array(Vec::new()),
        );
    }
    let defs = properties
        .get_mut("_currentParameterDefs")
        .expect("inserted above")
        .as_array_mut()
        .expect("parameter definitions must be an array");
    if remove {
        defs.retain(|entry| entry["ParameterName"] != def.name);
        properties.shift_remove(&def.name);
        return;
    }
    if let Some(entry) = defs
        .iter_mut()
        .find(|entry| entry["ParameterName"] == def.name)
    {
        *entry = parameter_entry(def);
    } else {
        defs.push(parameter_entry(def));
    }
    match &def.default {
        Some(value) => {
            mashup::edit::insert_sorted(properties, &def.name, value.clone());
        }
        None => {
            properties.shift_remove(&def.name);
        }
    }
}

fn field_xml(def: &ParameterDefinition) -> Result<String, scan::IllegalXmlChar> {
    let mut attributes = format!(
        "aspect.bindingDirection=\"{}\"",
        scan::escape_attribute(&def.direction)?
    );
    if let Some(shape) = &def.data_shape {
        attributes.push_str(&format!(
            " aspect.dataShape=\"{}\"",
            scan::escape_attribute(shape)?
        ));
    }
    attributes.push_str(&format!(" aspect.isMandatory=\"false\" baseType=\"{}\" description=\"{}\" name=\"{}\" ordinal=\"0\"", scan::escape_attribute(&def.base_type)?, scan::escape_attribute(&def.description)?, scan::escape_attribute(&def.name)?));
    Ok(format!("<FieldDefinition {attributes}/>"))
}

/// Change just the parameter definition element, retaining every byte outside it.
pub(super) fn splice_parameter_definition(
    src: &[u8],
    def: &ParameterDefinition,
    old_name: Option<&str>,
    remove: bool,
) -> Result<Vec<u8>, AuthoringError> {
    let tokens = scan::tokenize(src).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let entity = crate::core::sidecar::entity_element(&tokens, src)
        .ok_or_else(|| AuthoringError::Invalid("not an entity document".into()))?;
    let definitions = *scan::child_tags(&tokens, src, "ParameterDefinitions", entity)
        .first()
        .ok_or_else(|| AuthoringError::Invalid("mashup has no ParameterDefinitions".into()))?;
    let fields = scan::child_tags(&tokens, src, "FieldDefinition", definitions);
    let find = |name: &str| -> Result<Option<usize>, AuthoringError> {
        fields
            .iter()
            .copied()
            .find_map(|field| match scan::attribute(src, &tokens[field], "name") {
                Ok(Some(span))
                    if scan::decode_entities(&String::from_utf8_lossy(span.of(src))) == name =>
                {
                    Some(Ok(field))
                }
                Ok(_) => None,
                Err(e) => Some(Err(AuthoringError::Invalid(e.to_string()))),
            })
            .transpose()
    };
    let wanted = old_name.unwrap_or(&def.name);
    let found = find(wanted)?;
    if remove {
        let field = found
            .ok_or_else(|| AuthoringError::Unknown(format!("mashup has no parameter {wanted}")))?;
        let span = scan::element_span(&tokens, field)
            .ok_or_else(|| AuthoringError::Invalid("malformed FieldDefinition".into()))?;
        return splice::splice(src, &[splice::Edit::new(span, Vec::new())])
            .map_err(|e| AuthoringError::Invalid(e.to_string()));
    }
    // New lines end as the document's lines do, so a CRLF file stays CRLF.
    let nl = crate::core::sync::newline_of(&String::from_utf8_lossy(src));
    let written = field_xml(def)
        .map_err(|e| AuthoringError::Invalid(e.to_string()))?
        .replace('\n', nl);
    if let Some(field) = found {
        let span = scan::element_span(&tokens, field)
            .ok_or_else(|| AuthoringError::Invalid("malformed FieldDefinition".into()))?;
        return splice::splice(src, &[splice::Edit::new(span, written)])
            .map_err(|e| AuthoringError::Invalid(e.to_string()));
    }
    // The server keeps parameter definitions sorted by name, ignoring case: insert where it would.
    let wanted_key = def.name.to_lowercase();
    for &field in &fields {
        let name = scan::attribute(src, &tokens[field], "name")
            .map_err(|e| AuthoringError::Invalid(e.to_string()))?
            .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
            .unwrap_or_default();
        if name.to_lowercase() > wanted_key {
            let start = tokens[field].span.start;
            let indent = String::from_utf8_lossy(&src[..start])
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .to_string();
            let separator = if indent.trim().is_empty() && !indent.is_empty() {
                format!("{nl}{indent}")
            } else {
                String::new()
            };
            return splice::splice(
                src,
                &[splice::Edit::new(
                    scan::Span::new(start, start),
                    format!("{written}{separator}"),
                )],
            )
            .map_err(|e| AuthoringError::Invalid(e.to_string()));
        }
    }
    let end = scan::element_end_in(&tokens, src, definitions)
        .ok_or_else(|| AuthoringError::Invalid("malformed ParameterDefinitions".into()))?;
    let inner = scan::Span::new(tokens[definitions].span.end, tokens[end].span.start);
    let parent_indent = String::from_utf8_lossy(&src[..tokens[definitions].span.start])
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect::<String>();
    let child_indent = format!("{parent_indent}    ");
    let before = String::from_utf8_lossy(inner.of(src));
    let replacement = if before.trim().is_empty() {
        format!("{nl}{child_indent}{written}{nl}{parent_indent}")
    } else {
        format!(
            "{}{nl}{child_indent}{written}{nl}{parent_indent}",
            before.trim_end()
        )
    };
    splice::splice(src, &[splice::Edit::new(inner, replacement)])
        .map_err(|e| AuthoringError::Invalid(e.to_string()))
}

pub(super) fn cache_entry(def: &ParameterDefinition, value: Option<Value>) -> Value {
    let mut aspects = Map::new();
    aspects.insert("bindingDirection".into(), json!(def.direction));
    if let Some(shape) = &def.data_shape {
        aspects.insert("dataShape".into(), json!(shape));
    }
    aspects.insert("isMandatory".into(), json!("false"));
    let mut parm = Map::new();
    parm.insert("aspects".into(), Value::Object(aspects));
    parm.insert("baseType".into(), json!(def.base_type));
    parm.insert("description".into(), json!(def.description));
    parm.insert("name".into(), json!(def.name));
    parm.insert("ordinal".into(), json!(0));
    let mut entry = Map::new();
    entry.insert("BaseType".into(), json!(def.base_type));
    if let Some(value) = value {
        entry.insert("DefaultValue".into(), value);
    }
    entry.insert("Description".into(), json!(def.description));
    entry.insert("ParameterName".into(), json!(def.name));
    entry.insert("ParmDef".into(), Value::Object(parm));
    Value::Object(entry)
}

/// Whether a widget caches `child`'s parameters: a container showing it, or a navigation function
/// opening it (284 of 319 real ones keep the same `MashupParameters` cache a container does).
fn shows_mashup(properties: &Value, child: &str) -> bool {
    match properties["Type"].as_str() {
        Some("mashupcontainer") => properties["Name"] == child,
        Some("navigationfunction") => properties["TargetMashup"] == child,
        _ => false,
    }
}

/// A navigation function's own copy of the target's parameters, in its own shape.
pub(super) fn navigation_definition(def: &ParameterDefinition, value: Option<&Value>) -> Value {
    let (direction, source, target) = match def.direction.as_str() {
        "IN" => ("Target", false, true),
        "OUT" => ("Source", true, false),
        _ => ("Both", true, true),
    };
    let mut entry = Map::new();
    entry.insert("BaseType".into(), json!(def.base_type));
    if let Some(value) = value {
        entry.insert("DefaultValue".into(), value.clone());
    }
    entry.insert("Description".into(), json!(def.description));
    entry.insert("ParameterName".into(), json!(def.name));
    entry.insert("bindingDirection".into(), json!(direction));
    entry.insert("bindingType".into(), json!("Property"));
    entry.insert("isBindingSource".into(), json!(source));
    entry.insert("isBindingTarget".into(), json!(target));
    entry.insert("shown".into(), json!(def.visible));
    Value::Object(entry)
}

fn refresh_containers(
    value: &mut Value,
    child: &str,
    definitions: &[ParameterDefinition],
    refreshed: &mut Vec<(String, String)>,
) {
    fn visit(
        widget: &mut Value,
        child: &str,
        definitions: &[ParameterDefinition],
        refreshed: &mut Vec<(String, String)>,
    ) {
        let shows = widget
            .get("Properties")
            .is_some_and(|properties| shows_mashup(properties, child));
        let navigation = widget["Properties"]["Type"] == "navigationfunction";
        let properties = widget.get_mut("Properties").and_then(Value::as_object_mut);
        if let Some(properties) = properties {
            if shows {
                let id = properties
                    .get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let previous = properties
                    .get("MashupParameters")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut rebuilt = Vec::new();
                for def in definitions {
                    let old = previous
                        .iter()
                        .find(|entry| entry["ParameterName"] == def.name);
                    let old_type = old.and_then(|entry| entry["BaseType"].as_str());
                    let value = if old_type == Some(def.base_type.as_str()) {
                        old.and_then(|entry| entry.get("DefaultValue"))
                            .cloned()
                            .or_else(|| properties.get(&def.name).cloned())
                            .or_else(|| def.default.clone())
                    } else {
                        properties.shift_remove(&def.name);
                        def.default.clone()
                    };
                    if let Some(value) = value.as_ref().filter(|_| !navigation) {
                        mashup::edit::insert_sorted(properties, &def.name, value.clone());
                    }
                    rebuilt.push(cache_entry(def, value));
                }
                for old in previous {
                    if let Some(name) = old["ParameterName"].as_str() {
                        if !definitions.iter().any(|d| d.name == name) {
                            properties.shift_remove(name);
                        }
                    }
                }
                if navigation {
                    let own: Vec<Value> = rebuilt
                        .iter()
                        .zip(definitions)
                        .map(|(entry, def)| navigation_definition(def, entry.get("DefaultValue")))
                        .collect();
                    mashup::edit::insert_sorted(
                        properties,
                        "_currentParameterDefs",
                        Value::Array(own),
                    );
                }
                mashup::edit::insert_sorted(properties, "MashupParameters", Value::Array(rebuilt));
                refreshed.push((id, child.into()));
            }
        }
        if let Some(children) = widget.get_mut("Widgets").and_then(Value::as_array_mut) {
            for child_widget in children {
                visit(child_widget, child, definitions, refreshed);
            }
        }
    }
    visit(&mut value["UI"], child, definitions, refreshed);
}

fn change_parameter_bindings(
    value: &mut Value,
    child: &str,
    name: &str,
    new_type: Option<&str>,
    force: bool,
    labels: &mut Vec<String>,
) -> Result<(), AuthoringError> {
    let container_ids: std::collections::BTreeSet<String> = {
        fn collect(widget: &Value, child: &str, into: &mut std::collections::BTreeSet<String>) {
            if shows_mashup(&widget["Properties"], child) {
                if let Some(id) = widget["Properties"]["Id"].as_str() {
                    into.insert(id.into());
                }
            }
            for w in widget["Widgets"].as_array().into_iter().flatten() {
                collect(w, child, into);
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        collect(&value["UI"], child, &mut ids);
        ids
    };
    let bindings = value["DataBindings"].as_array_mut().into_iter().flatten();
    for binding in bindings {
        let source = (container_ids.is_empty() && binding["SourceArea"].as_str() == Some("Mashup"))
            || container_ids.contains(binding["SourceId"].as_str().unwrap_or_default());
        let target = (container_ids.is_empty() && binding["TargetArea"].as_str() == Some("Mashup"))
            || container_ids.contains(binding["TargetId"].as_str().unwrap_or_default());
        if !source && !target {
            continue;
        }
        let id = binding["Id"].as_str().unwrap_or("binding").to_string();
        let maps = binding["PropertyMaps"].as_array_mut().into_iter().flatten();
        let mut remove_binding = false;
        for map in maps {
            let relevant = (source && map["SourceProperty"] == name)
                || (target && map["TargetProperty"] == name);
            if !relevant {
                continue;
            }
            if new_type.is_none() {
                if !force {
                    labels.push(id.clone());
                } else {
                    remove_binding = true;
                    labels.push(id.clone());
                }
                continue;
            }
            let new_type = new_type.unwrap();
            // Each side naming the parameter takes the new type (both, when the binding joins
            // two widgets showing the same mashup); then the pair must still be one bind allows.
            if source && map["SourceProperty"] == name {
                map["SourcePropertyBaseType"] = json!(new_type);
            }
            if target && map["TargetProperty"] == name {
                map["TargetPropertyBaseType"] = json!(new_type);
            }
            let from = map["SourcePropertyBaseType"].as_str().unwrap_or_default();
            let to = map["TargetPropertyBaseType"].as_str().unwrap_or_default();
            // The rule `mashup bind` applies: the same base type, or a STRING target.
            if !(from.is_empty() || to.is_empty() || from == to || to == "STRING") {
                return Err(AuthoringError::Refused(format!(
                    "binding {id} would join {from} with {to}; change or remove it first"
                )));
            }
            labels.push(id.clone());
        }
        if remove_binding {
            binding["__twaco_remove"] = json!(true);
        }
    }
    if new_type.is_none() && !force && !labels.is_empty() {
        return Err(AuthoringError::Refused(format!(
            "parameter {name} is still bound by {}; use --force to remove those bindings",
            labels.join(", ")
        )));
    }
    if force {
        if let Some(bindings) = value["DataBindings"].as_array_mut() {
            bindings.retain(|b| b["__twaco_remove"] != true);
        }
    }
    Ok(())
}

fn document_parameters(document: &MashupDocument) -> Vec<ParameterDefinition> {
    let properties = document.value()["UI"]["Properties"].as_object();
    document.value()["UI"]["Properties"]["_currentParameterDefs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry["ParameterName"]
                .as_str()
                .map(|name| ParameterDefinition {
                    name: name.into(),
                    base_type: entry["BaseType"].as_str().unwrap_or_default().into(),
                    direction: entry["Aspects"]["bindingDirection"]
                        .as_str()
                        .unwrap_or("BOTH")
                        .into(),
                    description: entry["Description"].as_str().unwrap_or_default().into(),
                    default: properties
                        .and_then(|properties| root_default(properties, entry, name)),
                    data_shape: entry["Aspects"]["dataShape"].as_str().map(str::to_string),
                    visible: entry["Aspects"]["isVisible"] != false
                        && !name.starts_with("ConfigForm_"),
                })
        })
        .collect()
}

/// Plan a parameter addition, definition change or deletion, including every container that
/// displays the child mashup. Nothing is written.
pub(crate) fn plan_parameter(
    solution: &Solution,
    request: &ParameterRequest,
) -> Result<ParameterPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let entity_parameters =
        mashup_parameters(&loaded.entity_before).map_err(AuthoringError::Invalid)?;
    let mut document = parse_loaded(&loaded)?;
    let existing_xml = entity_parameters
        .iter()
        .find(|parameter| parameter.name == request.name);
    let existing = content_parameter(&document, &request.name).or_else(|| {
        existing_xml.map(|parameter| ParameterDefinition {
            name: parameter.name.clone(),
            base_type: parameter.base_type.clone(),
            direction: if parameter.direction.is_empty() {
                "BOTH".into()
            } else {
                parameter.direction.clone()
            },
            description: String::new(),
            default: None,
            data_shape: None,
            visible: !parameter.name.starts_with("ConfigForm_"),
        })
    });
    if request.action == ParameterAction::Add && existing.is_some() {
        return Err(AuthoringError::Exists(format!(
            "mashup {} already has parameter {}",
            request.mashup, request.name
        )));
    }
    if request.action != ParameterAction::Add && existing.is_none() {
        return Err(AuthoringError::Unknown(format!(
            "mashup {} has no parameter {}",
            request.mashup, request.name
        )));
    }
    if request.action == ParameterAction::Remove && existing.is_none() {
        return Err(AuthoringError::Unknown(format!(
            "mashup {} has no parameter {}",
            request.mashup, request.name
        )));
    }
    let definition = existing
        .as_ref()
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| definition_from_request(request, None))?;
    let definition = if request.action == ParameterAction::Remove {
        definition
    } else {
        definition_from_request(request, existing.as_ref())?
    };
    if definition.base_type == "INFOTABLE" {
        let shape = definition.data_shape.as_deref().unwrap_or_default();
        if !workspace::entities(solution)
            .iter()
            .any(|entity| entity.info.collection == "DataShapes" && entity.info.name == shape)
        {
            return Err(AuthoringError::Unknown(format!(
                "no DataShape {shape} in the solution"
            )));
        }
    }
    let mut changed_bindings = Vec::new();
    // Bindings change only with a removal or a new type; a set that keeps the type (say, only to
    // refresh every copy of the parameters) leaves even an odd existing binding alone.
    let touches_bindings = request.action == ParameterAction::Remove
        || existing
            .as_ref()
            .is_some_and(|before| before.base_type != definition.base_type);
    if touches_bindings {
        change_parameter_bindings(
            document.value_mut(),
            &request.mashup,
            &request.name,
            (request.action != ParameterAction::Remove).then_some(definition.base_type.as_str()),
            request.force,
            &mut changed_bindings,
        )?;
    }
    set_content_parameter(
        &mut document,
        &definition,
        request.action == ParameterAction::Remove,
    );
    let definitions = document_parameters(&document);
    let mut child_edit = finish_edit(loaded, document, "mashup param")?;
    child_edit.entity_after = splice_parameter_definition(
        &child_edit.entity_after,
        &definition,
        existing.as_ref().map(|_| request.name.as_str()),
        request.action == ParameterAction::Remove,
    )?;
    let mut edits = vec![child_edit];
    let mut refreshed = Vec::new();
    for entity in workspace::entities(solution)
        .into_iter()
        .filter(|entity| entity.info.collection == "Mashups" && entity.info.name != request.mashup)
    {
        let bytes = std::fs::read(&entity.path).map_err(|error| {
            AuthoringError::Unreadable(format!("{}: {error}", entity.path.display()))
        })?;
        let content = mashup::extract(&bytes)
            .map_err(AuthoringError::Mashup)?
            .content;
        let value: Value = serde_json::from_str(&content).map_err(|error| {
            AuthoringError::Invalid(format!("{}: {error}", entity.path.display()))
        })?;
        if !has_container(&value, &request.mashup) {
            continue;
        }
        let parent = load_existing(solution, &entity.info.name)?;
        let mut parent_document = parse_loaded(&parent)?;
        if touches_bindings {
            change_parameter_bindings(
                parent_document.value_mut(),
                &request.mashup,
                &request.name,
                (request.action != ParameterAction::Remove)
                    .then_some(definition.base_type.as_str()),
                request.force,
                &mut changed_bindings,
            )?;
        }
        let first = refreshed.len();
        refresh_containers(
            parent_document.value_mut(),
            &request.mashup,
            &definitions,
            &mut refreshed,
        );
        // Report the mashup holding each refreshed widget, not the one it shows.
        for entry in &mut refreshed[first..] {
            entry.1 = entity.info.name.clone();
        }
        edits.push(finish_edit(parent, parent_document, "mashup param")?);
    }
    let retyped = if request.action == ParameterAction::Set {
        changed_bindings.clone()
    } else {
        Vec::new()
    };
    let removed = if request.action == ParameterAction::Remove {
        changed_bindings
    } else {
        Vec::new()
    };
    Ok(ParameterPlan {
        edits,
        action: request.action,
        mashup: request.mashup.clone(),
        name: request.name.clone(),
        base_type: definition.base_type,
        direction: definition.direction,
        refreshed,
        retyped,
        removed,
    })
}

fn has_container(value: &Value, name: &str) -> bool {
    fn visit(widget: &Value, name: &str) -> bool {
        shows_mashup(&widget["Properties"], name)
            || widget["Widgets"]
                .as_array()
                .is_some_and(|children| children.iter().any(|child| visit(child, name)))
    }
    visit(&value["UI"], name)
}

/// Apply every affected mashup in one transaction.
pub(crate) fn apply_parameter(
    solution: &Solution,
    plan: &ParameterPlan,
    lock: &WorkspaceLock,
) -> Result<(), AuthoringError> {
    let mut transaction = Transaction::new(&solution.root, "mashup param");
    for edit in &plan.edits {
        transaction
            .replace_file(
                &edit.entity_path,
                &edit.entity_before,
                edit.entity_after.clone(),
            )
            .map_err(AuthoringError::Write)?;
        if let Some(sidecars) = &edit.sidecars {
            transaction
                .replace_file(
                    &sidecars.content_path,
                    &sidecars.content_before,
                    // An edited content.json keeps the byte-order mark it had.
                    workspace::keeping_bom(&sidecars.content_path, sidecars.content_after.clone()),
                )
                .map_err(AuthoringError::Write)?;
        }
    }
    transaction.apply(lock).map_err(AuthoringError::Write)?;
    Ok(())
}

pub fn parameter_plan_json(solution: &Solution, plan: &ParameterPlan, applied: bool) -> Value {
    let files = plan
        .edits
        .iter()
        .flat_map(|edit| {
            std::iter::once(&edit.entity_path)
                .chain(edit.sidecars.iter().map(|sidecar| &sidecar.content_path))
        })
        .map(|path| solution.relative(path))
        .collect::<Vec<_>>();
    json!({ "applied": applied, "action": format!("{:?}", plan.action).to_ascii_lowercase(), "mashup": plan.mashup, "name": plan.name, "base_type": plan.base_type, "direction": plan.direction, "containers_refreshed": plan.refreshed, "bindings_retyped": plan.retyped, "bindings_removed": plan.removed, "files": files })
}

/// Content metadata is Composer's current definition; an older/unmanaged mashup can have only
/// XML definitions, which remain a supported fallback.
pub(super) fn contained_parameters(
    child: &ExistingMashup,
    document: &MashupDocument,
) -> Result<Vec<ParameterDefinition>, AuthoringError> {
    let mut definitions = document_parameters(document);
    if definitions.is_empty() {
        definitions = mashup_parameters(&child.entity_before)
            .map_err(AuthoringError::Invalid)?
            .into_iter()
            .map(|parameter| ParameterDefinition {
                visible: !parameter.name.starts_with("ConfigForm_"),
                name: parameter.name,
                base_type: parameter.base_type,
                direction: if parameter.direction.is_empty() {
                    "BOTH".into()
                } else {
                    parameter.direction
                },
                description: String::new(),
                default: None,
                data_shape: None,
            })
            .collect();
    }
    if matches!(
        document.value()["mashupType"].as_str(),
        Some("thingtemplatemashup" | "thingshapemashup")
    ) {
        definitions.push(ParameterDefinition {
            name: "Entity".into(),
            base_type: "THINGNAME".into(),
            direction: "BOTH".into(),
            description: String::new(),
            default: None,
            data_shape: None,
            visible: true,
        });
    }
    Ok(definitions)
}

/// When the widget's copy lacks the parameter, or has it with another type, refresh every copy
/// of `child`'s parameters in the document before the binding is written against it.
pub(super) fn ensure_cached(
    document: &mut MashupDocument,
    found: &mashup::edit::WidgetRef,
    child: &str,
    definitions: &[ParameterDefinition],
    definition: &ParameterDefinition,
    refreshed: &mut Vec<String>,
) {
    let current = document.at(&found.path)["Properties"]["MashupParameters"]
        .as_array()
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry["ParameterName"] == definition.name.as_str()
                    && entry["BaseType"] == definition.base_type.as_str()
            })
        });
    if !current {
        let mut entries = Vec::new();
        refresh_containers(document.value_mut(), child, definitions, &mut entries);
        for (id, _) in entries {
            if !refreshed.contains(&id) {
                refreshed.push(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_infotable_parameter_names_its_data_shape() {
        let rows = parse_new_parameter("Rows:INFOTABLE:Acme.Row_DS:IN").unwrap();
        assert_eq!(rows.data_shape.as_deref(), Some("Acme.Row_DS"));
        assert_eq!(rows.direction, "IN");
        let both = parse_new_parameter("Rows:INFOTABLE:Acme.Row_DS").unwrap();
        assert_eq!(both.direction, "BOTH");
        assert!(parse_new_parameter("Rows:INFOTABLE").is_err());
        assert!(parse_new_parameter("Rows:INFOTABLE:").is_err());
        let plain = parse_new_parameter("Count:INTEGER:OUT").unwrap();
        assert_eq!(plain.data_shape, None);
        assert!(parse_new_parameter("Pos:VEC3").is_ok());
        for name in ["order-id", "ünï", "1st"] {
            assert!(
                parse_new_parameter(&format!("{name}:STRING")).is_ok(),
                "{name}"
            );
        }
        assert_eq!(
            default_value("JSON", r#"{"b":1}"#).unwrap(),
            serde_json::json!({"b": 1})
        );
        assert!(default_value("JSON", "{nope").is_err());
        assert!(default_value("INFOTABLE", "x").is_err());
        for name in ["na.me", "a b", ""] {
            assert!(
                parse_new_parameter(&format!("{name}:STRING")).is_err(),
                "{name:?}"
            );
        }
    }
}
