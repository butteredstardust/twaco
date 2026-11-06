//! `twaco mashup validate`: what in a mashup's content ThingWorx would load but not honour, or
//! what the next edit would trip over. Read-only, from the repository alone.

use crate::core::config::Solution;
use crate::core::mashup;
use crate::core::mashup::authoring::function_property;
use crate::core::mashup::edit::ROOT_ID;
use crate::core::{bindings, contained_check, types, widgets, workspace};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Advisory,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Problem {
    pub rule: String,
    pub message: String,
    pub severity: Severity,
}

#[derive(Debug, Serialize)]
pub struct MashupReport {
    pub mashup: String,
    pub file: String,
    pub problems: Vec<Problem>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProblemCounts {
    pub problems: usize,
    pub errors: usize,
    pub advisories: usize,
}

/// Count every validation finding, separating failures from working-mashup advisories.
pub fn problem_counts(reports: &[MashupReport]) -> ProblemCounts {
    reports.iter().flat_map(|report| &report.problems).fold(
        ProblemCounts::default(),
        |mut counts, problem| {
            counts.problems += 1;
            match problem.severity {
                Severity::Error => counts.errors += 1,
                Severity::Advisory => counts.advisories += 1,
            }
            counts
        },
    )
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
    let discovered = workspace::discover(solution);
    validate_in(solution, name, &discovered)
}

/// [`validate`] over a scan already made.
pub(crate) fn validate_in(
    solution: &Solution,
    name: Option<&str>,
    discovered: &workspace::Discovery,
) -> Result<Vec<MashupReport>, ValidateError> {
    let files: Vec<_> = discovered
        .entities
        .iter()
        .filter(|file| file.info.collection == "Mashups")
        .filter(|file| name.is_none_or(|name| file.info.name == name))
        .cloned()
        .collect();
    if let (Some(name), true) = (name, files.is_empty()) {
        return Err(ValidateError::Unknown(name.to_string()));
    }
    let drift = contained_check::run_in(solution, discovered);
    // Load this once: the bindings gate and the advisory below judge the same model and cached
    // platform contracts, without contacting a server.
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let binding_report =
        bindings::run_in_with_platform(solution, discovered, &model, platform.as_ref());
    let resolver = bindings::Resolver::new(&model, platform.as_ref());
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
            Ok(content) => {
                check_content(&content, &mut problems);
                if let Ok(value) = serde_json::from_str::<Value>(&content) {
                    chart_axis_problems(&file.info.name, &value, &resolver, &mut problems);
                    collection_problems(&value, discovered, &resolver, &mut problems);
                    state_format_problems(&value, discovered, &mut problems);
                }
            }
        }
        for found in binding_report
            .problems
            .iter()
            .filter(|p| p.file == relative)
        {
            push_unique(&mut problems, problem(&found.rule, found.message.clone()));
        }
        for found in drift.problems.iter().filter(|p| p.file == relative) {
            push_unique(&mut problems, problem(&found.rule, found.message.clone()));
        }
        reports.push(MashupReport {
            mashup: file.info.name.clone(),
            file: relative,
            problems,
        });
    }
    Ok(reports)
}

/// Collection maps are a second use of service-result fields. They are advisory here rather than
/// in the bindings gate: Composer accepts an unknown cell parameter and Runtime ignores it.
fn collection_problems(
    content: &Value,
    discovered: &workspace::Discovery,
    resolver: &bindings::Resolver<'_>,
    problems: &mut Vec<Problem>,
) {
    let Ok(document) = mashup::edit::parse(&mashup::to_sidecar(content)) else {
        return;
    };
    let sections = bindings::data_sections(content);
    for widget in document.widgets() {
        if widget.type_id != "ptcscollection" {
            continue;
        }
        let properties = &document.at(&widget.path)["Properties"];
        let mappings = properties["MashupPropertyBinding"].as_object();
        let Some(mappings) = mappings else {
            continue;
        };
        let binding = bindings::whole_data_bindings(content)
            .into_iter()
            .find(|binding| binding.widget == widget.id);
        let shape = binding
            .and_then(|binding| {
                let (section, call) =
                    bindings::service_at(&sections, &binding.section, &binding.call)?;
                resolver
                    .service(section, &call.target)
                    .service
                    .and_then(|service| {
                        service
                            .result
                            .base_type
                            .eq_ignore_ascii_case("INFOTABLE")
                            .then(|| service.result.data_shape.clone())
                            .flatten()
                    })
            })
            .and_then(|name| resolver.checkable_shape(&name));
        if let Some(shape) = shape {
            for field in mappings.keys() {
                if !shape.fields.iter().any(|known| known.name == *field) {
                    push_unique(problems, problem("unknown-field-property", format!(
                        "{}.MashupPropertyBinding names field {field}, but its DataShape {} has no field {field}", widget.id, shape.name
                    )));
                }
            }
            if let Some(uid) = properties["UIDField"]
                .as_str()
                .filter(|uid| !uid.is_empty())
            {
                if !shape.fields.iter().any(|known| known.name == uid) {
                    push_unique(
                        problems,
                        problem(
                            "unknown-field-property",
                            format!(
                                "{}.UIDField is {uid}, but its DataShape {} has no field {uid}",
                                widget.id, shape.name
                            ),
                        ),
                    );
                }
            }
        }
        let Some(cell) = properties["Mashup"]
            .as_str()
            .filter(|cell| !cell.is_empty())
        else {
            continue;
        };
        let Some(file) = discovered
            .entities
            .iter()
            .find(|file| file.info.collection == "Mashups" && file.info.name == cell)
        else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&file.path) else {
            continue;
        };
        let Ok(parameters) = crate::core::mashup::describe::mashup_parameters(&bytes) else {
            continue;
        };
        for parameter in mappings.values().filter_map(Value::as_str) {
            if !parameters.iter().any(|known| known.name == parameter) {
                push_unique(problems, problem("unknown-cell-parameter", format!(
                    "{}.MashupPropertyBinding maps to {cell}.{parameter}, which has no such parameter; Runtime ignores it", widget.id
                )));
            }
        }
    }
}

/// The `check` gate's name for the content rules.
pub(crate) const GATE: &str = "mashup content";

/// The content rules over every mashup, for the `check` gate: each mashup's file and its
/// problems. The contained-mashup rules are their own gate, so they are not repeated here.
#[cfg(test)]
pub(crate) fn content_problems(solution: &Solution) -> (usize, Vec<(String, Problem)>) {
    let discovered = workspace::discover(solution);
    content_problems_in(solution, &discovered)
}

/// [`content_problems`] over a scan already made.
pub(crate) fn content_problems_in(
    solution: &Solution,
    discovered: &workspace::Discovery,
) -> (usize, Vec<(String, Problem)>) {
    let files: Vec<_> = discovered
        .entities
        .iter()
        .filter(|file| file.info.collection == "Mashups")
        .cloned()
        .collect();
    let mut found = Vec::new();
    for file in &files {
        let relative = file
            .path
            .strip_prefix(&solution.root)
            .unwrap_or(&file.path)
            .to_string_lossy()
            .replace('\\', "/");
        let mut problems = Vec::new();
        match std::fs::read(&file.path) {
            Err(error) => problems.push(problem("unreadable", error.to_string())),
            Ok(bytes) => {
                // An empty placeholder, or JSON the project gate already reports as unparsable,
                // is no content to judge; nor is an object without a UI yet, as `{}` is.
                let payload = crate::core::validate::mashup_payload(&bytes).unwrap_or_default();
                let judged = serde_json::from_str::<Value>(payload.trim())
                    .is_ok_and(|value| value.get("UI").is_some());
                if judged {
                    check_content(payload.trim(), &mut problems);
                }
            }
        }
        found.extend(problems.into_iter().map(|p| (relative.clone(), p)));
    }
    (files.len(), found)
}

fn problem(rule: &str, message: String) -> Problem {
    Problem {
        rule: rule.to_string(),
        message,
        severity: severity(rule),
    }
}

/// Rules that describe tolerated Composer state rather than a broken mashup.
fn severity(rule: &str) -> Severity {
    match rule {
        "chart-axis-type" | "unknown-cell-parameter" => Severity::Advisory,
        _ => Severity::Error,
    }
}

fn push_unique(problems: &mut Vec<Problem>, found: Problem) {
    if !problems
        .iter()
        .any(|present| present.rule == found.rule && present.message == found.message)
    {
        problems.push(found);
    }
}

/// Chart axis kinds are a promise about values rather than a type contract: strings such as
/// `2021` work, while strings such as `Mon` draw nothing. Keep that distinction an advisory.
fn chart_axis_problems(
    mashup: &str,
    content: &Value,
    resolver: &bindings::Resolver<'_>,
    problems: &mut Vec<Problem>,
) {
    let sections = bindings::data_sections(content);
    for (section_name, section) in &sections {
        for call in &section.services {
            let lookup = resolver.service(section, &call.target);
            let Some(service) = lookup.service.as_ref() else {
                continue;
            };
            if !service.result.base_type.eq_ignore_ascii_case("INFOTABLE") {
                continue;
            }
            let Some(shape) = service
                .result
                .data_shape
                .as_deref()
                .and_then(|name| resolver.checkable_shape(name))
            else {
                continue;
            };
            for user in bindings::result_field_users(content, section_name, &call.instance) {
                let (Some(widget), Some(property)) =
                    (user.widget.as_deref(), user.property.as_deref())
                else {
                    continue;
                };
                let Some((setting, axis)) = bindings::chart_axis_type(content, widget, property)
                else {
                    continue;
                };
                if !matches!(axis.as_str(), "number" | "date") {
                    continue;
                }
                let Some(field) = shape.fields.iter().find(|field| field.name == user.field) else {
                    continue;
                };
                if !field.base_type.eq_ignore_ascii_case("STRING") {
                    continue;
                }
                let value_kind = if axis == "date" { "dates" } else { "numbers" };
                let fix = if setting == "XAxisType" {
                    format!("twaco mashup set {mashup} {widget} --prop XAxisType=label --apply")
                } else {
                    format!(
                        "twaco mashup set {mashup} {widget} --prop DataFieldType=string --apply"
                    )
                };
                push_unique(
                    problems,
                    problem(
                        "chart-axis-type",
                        format!(
                            "{mashup}: {widget}.{property} is STRING field {} from {}.{} with {setting}={axis}; Runtime draws only values that read as {value_kind}; use `{fix}`",
                            user.field, section.entity, call.target
                        ),
                    ),
                );
            }
        }
    }
}

/// Every content rule, for one mashup's `mashupContent`.
pub(crate) fn check_content(content: &str, problems: &mut Vec<Problem>) {
    let document = match mashup::edit::parse(content) {
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
    for (key, section) in value["Data"].as_object().into_iter().flatten() {
        let kind = section["EntityType"].as_str().unwrap_or_default();
        // ThingWorx has more source kinds than twaco authors (Networks, Resources, ...): only a
        // source that names none is a problem.
        if kind.is_empty() {
            problems.push(problem(
                "unknown-entity-type",
                format!("data source {key} has no EntityType"),
            ));
        }
        if section["DataName"].as_str() != Some(key) {
            problems.push(problem(
                "data-name-mismatch",
                format!("data source {key} has DataName {}", section["DataName"]),
            ));
        }
        let mut names = BTreeSet::new();
        for call in section["Services"].as_array().into_iter().flatten() {
            if let Some(name) = call["Name"].as_str() {
                if !names.insert(name) {
                    problems.push(problem(
                        "duplicate-call-name",
                        format!("data source {key} has two calls named {name}"),
                    ));
                }
            }
        }
        let entity_bindings = value["DataBindings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|binding| {
                binding["TargetArea"] == "Data"
                    && binding["TargetSection"] == *key
                    && binding["TargetId"] == "EntityName"
            })
            .count();
        // A dynamic section nothing wires is dead but harmless (real repositories keep some);
        // one whose calls run or feed something cannot work without its entity.
        let wired = value["DataBindings"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|binding| {
                (binding["SourceArea"] == "Data" && binding["SourceSection"] == *key)
                    || (binding["TargetArea"] == "Data"
                        && binding["TargetSection"] == *key
                        && binding["TargetId"] != "EntityName")
            })
            || value["Events"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|event| {
                    (event["EventHandlerArea"] == "Data" && event["EventHandlerId"] == *key)
                        || (event["EventTriggerArea"] == "Data"
                            && event["EventTriggerSection"] == *key)
                });
        if matches!(kind, "DynamicThingTemplates" | "DynamicThingShapes") {
            if entity_bindings == 0 && wired {
                problems.push(problem(
                    "unbound-dynamic-entity",
                    format!("dynamic data source {key} has no EntityName binding"),
                ));
            }
            if entity_bindings > 1 {
                problems.push(problem(
                    "multiple-dynamic-entity-bindings",
                    format!("dynamic data source {key} has {entity_bindings} EntityName bindings"),
                ));
            }
        }
        if matches!(kind, "Things" | "ThingTemplates") && entity_bindings > 0 {
            problems.push(problem(
                "entity-binding-into-static-section",
                format!("static data source {key} has an EntityName binding"),
            ));
        }
    }
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
    // A chart with MultipleDataSources takes one data source per series, DataSource1 to
    // DataSource<NumberOfSeries>, each an infotable it reads.
    if let Some(series) = property.strip_prefix("DataSource").and_then(|n| {
        n.parse::<u64>()
            .ok()
            .filter(|series| series.to_string() == n)
    }) {
        let separate = properties["MultipleDataSources"].as_bool() == Some(true);
        let count = match &properties["NumberOfSeries"] {
            Value::String(text) => text.trim().parse().unwrap_or(0),
            other => other.as_u64().unwrap_or(0),
        };
        if separate && (1..=count).contains(&series) {
            if source {
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

fn state_format_problems(
    content: &Value,
    discovered: &workspace::Discovery,
    problems: &mut Vec<Problem>,
) {
    let Ok(document) = mashup::edit::parse(&mashup::to_sidecar(content)) else {
        return;
    };
    for widget in document.widgets() {
        let properties = &document.at(&widget.path)["Properties"];
        let mut formats = Vec::new();
        if let Some(catalogue) = widgets::catalog().by_type(&widget.type_id) {
            for (name, contract) in catalogue
                .properties
                .iter()
                .flat_map(|properties| properties.iter())
            {
                if contract.base_type.as_deref() == Some("STATEFORMATTING") {
                    formats.push((format!("{}.{}", widget.id, name), &properties[name]));
                }
            }
        }
        if widget.type_id == "ptcsgrid" {
            for column in properties["ColumnFormat"]["formatInfo"]
                .as_array()
                .into_iter()
                .flatten()
            {
                formats.push((
                    format!(
                        "{} column {}",
                        widget.id,
                        column["FieldName"].as_str().unwrap_or("?")
                    ),
                    &column["FormatOptions"]["formatInfo"],
                ));
            }
        }
        for (label, format) in formats {
            let Some(name) = format["StateDefinition"]
                .as_str()
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            let Some(file) = discovered
                .entities
                .iter()
                .find(|file| file.info.collection == "StateDefinitions" && file.info.name == name)
            else {
                push_unique(
                    problems,
                    problem(
                        "unknown-state-definition",
                        format!(
                            "{label} names StateDefinition {name}, which is not in the solution"
                        ),
                    ),
                );
                continue;
            };
            let expected = std::fs::read(&file.path)
                .ok()
                .and_then(|bytes| mashup::authoring::state_format::read_definition(&bytes).ok())
                .and_then(|definition| {
                    mashup::authoring::state_format::inline(&definition, name, "").ok()
                });
            if let Some(expected) = expected {
                if format["StateFormats"] != expected["StateFormats"]
                    || format["StateDefinitionType"] != expected["StateDefinitionType"]
                {
                    push_unique(problems, Problem { rule: "state-format-drift".into(), severity: Severity::Advisory,
                        message: format!("{label} has a snapshot that differs from StateDefinition {name}; use mashup state-format set to refresh it") });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Solution;
    use serde_json::json;
    use std::path::Path;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn data_solution(content: &str, shape: Option<&str>) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "twaco.toml", "[[project]]\nname = \"Acme\"\ncollections = [\"Things\", \"DataShapes\", \"Mashups\"]\n");
        write(
            dir.path(),
            "Things/Acme.M.xml",
            r#"<Entities><Things><Thing name="Acme.M"><ThingShape><ServiceDefinitions><ServiceDefinition name="Get"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Rows"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
        );
        if let Some(shape) = shape {
            write(dir.path(), "DataShapes/Acme.Rows.xml", shape);
        }
        write(
            dir.path(),
            "Mashups/Acme.View.xml",
            &format!(
                r#"<Entities><Mashups><Mashup name="Acme.View"><mashupContent><![CDATA[{content}]]></mashupContent></Mashup></Mashups></Entities>"#
            ),
        );
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        (dir, solution)
    }

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
        // Not problems: numbered properties, action events, resets, a bound dynamic source.
        let allowed = json!({
            "UI": {"Properties": {"Id": "mashup-root", "Type": "mashup", "id_index": 2},
                "Widgets": [{"Properties": {"Id": "flexcontainer-2", "Type": "flexcontainer"}, "Widgets": [
                    widget("ptcstabset", "ptcstabset-3"), widget("ptcstoolbar", "ptcstoolbar-4"),
                    widget("ptcsdatepicker", "ptcsdatepicker-5")]}]},
            "Data": {"DynamicThingShapes_Acme.S": {"DataName": "DynamicThingShapes_Acme.S", "EntityName": "Acme.S",
                "EntityType": "DynamicThingShapes", "Services": [{"Name": "List"}]}},
            "DataBindings": [{"Id": "b", "PropertyMaps": [{"SourceProperty": "Visible", "TargetProperty": "Tab2Visible"}],
                "SourceArea": "Data", "SourceSection": "DynamicThingShapes_Acme.S", "SourceId": "List",
                "TargetArea": "UI", "TargetId": "ptcstabset-3"},
                {"Id": "e1", "PropertyMaps": [{"SourceProperty": "Manager", "TargetProperty": "value"}],
                "SourceArea": "Mashup", "SourceId": "mashup-root", "TargetArea": "Data",
                "TargetSection": "DynamicThingShapes_Acme.S", "TargetId": "EntityName"}],
            "Events": [{"Id": "e", "EventTriggerArea": "UI", "EventTriggerId": "ptcstoolbar-4", "EventTriggerEvent": "ViewB_Clicked",
                "EventHandlerArea": "UI", "EventHandlerId": "ptcsdatepicker-5", "EventHandlerService": "ResetToDefaultValue"}],
            "mashupType": "mashup"
        })
        .to_string();
        assert_eq!(
            rules(&allowed),
            Vec::<String>::new(),
            "{:?}",
            rules(&allowed)
        );
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

    #[test]
    fn a_chart_with_separate_data_sources_takes_one_per_series() {
        let chart = |separate: bool| {
            json!([{"Properties": {"Id": "ptcschartline-3", "Type": "ptcschartline", "DisplayName": "c",
                "MultipleDataSources": separate, "NumberOfSeries": 2}, "Widgets": []}])
        };
        let bind = |property: &str| {
            json!([{"Id": "b1", "PropertyMaps": [{"SourceProperty": "", "TargetProperty": property}],
                "SourceArea": "Data", "SourceSection": "Things_Acme.M", "SourceId": "Get",
                "TargetArea": "UI", "TargetSection": "", "TargetId": "ptcschartline-3"}])
        };
        for property in ["DataSource1", "DataSource2"] {
            assert!(rules(&content(chart(true), bind(property), json!([]), 4)).is_empty());
        }
        // Beyond NumberOfSeries, or without MultipleDataSources, the property does not exist.
        assert_eq!(
            rules(&content(chart(true), bind("DataSource3"), json!([]), 4)),
            ["unknown-property"]
        );
        assert_eq!(
            rules(&content(chart(false), bind("DataSource1"), json!([]), 4)),
            ["unknown-property"]
        );
        // A series count stored as text counts; a name ThingWorx would not write does not.
        let text_count = json!([{"Properties": {"Id": "ptcschartline-3", "Type": "ptcschartline", "DisplayName": "c",
            "MultipleDataSources": true, "NumberOfSeries": "2"}, "Widgets": []}]);
        assert!(rules(&content(text_count, bind("DataSource2"), json!([]), 4)).is_empty());
        assert_eq!(
            rules(&content(chart(true), bind("DataSource01"), json!([]), 4)),
            ["unknown-property"]
        );
    }

    #[test]
    fn validate_includes_bound_fields_and_advises_only_value_dependent_chart_axes() {
        let shape = r#"<Entities><DataShapes><DataShape name="Acme.Rows"><FieldDefinitions><FieldDefinition name="text" baseType="STRING"/><FieldDefinition name="count" baseType="INTEGER"/><FieldDefinition name="when" baseType="DATETIME"/></FieldDefinitions></DataShape></DataShapes></Entities>"#;
        let chart = |field: &str, axis: Option<&str>| {
            let mut chart = json!({"Properties": {"Id": "ptcschartline-3", "Type": "ptcschartline", "DisplayName": "c", "XAxisField": field}, "Widgets": []});
            if let Some(axis) = axis {
                chart["Properties"]["XAxisType"] = json!(axis);
            }
            content(
                json!([chart]),
                json!([{"Id": "b", "SourceArea": "Data", "SourceSection": "Things_Acme.M", "SourceId": "Get", "TargetArea": "UI", "TargetId": "ptcschartline-3", "PropertyMaps": [{"SourcePropertyType": "InfoTable", "TargetProperty": "Data"}]}]),
                json!([]),
                4,
            )
        };
        let (_dir, solution) = data_solution(&chart("text", Some("number")), Some(shape));
        let report = validate(&solution, Some("Acme.View")).unwrap();
        assert!(
            report[0]
                .problems
                .iter()
                .any(|problem| problem.rule == "chart-axis-type"
                    && problem.severity == Severity::Advisory
                    && problem.message.contains("XAxisType=label")),
            "{:?}",
            report[0].problems
        );
        assert!(!crate::core::bindings::run(&solution)
            .problems
            .iter()
            .any(|problem| problem.rule == "chart-axis-type"));
        for (field, axis) in [
            ("count", Some("number")),
            ("when", Some("date")),
            ("text", Some("label")),
            ("text", None),
        ] {
            let (_dir, solution) = data_solution(&chart(field, axis), Some(shape));
            let report = validate(&solution, Some("Acme.View")).unwrap();
            assert!(
                !report[0]
                    .problems
                    .iter()
                    .any(|problem| problem.rule == "chart-axis-type"),
                "{field}/{axis:?}: {:?}",
                report[0].problems
            );
        }
        let (_dir, solution) = data_solution(&chart("text", Some("number")), None);
        let report = validate(&solution, Some("Acme.View")).unwrap();
        assert!(!report[0]
            .problems
            .iter()
            .any(|problem| problem.rule == "chart-axis-type"));

        let (_dir, solution) = data_solution(&chart("missing", Some("label")), Some(shape));
        let report = validate(&solution, Some("Acme.View")).unwrap();
        assert!(
            report[0]
                .problems
                .iter()
                .any(|problem| problem.rule == "unknown-field-property"),
            "{:?}",
            report[0].problems
        );
    }

    #[test]
    fn only_tolerated_composer_state_is_advisory() {
        let reports = [MashupReport {
            mashup: "Acme.View".to_string(),
            file: "Mashups/Acme.View.xml".to_string(),
            problems: vec![
                problem("chart-axis-type", "axis".to_string()),
                problem("unknown-cell-parameter", "parameter".to_string()),
                problem("unknown-field", "field".to_string()),
            ],
        }];
        assert_eq!(reports[0].problems[0].severity, Severity::Advisory);
        assert_eq!(reports[0].problems[1].severity, Severity::Advisory);
        assert_eq!(reports[0].problems[2].severity, Severity::Error);
        assert_eq!(
            problem_counts(&reports),
            ProblemCounts {
                problems: 3,
                errors: 1,
                advisories: 2,
            }
        );
    }

    #[test]
    fn data_sources_name_themselves_and_wired_dynamic_ones_have_one_entity() {
        let rules = |data: Value, bindings: Value| {
            let mut problems = Vec::new();
            let content = json!({"UI": {"Properties": {"Id": "mashup-root", "Type": "mashup", "id_index": 1}, "Widgets": []},
                "Data": data, "DataBindings": bindings, "Events": [], "mashupType": "mashup"});
            check_content(&content.to_string(), &mut problems);
            let mut found: Vec<String> = problems.into_iter().map(|p| p.rule.to_string()).collect();
            found.sort();
            found
        };
        let section = |key: &str, kind: &str, calls: Value| json!({key: {"DataName": key, "EntityName": "Acme.S", "EntityType": kind, "Services": calls}});
        let entity = |section: &str| {
            json!({"Id": "e", "PropertyMaps": [{"SourceProperty": "Manager", "TargetProperty": "value"}],
                "SourceArea": "Mashup", "SourceId": "mashup-root", "TargetArea": "Data",
                "TargetSection": section, "TargetId": "EntityName"})
        };
        let feeds = |section: &str| {
            json!({"Id": "f", "PropertyMaps": [{"SourceProperty": "x", "TargetProperty": "y"}],
                "SourceArea": "Data", "SourceSection": section, "SourceId": "List", "TargetArea": "Mashup",
                "TargetId": "mashup-root"})
        };
        let key = "DynamicThingShapes_Acme.S";
        let one = json!([{"Name": "List", "Target": "List"}]);
        // Unbound and unused: dead, not reported. Unbound and wired: reported.
        assert!(rules(section(key, "DynamicThingShapes", one.clone()), json!([])).is_empty());
        assert_eq!(
            rules(
                section(key, "DynamicThingShapes", one.clone()),
                json!([feeds(key)])
            ),
            ["unbound-dynamic-entity"]
        );
        assert_eq!(
            rules(
                section(key, "DynamicThingShapes", one.clone()),
                json!([feeds(key), entity(key), entity(key)])
            ),
            ["multiple-dynamic-entity-bindings"]
        );
        let fixed = "Things_Acme.S";
        assert_eq!(
            rules(
                section(fixed, "Things", one.clone()),
                json!([entity(fixed)])
            ),
            ["entity-binding-into-static-section"]
        );
        // A kind twaco does not author is still a kind.
        assert!(rules(
            section("Networks_Acme.Net", "Networks", one.clone()),
            json!([])
        )
        .is_empty());
        let mut renamed = section(
            key,
            "DynamicThingShapes",
            json!([{"Name": "List"}, {"Name": "List"}]),
        );
        renamed[key]["DataName"] = json!("Other");
        renamed[key]["EntityType"] = json!("");
        assert_eq!(
            rules(renamed, json!([])),
            [
                "data-name-mismatch",
                "duplicate-call-name",
                "unknown-entity-type"
            ]
        );
    }
}
