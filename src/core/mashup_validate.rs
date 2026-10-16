//! `twaco mashup validate`: what in a mashup's content ThingWorx would load but not honour, or
//! what the next edit would trip over. Read-only, from the repository alone.

use super::config::Solution;
use super::mashup;
use super::mashup_authoring::function_property;
use super::mashup_edit::{self, ROOT_ID};
use super::{contained_check, widgets, workspace};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Problem {
    pub rule: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct MashupReport {
    pub mashup: String,
    pub file: String,
    pub problems: Vec<Problem>,
}

#[derive(Debug, thiserror::Error)]
pub enum ValidateError {
    #[error("no mashup {0} in the solution")]
    Unknown(String),
}

impl crate::core::codes::Coded for ValidateError {
    fn code(&self) -> crate::core::codes::ErrorCode {
        crate::core::codes::ErrorCode::UnknownEntity
    }
}

/// Validate one mashup, or every one when `name` is `None`.
pub fn validate(
    solution: &Solution,
    name: Option<&str>,
) -> Result<Vec<MashupReport>, ValidateError> {
    let files: Vec<_> = workspace::discover(solution)
        .entities
        .into_iter()
        .filter(|file| file.info.collection == "Mashups")
        .filter(|file| name.is_none_or(|name| file.info.name == name))
        .collect();
    if let (Some(name), true) = (name, files.is_empty()) {
        return Err(ValidateError::Unknown(name.to_string()));
    }
    let drift = contained_check::run(solution);
    let mut reports = Vec::new();
    for file in files {
        let relative = file
            .path
            .strip_prefix(&solution.root)
            .unwrap_or(&file.path)
            .to_string_lossy()
            .replace('\\', "/");
        let mut problems = Vec::new();
        let content = std::fs::read(&file.path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| mashup::extract(&bytes).map_err(|error| error.to_string()))
            .map(|assets| assets.content);
        match content {
            Err(error) => problems.push(problem("unreadable", error)),
            Ok(content) => check_content(&content, &mut problems),
        }
        for found in drift.problems.iter().filter(|p| p.file == relative) {
            problems.push(problem(&found.rule, found.message.clone()));
        }
        reports.push(MashupReport {
            mashup: file.info.name.clone(),
            file: relative,
            problems,
        });
    }
    Ok(reports)
}

fn problem(rule: &str, message: String) -> Problem {
    Problem {
        rule: rule.to_string(),
        message,
    }
}

/// Every content rule, for one mashup's `mashupContent`.
pub(crate) fn check_content(content: &str, problems: &mut Vec<Problem>) {
    let document = match mashup_edit::parse(content) {
        Ok(document) => document,
        Err(error) => {
            problems.push(problem("structure", error.to_string()));
            return;
        }
    };
    for found in document.problems() {
        problems.push(problem("mixed-container", found.message));
    }
    let value = document.value();
    // `parse` refuses duplicate ids; they surface as the structure rule. A stale id_index is
    // harmless: twaco and Composer number a new widget past the highest id in use.
    let widgets_by_id: BTreeMap<String, Value> = document
        .widgets()
        .into_iter()
        .map(|widget| {
            let properties = document.at(&widget.path)["Properties"].clone();
            (widget.id, properties)
        })
        .collect();
    // Every data source and call by name, read directly: a dynamic source may name no entity.
    let calls: BTreeMap<String, BTreeSet<String>> = value["Data"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(section, data)| {
            let names = data["Services"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|call| call["Name"].as_str().map(str::to_string))
                .collect();
            (section.clone(), names)
        })
        .collect();
    let call_exists =
        |section: &str, call: &str| calls.get(section).is_some_and(|names| names.contains(call));
    // Bindings: each end exists and the catalogue contract allows it.
    for binding in value["DataBindings"].as_array().into_iter().flatten() {
        let id = binding["Id"].as_str().unwrap_or("a binding");
        let maps = binding["PropertyMaps"].as_array().map_or(0, Vec::len);
        if maps == 0 {
            problems.push(problem(
                "no-property-map",
                format!("binding {id} has no property map, so it carries nothing"),
            ));
            continue;
        }
        if maps > 1 {
            problems.push(problem(
                "several-property-maps",
                format!(
                    "binding {id} has {maps} property maps; the runtime applies only the first"
                ),
            ));
        }
        let map = &binding["PropertyMaps"][0];
        for (source, area, section, end, property) in [
            (
                true,
                &binding["SourceArea"],
                &binding["SourceSection"],
                &binding["SourceId"],
                &map["SourceProperty"],
            ),
            (
                false,
                &binding["TargetArea"],
                &binding["TargetSection"],
                &binding["TargetId"],
                &map["TargetProperty"],
            ),
        ] {
            let (area, section, end, property) = (
                area.as_str().unwrap_or_default(),
                section.as_str().unwrap_or_default(),
                end.as_str().unwrap_or_default(),
                property.as_str().unwrap_or_default(),
            );
            match area {
                "UI" => check_ui_end(&widgets_by_id, id, end, property, source, problems),
                "Data" if end == "EntityName" => {
                    if !calls.contains_key(section) {
                        problems.push(problem("missing-data-source", format!("binding {id} names data source {section}, which the mashup does not have")));
                    }
                }
                "Data" if !call_exists(section, end) => problems.push(problem(
                    "missing-call",
                    format!(
                        "binding {id} names {section} call {end}, which the mashup does not have"
                    ),
                )),
                _ => {}
            }
        }
    }
    // Events: the trigger and the handler exist, with that event and that service.
    for event in value["Events"].as_array().into_iter().flatten() {
        let id = event["Id"].as_str().unwrap_or("an event");
        let text = |key: &str| event[key].as_str().unwrap_or_default().to_string();
        match text("EventTriggerArea").as_str() {
            "UI" => check_ui_member(
                &widgets_by_id,
                id,
                &text("EventTriggerId"),
                &text("EventTriggerEvent"),
                true,
                problems,
            ),
            "Data" if !call_exists(&text("EventTriggerSection"), &text("EventTriggerId")) => {
                problems.push(problem(
                    "missing-call",
                    format!(
                        "event {id} is triggered by {} call {}, which the mashup does not have",
                        text("EventTriggerSection"),
                        text("EventTriggerId")
                    ),
                ))
            }
            "Mashup" if text("EventTriggerId") != ROOT_ID => problems.push(problem(
                "unknown-widget",
                format!(
                    "event {id} names mashup {}; the mashup is {ROOT_ID}",
                    text("EventTriggerId")
                ),
            )),
            _ => {}
        }
        match text("EventHandlerArea").as_str() {
            "UI" => check_ui_member(
                &widgets_by_id,
                id,
                &text("EventHandlerId"),
                &text("EventHandlerService"),
                false,
                problems,
            ),
            "Data" if !call_exists(&text("EventHandlerId"), &text("EventHandlerService")) => {
                problems.push(problem(
                    "missing-call",
                    format!(
                        "event {id} runs {} call {}, which the mashup does not have",
                        text("EventHandlerId"),
                        text("EventHandlerService")
                    ),
                ))
            }
            _ => {}
        }
    }
}

/// A binding end on a widget: the widget exists, and a catalogue-known widget allows the property
/// in that direction. Dynamic properties (inputs, a container's or navigation function's
/// parameters) are read from the widget itself.
fn check_ui_end(
    widgets_by_id: &BTreeMap<String, Value>,
    binding: &str,
    id: &str,
    property: &str,
    source: bool,
    problems: &mut Vec<Problem>,
) {
    let Some(properties) = widgets_by_id.get(id) else {
        problems.push(problem(
            "unknown-widget",
            format!("binding {binding} names widget {id}, which the mashup does not have"),
        ));
        return;
    };
    let type_id = properties["Type"].as_str().unwrap_or_default();
    if type_id == "mashupcontainer" {
        return; // the contained-mashups rules judge these
    }
    let Some(widget) = widgets::catalog()
        .by_type(type_id)
        .filter(|w| w.has_contract())
    else {
        return; // an extension or legacy widget: not judged
    };
    let wrapped = serde_json::json!({ "Properties": properties });
    if widget.function {
        if let Some((can_source, can_target, _)) = function_property(&wrapped, type_id, property) {
            if !(if source { can_source } else { can_target }) {
                problems.push(direction(binding, id, property, source));
            }
            return;
        }
    }
    // A navigation function's other properties are its target's parameters, which the
    // contained-mashups rules judge; numbered properties (Tab2Visible) are in the defaults.
    // A collection's other properties are its cell mashup's parameters; a toolbar makes
    // `<action>_Disabled` per configured action.
    let own = widget.property(property).is_some();
    if !own
        && (matches!(type_id, "navigationfunction" | "ptcscollection")
            || widget.defaults.contains_key(property)
            || property.contains('_'))
    {
        return;
    }
    match widget.property(property) {
        None => problems.push(problem(
            "unknown-property",
            format!(
                "binding {binding} names {id}.{property}, which {id} ({type_id}) does not have"
            ),
        )),
        Some(contract)
            if !(if source {
                contract.source
            } else {
                contract.target
            }) =>
        {
            problems.push(direction(binding, id, property, source))
        }
        Some(_) => {}
    }
}

fn direction(binding: &str, id: &str, property: &str, source: bool) -> Problem {
    problem(
        "direction",
        format!(
            "binding {binding} {} {id}.{property}, which is not a binding {}",
            if source { "reads from" } else { "writes into" },
            if source { "source" } else { "target" }
        ),
    )
}

/// An event's trigger or handler on a widget: the widget exists and, when the catalogue knows it,
/// has that event or service.
fn check_ui_member(
    widgets_by_id: &BTreeMap<String, Value>,
    event: &str,
    id: &str,
    member: &str,
    trigger: bool,
    problems: &mut Vec<Problem>,
) {
    let Some(properties) = widgets_by_id.get(id) else {
        problems.push(problem(
            "unknown-widget",
            format!("event {event} names widget {id}, which the mashup does not have"),
        ));
        return;
    };
    let type_id = properties["Type"].as_str().unwrap_or_default();
    let Some(widget) = widgets::catalog()
        .by_type(type_id)
        .filter(|w| w.has_contract())
    else {
        return;
    };
    let known: BTreeSet<&str> = if trigger {
        widget.events.iter().flatten().map(String::as_str).collect()
    } else {
        widget
            .services
            .iter()
            .flatten()
            .map(String::as_str)
            .collect()
    };
    // Not judged: a widget that declares nothing, Composer's own refresh on a container, the
    // reset every input widget has, and the events a widget makes per configured action
    // (`<action>_Clicked` on a toolbar or menu button, `CellMenu:<item>` on a collection).
    let dynamic = trigger && (member.contains('_') || member.contains(':'));
    let shared = !trigger && matches!(member, "ResetToDefaultValue" | "ResetInputsToDefaultValue");
    if known.is_empty()
        || known.contains(member)
        || dynamic
        || shared
        || (type_id == "mashupcontainer" && member == "Refresh")
    {
        return;
    }
    problems.push(problem(
        if trigger {
            "unknown-event"
        } else {
            "unknown-service"
        },
        format!(
            "event {event} {} {id}.{member}, which {id} ({type_id}) does not have (it has {})",
            if trigger { "is triggered by" } else { "runs" },
            known.into_iter().collect::<Vec<_>>().join(", ")
        ),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn content(widgets: Value, bindings: Value, events: Value, index: u64) -> String {
        json!({
            "UI": {"Properties": {"Id": "mashup-root", "Type": "mashup", "ResponsiveLayout": true, "id_index": index},
                "Widgets": [{"Properties": {"Id": "flexcontainer-2", "Type": "flexcontainer", "DisplayName": "center"},
                    "Widgets": widgets}]},
            "Data": {"Things_Acme.M": {"DataName": "Things_Acme.M", "EntityName": "Acme.M", "EntityType": "Things",
                "Id": "d1", "Services": [{"Name": "Get", "Target": "Get"}]}},
            "DataBindings": bindings,
            "Events": events,
            "mashupType": "mashup"
        })
        .to_string()
    }

    fn widget(type_id: &str, id: &str) -> Value {
        json!({"Properties": {"Id": id, "Type": type_id, "DisplayName": id}, "Widgets": []})
    }

    fn rules(text: &str) -> Vec<String> {
        let mut problems = Vec::new();
        check_content(text, &mut problems);
        problems.into_iter().map(|p| p.rule).collect()
    }

    #[test]
    fn a_clean_mashup_has_no_problems() {
        let text = content(
            json!([
                widget("ptcsgrid", "ptcsgrid-3"),
                widget("ptcsbutton", "ptcsbutton-4")
            ]),
            json!([{"Id": "b1", "PropertyMaps": [{"SourceProperty": "", "TargetProperty": "Data"}],
                "SourceArea": "Data", "SourceSection": "Things_Acme.M", "SourceId": "Get",
                "TargetArea": "UI", "TargetSection": "", "TargetId": "ptcsgrid-3"}]),
            json!([{"Id": "e1", "EventTriggerArea": "UI", "EventTriggerId": "ptcsbutton-4", "EventTriggerEvent": "Clicked",
                "EventHandlerArea": "Data", "EventHandlerId": "Things_Acme.M", "EventHandlerService": "Get"}]),
            4,
        );
        assert!(rules(&text).is_empty(), "{:?}", rules(&text));
    }

    #[test]
    fn each_rule_finds_its_problem() {
        let text = content(
            json!([
                widget("ptcsgrid", "ptcsgrid-3"),
                widget("ptcsbutton", "ptcsbutton-9")
            ]),
            json!([
                {"Id": "two-maps", "PropertyMaps": [{"SourceProperty": "", "TargetProperty": "Data"}, {"SourceProperty": "", "TargetProperty": "Data"}],
                    "SourceArea": "Data", "SourceSection": "Things_Acme.M", "SourceId": "Get", "TargetArea": "UI", "TargetId": "ptcsgrid-3"},
                {"Id": "no-call", "PropertyMaps": [{"SourceProperty": "", "TargetProperty": "Data"}],
                    "SourceArea": "Data", "SourceSection": "Things_Acme.M", "SourceId": "Nope", "TargetArea": "UI", "TargetId": "ptcsgrid-3"},
                {"Id": "no-widget", "PropertyMaps": [{"SourceProperty": "", "TargetProperty": "Data"}],
                    "SourceArea": "Data", "SourceSection": "Things_Acme.M", "SourceId": "Get", "TargetArea": "UI", "TargetId": "ptcsgrid-77"},
                {"Id": "no-property", "PropertyMaps": [{"SourceProperty": "", "TargetProperty": "Nonsense"}],
                    "SourceArea": "Data", "SourceSection": "Things_Acme.M", "SourceId": "Get", "TargetArea": "UI", "TargetId": "ptcsgrid-3"},
                {"Id": "wrong-way", "PropertyMaps": [{"SourceProperty": "Data", "TargetProperty": "Label"}],
                    "SourceArea": "UI", "SourceId": "ptcsgrid-3", "TargetArea": "UI", "TargetId": "ptcsbutton-9"}
            ]),
            json!([
                {"Id": "bad-event", "EventTriggerArea": "UI", "EventTriggerId": "ptcsbutton-9", "EventTriggerEvent": "Exploded",
                    "EventHandlerArea": "Data", "EventHandlerId": "Things_Acme.M", "EventHandlerService": "Get"},
                {"Id": "bad-service", "EventTriggerArea": "UI", "EventTriggerId": "ptcsbutton-9", "EventTriggerEvent": "Clicked",
                    "EventHandlerArea": "UI", "EventHandlerId": "ptcsgrid-3", "EventHandlerService": "Explode"}
            ]),
            4,
        );
        let found = rules(&text);
        for rule in [
            "several-property-maps",
            "missing-call",
            "unknown-widget",
            "unknown-property",
            "direction",
            "unknown-event",
            "unknown-service",
        ] {
            assert!(
                found.iter().any(|r| r == rule),
                "{rule} not found in {found:?}"
            );
        }
        assert!(rules("{not json").contains(&"structure".to_string()));
        let empty = content(
            json!([widget("ptcsgrid", "ptcsgrid-3")]),
            json!([{"Id": "b", "PropertyMaps": [], "SourceArea": "Data", "SourceSection": "Things_Acme.M",
                "SourceId": "Get", "TargetArea": "UI", "TargetId": "ptcsgrid-3"}]),
            json!([]),
            3,
        );
        assert_eq!(rules(&empty), ["no-property-map"]);
        // Not problems: dynamic sources with no entity, numbered properties, action events, resets.
        let allowed = json!({
            "UI": {"Properties": {"Id": "mashup-root", "Type": "mashup", "id_index": 2},
                "Widgets": [{"Properties": {"Id": "flexcontainer-2", "Type": "flexcontainer"}, "Widgets": [
                    widget("ptcstabset", "ptcstabset-3"), widget("ptcstoolbar", "ptcstoolbar-4"),
                    widget("ptcsdatepicker", "ptcsdatepicker-5")]}]},
            "Data": {"DynamicThingShapes_Acme.S": {"EntityName": "", "EntityType": "DynamicThingShapes",
                "Services": [{"Name": "List"}]}},
            "DataBindings": [{"Id": "b", "PropertyMaps": [{"SourceProperty": "Visible", "TargetProperty": "Tab2Visible"}],
                "SourceArea": "Data", "SourceSection": "DynamicThingShapes_Acme.S", "SourceId": "List",
                "TargetArea": "UI", "TargetId": "ptcstabset-3"}],
            "Events": [{"Id": "e", "EventTriggerArea": "UI", "EventTriggerId": "ptcstoolbar-4", "EventTriggerEvent": "ViewB_Clicked",
                "EventHandlerArea": "UI", "EventHandlerId": "ptcsdatepicker-5", "EventHandlerService": "ResetToDefaultValue"}],
            "mashupType": "mashup"
        })
        .to_string();
        assert!(rules(&allowed).is_empty(), "{:?}", rules(&allowed));
        let twice = content(
            json!([
                widget("ptcsbutton", "ptcsbutton-3"),
                widget("ptcsbutton", "ptcsbutton-3")
            ]),
            json!([]),
            json!([]),
            3,
        );
        assert_eq!(
            rules(&twice),
            ["structure"],
            "duplicate ids are refused by parse"
        );
    }
}
