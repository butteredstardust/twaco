//! What a mashup is made of and how it is wired, without its XML: the widget tree, the services
//! it calls with the parameters it binds (and those it never binds), each binding, each event.
//!
//! Addresses are written the way `mashup bind` and `mashup on` take them: `ui:<widget>.<Property>`,
//! `data:<Entity>.<Service>[.<Parameter or field>]`, `mashup:<Parameter>`. A data source named by
//! the mashup's alias for a call that differs from the service shows as `<Service> as <alias>`.

use crate::core::bindings::{self, Section};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{mashup, types, workspace};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize, PartialEq)]
pub struct Description {
    pub mashup: String,
    pub file: String,
    pub mashup_type: String,
    pub parameters: Vec<MashupParameter>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub containers: Vec<Container>,
    pub widgets: Vec<WidgetNode>,
    pub services: Vec<ServiceUse>,
    pub bindings: Vec<Binding>,
    pub events: Vec<Event>,
    pub warnings: Vec<String>,
}

/// A parameter the mashup declares (its XML `ParameterDefinitions`).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MashupParameter {
    pub name: String,
    pub base_type: String,
    /// `IN`, `OUT` or `BOTH`, as `aspect.bindingDirection` says; empty when it does not.
    pub direction: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Container {
    pub id: String,
    pub mashup: String,
    pub parameters: Vec<ContainerParameter>,
}
#[derive(Debug, Serialize, PartialEq)]
pub struct ContainerParameter {
    pub name: String,
    pub status: String,
}

/// The parameters an entity file declares, from its `ParameterDefinitions` only (the
/// MobileSettings table has `FieldDefinition`s too).
pub fn mashup_parameters(src: &[u8]) -> Result<Vec<MashupParameter>, String> {
    use crate::core::scan;
    let tokens = scan::tokenize(src).map_err(|error| error.to_string())?;
    let entity =
        crate::core::sidecar::entity_element(&tokens, src).ok_or("not an entity document")?;
    let Some(&definitions) = scan::child_tags(&tokens, src, "ParameterDefinitions", entity).first()
    else {
        return Ok(Vec::new());
    };
    let read = |tag: usize, name: &str| -> Result<String, String> {
        Ok(scan::attribute(src, &tokens[tag], name)
            .map_err(|error| error.to_string())?
            .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
            .unwrap_or_default())
    };
    scan::child_tags(&tokens, src, "FieldDefinition", definitions)
        .into_iter()
        .map(|tag| {
            Ok(MashupParameter {
                name: read(tag, "name")?,
                base_type: read(tag, "baseType")?,
                direction: read(tag, "aspect.bindingDirection")?,
                default: None,
            })
        })
        .collect()
}

#[derive(Debug, Serialize, PartialEq)]
pub struct WidgetNode {
    pub id: String,
    #[serde(rename = "type")]
    pub type_id: String,
    pub display_name: String,
    pub depth: usize,
    /// `row` or `column`, for a responsive container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// It fills the container it is in.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fills: bool,
    /// What it shows: its label, text or title, as written (a `[[token]]` stays one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ServiceUse {
    /// `Entity.Service`, or `Entity.Service as alias`.
    pub service: String,
    pub section: String,
    pub alias: String,
    /// Each declared parameter and what sets it: a binding's source, `static`, or `unbound`.
    pub parameters: Vec<ParameterUse>,
    /// What runs it: event triggers.
    pub invoked_by: Vec<String>,
    /// False when twaco could not find the service's definition in the repository.
    pub resolved: bool,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ParameterUse {
    pub name: String,
    pub base_type: String,
    pub set_by: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Binding {
    pub id: String,
    pub from: String,
    pub to: String,
    /// A property map after the first: the runtime does not apply it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ignored: bool,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub trigger: String,
    pub handler: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DescribeError {
    #[error("no mashup {0} in the solution")]
    Unknown(String),
    #[error("{0}")]
    Unreadable(String),
}

impl Coded for DescribeError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Unknown(_) => ErrorCode::UnknownEntity,
            Self::Unreadable(_) => ErrorCode::InvalidData,
        }
    }
}

/// Describe the mashup `name` as the repository has it: its sidecar content when it has one.
pub fn describe(solution: &Solution, name: &str) -> Result<Description, DescribeError> {
    let entity = workspace::entities(solution)
        .into_iter()
        .find(|e| e.info.collection == "Mashups" && e.info.name == name)
        .ok_or_else(|| DescribeError::Unknown(name.to_string()))?;
    let dir = workspace::mashup_dir(solution, &entity);
    let bytes = std::fs::read(&entity.path).map_err(|error| {
        DescribeError::Unreadable(format!("{}: {error}", entity.path.display()))
    })?;
    let parameters = mashup_parameters(&bytes)
        .map_err(|why| DescribeError::Unreadable(format!("{}: {why}", entity.path.display())))?;
    let content = match workspace::read_mashup(&dir)
        .map_err(|error| DescribeError::Unreadable(error.to_string()))?
    {
        Some(assets) => assets.content,
        None => {
            mashup::extract(&bytes)
                .map_err(|error| DescribeError::Unreadable(error.to_string()))?
                .content
        }
    };
    let value: Value = serde_json::from_str(&content)
        .map_err(|error| DescribeError::Unreadable(format!("{name}: {error}")))?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let resolver = bindings::Resolver::new(&model, platform.as_ref());
    let file = entity
        .path
        .strip_prefix(&solution.root)
        .unwrap_or(&entity.path)
        .to_string_lossy()
        .replace('\\', "/");
    let mut description = of_content(name, &file, &value, &|section, service| {
        resolver.service(section, service).service.map(|found| {
            found
                .parameters
                .iter()
                .map(|p| (p.name.clone(), p.value.base_type.clone()))
                .collect()
        })
    });
    description.parameters = parameters
        .into_iter()
        .map(|mut parameter| {
            parameter.default = value["UI"]["Properties"]["_currentParameterDefs"]
                .as_array()
                .and_then(|defs| {
                    defs.iter()
                        .find(|def| def["ParameterName"] == parameter.name)
                })
                .and_then(|def| def.get("DefaultValue"))
                .cloned();
            parameter
        })
        .collect();
    description.containers = containers(solution, &value);
    Ok(description)
}

/// A service's declared parameters and their base types, or `None` when it cannot be found.
type Signature<'a> = dyn Fn(&Section, &str) -> Option<Vec<(String, String)>> + 'a;

fn of_content(name: &str, file: &str, value: &Value, signature: &Signature<'_>) -> Description {
    let sections = bindings::data_sections(value);
    let address = Addresses {
        sections: &sections,
    };
    let mut warnings = Vec::new();

    let mut bindings_out = Vec::new();
    // Which parameter of which call each binding sets: (section, alias, parameter) -> sources.
    let mut setters: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    for binding in array(value, "DataBindings") {
        let id = text(binding, "Id");
        let maps = array(binding, "PropertyMaps");
        if maps.len() > 1 {
            warnings.push(format!(
                "binding {id} has {} property maps; the runtime applies only the first",
                maps.len()
            ));
        }
        for (index, map) in maps.iter().enumerate() {
            let from = address.of(
                &text(binding, "SourceArea"),
                &text(binding, "SourceSection"),
                &text(binding, "SourceId"),
                &text(map, "SourceProperty"),
                &text(binding, "SourceDetails"),
            );
            let to = address.of(
                &text(binding, "TargetArea"),
                &text(binding, "TargetSection"),
                &text(binding, "TargetId"),
                &text(map, "TargetProperty"),
                "",
            );
            // Only the first map runs, so only it sets a parameter.
            if index == 0 && text(binding, "TargetArea") == "Data" {
                setters
                    .entry((
                        text(binding, "TargetSection"),
                        text(binding, "TargetId"),
                        text(map, "TargetProperty"),
                    ))
                    .or_default()
                    .push(from.clone());
            }
            bindings_out.push(Binding {
                id: id.clone(),
                from,
                to,
                ignored: index > 0,
            });
        }
    }

    let mut events = Vec::new();
    let mut invokers: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for event in array(value, "Events") {
        let trigger = address.of(
            &text(event, "EventTriggerArea"),
            &text(event, "EventTriggerSection"),
            &text(event, "EventTriggerId"),
            &text(event, "EventTriggerEvent"),
            "",
        );
        let handler_area = text(event, "EventHandlerArea");
        // A Data handler is addressed by its section (EventHandlerId) and the call's alias.
        let handler = if handler_area == "Data" {
            invokers
                .entry((
                    text(event, "EventHandlerId"),
                    text(event, "EventHandlerService"),
                ))
                .or_default()
                .push(trigger.clone());
            address.call(
                &text(event, "EventHandlerId"),
                &text(event, "EventHandlerService"),
            )
        } else {
            address.of(
                &handler_area,
                "",
                &text(event, "EventHandlerId"),
                &text(event, "EventHandlerService"),
                "",
            )
        };
        events.push(Event {
            id: text(event, "Id"),
            trigger,
            handler,
        });
    }

    let mut services = Vec::new();
    for (section_name, section) in &sections {
        for call in &section.services {
            let declared = signature(section, &call.target);
            let resolved = declared.is_some();
            let mut parameters = Vec::new();
            let mut seen = BTreeSet::new();
            for (parameter, base_type) in declared.unwrap_or_default() {
                seen.insert(parameter.clone());
                let mut set_by = setters
                    .get(&(
                        section_name.clone(),
                        call.instance.clone(),
                        parameter.clone(),
                    ))
                    .cloned()
                    .unwrap_or_default();
                if call.static_parameters.contains(&parameter) {
                    set_by.push("static".to_string());
                }
                if set_by.is_empty() {
                    set_by.push("unbound".to_string());
                }
                parameters.push(ParameterUse {
                    name: parameter,
                    base_type,
                    set_by,
                });
            }
            // Something bound to a name the service does not declare: shown, and warned about
            // when the service was found (the bindings gate fails it).
            for ((section_key, alias, parameter), sources) in &setters {
                if section_key != section_name
                    || alias != &call.instance
                    || seen.contains(parameter)
                {
                    continue;
                }
                if resolved && !parameter.is_empty() {
                    warnings.push(format!(
                        "{} binds {parameter}, which the service does not declare",
                        address.call(section_name, &call.instance)
                    ));
                }
                parameters.push(ParameterUse {
                    name: parameter.clone(),
                    base_type: String::new(),
                    set_by: sources.clone(),
                });
            }
            services.push(ServiceUse {
                service: address.call(section_name, &call.instance)["data:".len()..].to_string(),
                section: section_name.clone(),
                alias: call.instance.clone(),
                parameters,
                invoked_by: invokers
                    .get(&(section_name.clone(), call.instance.clone()))
                    .cloned()
                    .unwrap_or_default(),
                resolved,
            });
        }
    }

    let mut widgets = Vec::new();
    if let Some(ui) = value.get("UI") {
        tree(ui, 0, &mut widgets);
    }
    Description {
        mashup: name.to_string(),
        file: file.to_string(),
        mashup_type: text(value, "mashupType"),
        parameters: Vec::new(),
        containers: Vec::new(),
        widgets,
        services,
        bindings: bindings_out,
        events,
        warnings,
    }
}

fn containers(solution: &Solution, value: &Value) -> Vec<Container> {
    fn visit(solution: &Solution, widget: &Value, out: &mut Vec<Container>) {
        let properties = &widget["Properties"];
        if properties["Type"] == "mashupcontainer" {
            if let Some(mashup) = properties["Name"].as_str() {
                let declared = workspace::entities(solution)
                    .into_iter()
                    .find(|entity| {
                        entity.info.collection == "Mashups" && entity.info.name == mashup
                    })
                    .and_then(|entity| std::fs::read(entity.path).ok())
                    .and_then(|xml| mashup_parameters(&xml).ok())
                    .unwrap_or_default();
                let cached = properties["MashupParameters"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let mut parameters = declared
                    .iter()
                    .map(|parameter| {
                        let status = match cached
                            .iter()
                            .find(|cache| cache["ParameterName"] == parameter.name)
                        {
                            None => "missing in cache",
                            Some(cache)
                                if cache["BaseType"].as_str()
                                    != Some(parameter.base_type.as_str()) =>
                            {
                                "stale type"
                            }
                            Some(_) => "ok",
                        };
                        ContainerParameter {
                            name: parameter.name.clone(),
                            status: status.into(),
                        }
                    })
                    .collect::<Vec<_>>();
                for cache in &cached {
                    if let Some(name) = cache["ParameterName"].as_str() {
                        if !declared.iter().any(|parameter| parameter.name == name) {
                            parameters.push(ContainerParameter {
                                name: name.into(),
                                status: "not in child".into(),
                            });
                        }
                    }
                }
                out.push(Container {
                    id: properties["Id"].as_str().unwrap_or_default().into(),
                    mashup: mashup.into(),
                    parameters,
                });
            }
        }
        for child in widget["Widgets"].as_array().into_iter().flatten() {
            visit(solution, child, out);
        }
    }
    let mut out = Vec::new();
    visit(solution, &value["UI"], &mut out);
    out
}

struct Addresses<'a> {
    sections: &'a BTreeMap<String, Section>,
}

impl Addresses<'_> {
    /// `data:<Entity>.<Service>`, with the alias when the call has its own name.
    fn call(&self, section: &str, alias: &str) -> String {
        match self.sections.get(section) {
            Some(found) => {
                let target = found
                    .services
                    .iter()
                    .find(|call| call.instance == alias)
                    .map(|call| call.target.as_str());
                match target {
                    Some(target) if target != alias => {
                        format!("data:{}.{target}[as {alias}]", found.entity)
                    }
                    _ => format!("data:{}.{alias}", found.entity),
                }
            }
            None => format!("data:{section}.{alias}"),
        }
    }

    fn entity(&self, section: &str) -> String {
        self.sections
            .get(section)
            .map_or(section, |found| found.entity.as_str())
            .to_string()
    }

    fn is_entity_slot(&self, section: &str, id: &str) -> bool {
        id == "EntityName"
            && self
                .sections
                .get(section)
                .is_some_and(|found| found.services.iter().all(|call| call.instance != id))
    }

    fn of(&self, area: &str, section: &str, id: &str, property: &str, details: &str) -> String {
        let property = if property.is_empty() {
            String::new()
        } else {
            format!(".{property}")
        };
        match area {
            "UI" => format!("ui:{id}{property}"),
            "Mashup" => format!("mashup:{}", property.trim_start_matches('.')),
            // A shape's or template's data source takes the entity it runs on from a binding
            // to `EntityName`, not to a service.
            "Data" if self.is_entity_slot(section, id) => {
                format!("data:{}[{id}]", self.entity(section))
            }
            "Data" => {
                let rows = match details {
                    "" | "AllData" => String::new(),
                    other => format!("[{other}]"),
                };
                format!("{}{rows}{property}", self.call(section, id))
            }
            // Session and UserExtensions name themselves three times over.
            other if id == other && (section.is_empty() || section == other) => {
                format!(
                    "{}:{}",
                    other.to_ascii_lowercase(),
                    property.trim_start_matches('.')
                )
            }
            other => {
                let section = if section.is_empty() {
                    String::new()
                } else {
                    format!("{section}.")
                };
                format!("{}:{section}{id}{property}", other.to_ascii_lowercase())
            }
        }
    }
}

fn tree(widget: &Value, depth: usize, out: &mut Vec<WidgetNode>) {
    let properties = &widget["Properties"];
    // Not a widget as Composer writes one: nothing to say about it.
    if !properties.is_object() {
        return;
    }
    let type_id = text(properties, "Type");
    let shown = ["Label", "LabelText", "Text", "Title", "ButtonText"]
        .iter()
        .find_map(|key| properties[*key].as_str().filter(|t| !t.is_empty()))
        .map(str::to_string);
    out.push(WidgetNode {
        id: text(properties, "Id"),
        display_name: text(properties, "DisplayName"),
        depth,
        direction: (type_id == "flexcontainer")
            .then(|| text(properties, "flex-direction"))
            .filter(|d| !d.is_empty()),
        // The root's ResponsiveLayout says the page is responsive, not that it fills anything.
        fills: depth > 0 && type_id != "flexcontainer" && properties["ResponsiveLayout"] == true,
        text: shown,
        type_id,
    });
    for child in array(widget, "Widgets") {
        tree(child, depth + 1, out);
    }
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The description as text: compact, one line per thing.
pub fn render(description: &Description) -> String {
    let mut out = format!(
        "mashup {} ({}){}\n",
        description.mashup,
        description.file,
        if description.mashup_type.is_empty() {
            String::new()
        } else {
            format!(", {}", description.mashup_type)
        }
    );
    if !description.parameters.is_empty() {
        let parameters: Vec<String> = description
            .parameters
            .iter()
            .map(|p| {
                let detail = [
                    (!p.direction.is_empty()).then_some(p.direction.clone()),
                    p.default.as_ref().map(|value| format!("default {value}")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                if detail.is_empty() {
                    format!("{} {}", p.name, p.base_type)
                } else {
                    format!("{} {} ({})", p.name, p.base_type, detail.join(", "))
                }
            })
            .collect();
        out.push_str(&format!("parameters: {}\n", parameters.join(", ")));
    }
    out.push_str("\nwidgets\n");
    if description.widgets.is_empty() {
        out.push_str("  (none Composer would recognise)\n");
    }
    for widget in &description.widgets {
        let mut notes = Vec::new();
        if let Some(direction) = &widget.direction {
            notes.push(direction.clone());
        }
        if widget.fills {
            notes.push("fills".to_string());
        }
        if widget.display_name != widget.id && !widget.display_name.is_empty() {
            notes.push(format!("\"{}\"", widget.display_name));
        }
        if let Some(text) = &widget.text {
            notes.push(format!("text {text:?}"));
        }
        out.push_str(&format!(
            "  {}{} {}{}\n",
            "  ".repeat(widget.depth),
            widget.id,
            widget.type_id,
            if notes.is_empty() {
                String::new()
            } else {
                format!("  ({})", notes.join(", "))
            }
        ));
    }
    if !description.containers.is_empty() {
        out.push_str("\ncontainers\n");
        for container in &description.containers {
            let drift: Vec<String> = container
                .parameters
                .iter()
                .filter(|p| p.status != "ok")
                .map(|p| format!("{} {}", p.name, p.status))
                .collect();
            out.push_str(&format!(
                "  {} shows {}{}\n",
                container.id,
                container.mashup,
                if drift.is_empty() {
                    String::new()
                } else {
                    format!("  (parameters: {})", drift.join(", "))
                }
            ));
        }
    }
    if !description.services.is_empty() {
        out.push_str("\nservices\n");
        for service in &description.services {
            out.push_str(&format!(
                "  {}{}\n",
                service.service,
                if service.resolved {
                    ""
                } else {
                    "  (definition not found in the repository)"
                }
            ));
            for parameter in &service.parameters {
                out.push_str(&format!(
                    "      {} {}: {}\n",
                    parameter.name,
                    parameter.base_type,
                    parameter.set_by.join(", ")
                ));
            }
            if !service.invoked_by.is_empty() {
                out.push_str(&format!(
                    "      runs on: {}\n",
                    service.invoked_by.join(", ")
                ));
            }
        }
    }
    if !description.bindings.is_empty() {
        out.push_str("\nbindings\n");
        for binding in &description.bindings {
            out.push_str(&format!(
                "  {} -> {}{}\n",
                binding.from,
                binding.to,
                if binding.ignored {
                    "  (ignored: only the first property map applies)"
                } else {
                    ""
                }
            ));
        }
    }
    if !description.events.is_empty() {
        out.push_str("\nevents\n");
        for event in &description.events {
            out.push_str(&format!("  {} -> {}\n", event.trigger, event.handler));
        }
    }
    for warning in &description.warnings {
        out.push_str(&format!("\nwarning: {warning}"));
    }
    if !description.warnings.is_empty() {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An order screen: a grid of GetOrders, a text field for SaveOrder's note, a button that saves,
    /// and SaveOrder's `id` never bound.
    pub(super) fn orders() -> Value {
        json!({
            "Data": {
                "Things_Acme.Orders.Manager": {
                    "EntityName": "Acme.Orders.Manager",
                    "EntityType": "Things",
                    "Services": [
                        {"Name": "GetOrders", "Target": "GetOrders", "Parameters": {}},
                        {"Name": "Save", "Target": "SaveOrder", "Parameters": {"mode": "fast"}}
                    ]
                }
            },
            "DataBindings": [
                {"Id": "b1", "SourceArea": "Data", "SourceSection": "Things_Acme.Orders.Manager",
                 "SourceId": "GetOrders", "SourceDetails": "AllData", "TargetArea": "UI",
                 "TargetId": "ptcsgrid-3", "TargetSection": "",
                 "PropertyMaps": [{"SourceProperty": "", "TargetProperty": "Data"}]},
                {"Id": "b2", "SourceArea": "UI", "SourceId": "ptcstextfield-4", "SourceSection": "",
                 "TargetArea": "Data", "TargetSection": "Things_Acme.Orders.Manager", "TargetId": "Save",
                 "PropertyMaps": [
                     {"SourceProperty": "Text", "TargetProperty": "note"},
                     {"SourceProperty": "Text", "TargetProperty": "title"}
                 ]}
            ],
            "Events": [
                {"Id": "e1", "EventTriggerArea": "UI", "EventTriggerId": "ptcsbutton-5",
                 "EventTriggerEvent": "Clicked", "EventTriggerSection": "",
                 "EventHandlerArea": "Data", "EventHandlerId": "Things_Acme.Orders.Manager",
                 "EventHandlerService": "Save"}
            ],
            "UI": {
                "Properties": {"Id": "mashup-root", "Type": "mashup", "DisplayName": "Mashup"},
                "Widgets": [{
                    "Properties": {"Id": "flexcontainer-2", "Type": "flexcontainer",
                                   "DisplayName": "center", "flex-direction": "column"},
                    "Widgets": [
                        {"Properties": {"Id": "ptcsgrid-3", "Type": "ptcsgrid",
                                        "DisplayName": "ptcs-grid-3", "ResponsiveLayout": true}},
                        {"Properties": {"Id": "ptcstextfield-4", "Type": "ptcstextfield",
                                        "DisplayName": "ptcs-textfield-4", "Text": ""}},
                        {"Properties": {"Id": "ptcsbutton-5", "Type": "ptcsbutton",
                                        "DisplayName": "ptcs-button-5", "Label": "[[Acme.Save]]"}}
                    ]
                }]
            },
            "mashupType": "mashup"
        })
    }

    fn signature(_: &Section, service: &str) -> Option<Vec<(String, String)>> {
        match service {
            "GetOrders" => Some(vec![]),
            "SaveOrder" => Some(vec![
                ("id".into(), "LONG".into()),
                ("mode".into(), "STRING".into()),
                ("note".into(), "STRING".into()),
                ("title".into(), "STRING".into()),
            ]),
            _ => None,
        }
    }

    #[test]
    fn services_list_what_sets_each_parameter_and_what_is_left_unbound() {
        let description = of_content("Acme.Orders_MU", "Mashups/x.xml", &orders(), &signature);
        let save = &description.services[1];
        assert_eq!(save.service, "Acme.Orders.Manager.SaveOrder[as Save]");
        let set: Vec<(&str, Vec<String>)> = save
            .parameters
            .iter()
            .map(|p| (p.name.as_str(), p.set_by.clone()))
            .collect();
        assert_eq!(
            set,
            [
                ("id", vec!["unbound".to_string()]),
                ("mode", vec!["static".to_string()]),
                ("note", vec!["ui:ptcstextfield-4.Text".to_string()]),
                // Bound only by the binding's second map, which the runtime ignores.
                ("title", vec!["unbound".to_string()]),
            ]
        );
        assert_eq!(save.invoked_by, ["ui:ptcsbutton-5.Clicked"]);
    }

    #[test]
    fn each_property_map_is_its_own_row_and_extra_maps_are_warned_about() {
        let description = of_content("Acme.Orders_MU", "Mashups/x.xml", &orders(), &signature);
        let rows: Vec<(&str, &str)> = description
            .bindings
            .iter()
            .map(|b| (b.from.as_str(), b.to.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("data:Acme.Orders.Manager.GetOrders", "ui:ptcsgrid-3.Data"),
                (
                    "ui:ptcstextfield-4.Text",
                    "data:Acme.Orders.Manager.SaveOrder[as Save].note"
                ),
                (
                    "ui:ptcstextfield-4.Text",
                    "data:Acme.Orders.Manager.SaveOrder[as Save].title"
                ),
            ]
        );
        assert!(description
            .warnings
            .iter()
            .any(|w| w.contains("b2 has 2 property maps")));
        assert!(!description.bindings[1].ignored);
        assert!(description.bindings[2].ignored, "only the first map runs");
        assert!(render(&description).contains("(ignored: only the first property map applies)"));
        assert_eq!(
            description.events[0].handler,
            "data:Acme.Orders.Manager.SaveOrder[as Save]"
        );
        // A binding to a parameter the service does not declare is shown and warned about.
        let mut content = orders();
        content["DataBindings"].as_array_mut().unwrap().push(json!({
            "Id": "b3", "SourceArea": "UI", "SourceId": "ptcstextfield-4", "SourceSection": "",
            "TargetArea": "Data", "TargetSection": "Things_Acme.Orders.Manager", "TargetId": "Save",
            "PropertyMaps": [{"SourceProperty": "Text", "TargetProperty": "priority"}]
        }));
        let description = of_content("Acme.Orders_MU", "Mashups/x.xml", &content, &signature);
        assert!(description
            .warnings
            .iter()
            .any(|w| w.contains("binds priority, which the service does not declare")));
    }

    #[test]
    fn the_widget_tree_shows_layout_and_text() {
        let description = of_content("Acme.Orders_MU", "Mashups/x.xml", &orders(), &signature);
        let center = &description.widgets[1];
        assert_eq!(
            (
                center.id.as_str(),
                center.direction.as_deref(),
                center.depth
            ),
            ("flexcontainer-2", Some("column"), 1)
        );
        assert!(description.widgets[2].fills);
        assert_eq!(
            description.widgets[4].text.as_deref(),
            Some("[[Acme.Save]]")
        );
        let text = render(&description);
        assert!(
            text.contains("    flexcontainer-2 flexcontainer  (column, \"center\")"),
            "{text}"
        );
        assert!(text.contains("      id LONG: unbound"), "{text}");
        assert!(
            text.contains(
                "  ui:ptcsbutton-5.Clicked -> data:Acme.Orders.Manager.SaveOrder[as Save]"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_binding_to_a_data_sources_entity_name_reads_as_that_and_the_root_never_fills() {
        let mut content = orders();
        content["DataBindings"].as_array_mut().unwrap().push(json!({
            "Id": "b3", "SourceArea": "Mashup", "SourceId": "mashup-root", "SourceSection": "",
            "TargetArea": "Data", "TargetSection": "Things_Acme.Orders.Manager", "TargetId": "EntityName",
            "PropertyMaps": [{"SourceProperty": "Manager", "TargetProperty": "value"}]
        }));
        content["UI"]["Properties"]["ResponsiveLayout"] = json!(true);
        let description = of_content("Acme.Orders_MU", "Mashups/x.xml", &content, &signature);
        assert_eq!(
            (
                description.bindings[3].from.as_str(),
                description.bindings[3].to.as_str()
            ),
            ("mashup:Manager", "data:Acme.Orders.Manager[EntityName]")
        );
        assert!(!description.widgets[0].fills);
        let address = Addresses {
            sections: &bindings::data_sections(&content),
        };
        assert_eq!(
            address.of(
                "UserExtensions",
                "UserExtensions",
                "UserExtensions",
                "name",
                ""
            ),
            "userextensions:name"
        );
    }

    #[test]
    fn parameters_come_from_the_parameter_definitions_only() {
        let xml = br#"<Entities><Mashups><Mashup name="Acme.Orders_MU" projectName="Acme">
            <ConfigurationTables><ConfigurationTable name="MobileSettings"><DataShape><FieldDefinitions>
                <FieldDefinition baseType="BOOLEAN" name="disableZoom"></FieldDefinition>
            </FieldDefinitions></DataShape></ConfigurationTable></ConfigurationTables>
            <ParameterDefinitions>
                <FieldDefinition aspect.bindingDirection="BOTH" baseType="THINGNAME" name="Manager"></FieldDefinition>
                <FieldDefinition baseType="STRING" name="R&amp;D"></FieldDefinition>
            </ParameterDefinitions>
            <mashupContent><![CDATA[{}]]></mashupContent></Mashup></Mashups></Entities>"#;
        assert_eq!(
            mashup_parameters(xml).unwrap(),
            [
                MashupParameter {
                    name: "Manager".into(),
                    base_type: "THINGNAME".into(),
                    direction: "BOTH".into(),
                    default: None,
                },
                MashupParameter {
                    name: "R&D".into(),
                    base_type: "STRING".into(),
                    direction: String::new(),
                    default: None,
                },
            ]
        );
        let none =
            br#"<Entities><Mashups><Mashup name="M" projectName="P"></Mashup></Mashups></Entities>"#;
        assert!(mashup_parameters(none).unwrap().is_empty());
    }

    #[test]
    fn a_service_the_repository_lacks_is_marked_and_its_bindings_still_shown() {
        let missing = |_: &Section, _: &str| None;
        let description = of_content("Acme.Orders_MU", "Mashups/x.xml", &orders(), &missing);
        assert!(!description.services[1].resolved);
        assert!(!description
            .warnings
            .iter()
            .any(|w| w.contains("does not declare")));
        assert_eq!(
            description.services[1].parameters.len(),
            1,
            "note, from the binding that runs"
        );
    }
}

#[cfg(test)]
mod repository_tests {
    use super::*;
    use std::path::Path;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn describe_reads_the_mashup_and_the_services_it_calls_from_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "twaco.toml", "[[project]]\nname = \"Acme\"\n");
        write(
            root,
            "Things/Acme.Orders.Manager.xml",
            r#"<Entities><Things><Thing name="Acme.Orders.Manager" projectName="Acme" thingTemplate="GenericThing"><ThingShape><ServiceDefinitions>
                <ServiceDefinition name="GetOrders"><ParameterDefinitions></ParameterDefinitions><ResultType baseType="INFOTABLE" name="result"></ResultType></ServiceDefinition>
                <ServiceDefinition name="SaveOrder"><ParameterDefinitions>
                    <FieldDefinition baseType="LONG" name="id"></FieldDefinition>
                    <FieldDefinition baseType="STRING" name="note"></FieldDefinition>
                </ParameterDefinitions><ResultType baseType="NOTHING" name="result"></ResultType></ServiceDefinition>
            </ServiceDefinitions></ThingShape></Thing></Things></Entities>"#,
        );
        write(
            root,
            "Mashups/Acme.Orders_MU.xml",
            &format!(
                "<Entities><Mashups><Mashup name=\"Acme.Orders_MU\" projectName=\"Acme\"><ParameterDefinitions><FieldDefinition baseType=\"STRING\" name=\"Order\"></FieldDefinition></ParameterDefinitions><mashupContent><![CDATA[{}]]></mashupContent></Mashup></Mashups></Entities>",
                super::tests::orders()
            ),
        );
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let description = describe(&solution, "Acme.Orders_MU").unwrap();
        assert_eq!(description.parameters[0].name, "Order");
        let save = &description.services[1];
        assert!(save.resolved, "SaveOrder is found in the Thing's XML");
        let set: Vec<(&str, &str, &str)> = save
            .parameters
            .iter()
            .map(|p| (p.name.as_str(), p.base_type.as_str(), p.set_by[0].as_str()))
            .collect();
        assert_eq!(
            set,
            [
                ("id", "LONG", "unbound"),
                ("note", "STRING", "ui:ptcstextfield-4.Text")
            ]
        );
        let json = serde_json::to_value(&description).unwrap();
        assert_eq!(json["services"][1]["parameters"][0]["set_by"][0], "unbound");
        assert!(matches!(
            describe(&solution, "Acme.Nope_MU"),
            Err(DescribeError::Unknown(_))
        ));
    }
}
