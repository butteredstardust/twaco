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
                name_the_mashup(&mut problems, &file.info.name);
                if let Ok(value) = serde_json::from_str::<Value>(&content) {
                    scalar_result_problems(&value, &resolver, &mut problems);
                    chart_axis_problems(&file.info.name, &value, &resolver, &mut problems);
                    collection_problems(&value, discovered, &resolver, &mut problems);
                    state_format_problems(&value, discovered, &mut problems);
                    style_problems(&value, &mut problems);
                    cell_menu_problems(&value, discovered, &mut problems);
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
                    name_the_mashup(&mut problems, &file.info.name);
                }
            }
        }
        found.extend(problems.into_iter().map(|p| (relative.clone(), p)));
    }
    (files.len(), found)
}

/// A finding that quotes a command writes `<mashup>`; the caller knows which mashup it judged.
fn name_the_mashup(problems: &mut [Problem], name: &str) {
    for found in problems {
        found.message = found.message.replace("<mashup>", name);
    }
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
        "selected-rows-without-selection"
        | "chart-axis-type"
        | "unknown-cell-parameter"
        | "mixed-label-alignment"
        | "responsive-leaf-size"
        | "wrapped-row-clipping"
        | "wrapped-content-clipping"
        | "remainder-not-last"
        | "slider-button-alignment"
        | "style-important"
        | "style-placement-property"
        | "style-unknown-part"
        | "layout-siblings"
        | "layout-slot-size" => Severity::Advisory,
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
    let generated_v1 = document.value()["twacoPageRules"] == "v1";
    for widget in document.widgets() {
        let row = document.at(&widget.path);
        let props = &row["Properties"];
        if widget.type_id == "flexcontainer"
            && props["flex-wrap"] == "wrap"
            && (props["flex-size"] == "fixed"
                || (!props["flex-max-height"].is_null() && props["flex-max-height"] != ""))
        {
            problems.push(problem(
                "wrapped-row-clipping",
                format!(
                    "{}: a wrapped second line is clipped; use flex-size range with no max height; on 10.2.1 set height:auto, min-height:0, max-height:none (!important) on the row bounding box, inner .widget-flexcontainer and parents up to the panel, including rows populated after layout",
                    widget.id
                ),
            ));
        }
        let leaf = crate::core::widgets::catalog()
            .by_type(&widget.type_id)
            .is_some_and(|widget| !widget.container);
        if leaf
            && props["FillContainer"] == false
            && mashup::edit::has_leaf_size(props)
            && props["ResponsiveLayout"] == true
        {
            if let Some((_, parent)) = widget.path.split_last() {
                let parent = &document.at(parent)["Properties"];
                if parent["Type"] == "flexcontainer" && parent["ResponsiveLayout"] != false {
                    problems.push(problem(
                        "responsive-leaf-size",
                        format!(
                            "{}: Width/Height are ignored at run time; set ResponsiveLayout=false",
                            widget.id
                        ),
                    ));
                }
            }
        }
        if widget.type_id == "flexcontainer" {
            container_layout_problems(
                row,
                &widget.id,
                generated_v1 && row["twacoLayoutAuthored"] != true,
                problems,
            );
            for found in mashup::why::contradictions(&document, &widget.path, &widget.id) {
                let rule = found.rule.as_deref().unwrap_or("");
                problems.push(problem(
                    if found.name == "siblings" { "layout-siblings" } else { "layout-slot-size" },
                    format!(
                        "{}: layout rule {rule} decides {} {}, but the mashup has {}; `twaco mashup why <mashup> {}` explains it and how to override",
                        widget.id,
                        found.name,
                        found.value,
                        found.current.as_deref().unwrap_or("something else"),
                        widget.id
                    ),
                ));
            }
        }
        let alignment = &row["Properties"]["align-items"];
        if alignment.is_null() || alignment == "flex-start" {
            if let Some((labelled, controls)) = mashup::edit::mixed_label_controls(row) {
                problems.push(problem("mixed-label-alignment", format!(
                    "row {} mixes labelled inputs {} with controls {}; align their input boxes with twaco mashup set <M> {} --prop align-items=flex-end",
                    widget.id, labelled.join(", "), controls.join(", "), widget.id
                )));
            }
        }
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
        let unbind = mashup::describe::unbind_command(value, binding);
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
                "UI" => check_ui_end(
                    &widgets_by_id,
                    (id, &unbind),
                    end,
                    property,
                    source,
                    problems,
                ),
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

/// Advisories about how a container's own size and alignment treat its content.
fn container_layout_problems(row: &Value, id: &str, measured: bool, problems: &mut Vec<Problem>) {
    use mashup::edit::{children, is_remainder, is_spacer, type_of};
    let props = &row["Properties"];
    let kids = children(row);
    if props["flex-direction"] == "column" {
        let last_content = kids.iter().rposition(|kid| {
            !is_remainder(kid) && !is_spacer(kid) && !crate::core::mashup::edit::is_footer(kid)
        });
        for (index, kid) in kids.iter().enumerate() {
            if is_remainder(kid) && last_content.is_some_and(|last| last > index) {
                problems.push(problem("remainder-not-last", format!(
                    "{}: a growing empty container sits above more content in {id}, so it pushes that content to the bottom; move it last",
                    kid["Properties"]["Id"].as_str().unwrap_or("a container")
                )));
            }
        }
    }
    let direction = props["flex-direction"].as_str().unwrap_or("row");
    if matches!(direction, "row" | "") && props["align-items"] == "center" {
        let kinds: Vec<_> = kids
            .iter()
            .filter_map(|kid| {
                let mut widget = kid;
                while type_of(widget) == Some("flexcontainer") && children(widget).len() == 1 {
                    widget = &children(widget)[0];
                }
                type_of(widget)
            })
            .collect();
        if kinds.contains(&"ptcsslider") && kinds.contains(&"ptcsbutton") {
            problems.push(problem("slider-button-alignment", format!(
                "row {id} centres a button on a slider's whole box (label, value, MIN/MAX), so it sits above the track; use align-items=flex-end or offset the button's margin"
            )));
        }
    }
    let capped = props["flex-size"] == "fixed"
        || props["flex-max-height"]
            .as_str()
            .is_some_and(|max| !max.is_empty());
    if capped {
        let authored = !measured
            && !props["DisplayName"]
                .as_str()
                .is_some_and(|name| name.starts_with("container-"));
        if kids
            .iter()
            .any(|kid| wraps_content(kid, authored, props["Width"].as_f64()))
        {
            problems.push(problem("wrapped-content-clipping", format!(
                "{id}: a fixed or capped height clips wrapped text of a property display or multi-line label; use flex-size range with no max height"
            )));
        }
    }
}

/// Whether the widget, or one inside it, shows text that can wrap onto more lines. A label
/// counts only in an authored container: generated rows are measured for their text.
fn wraps_content(widget: &Value, authored: bool, width: Option<f64>) -> bool {
    let properties = &widget["Properties"];
    match properties["Type"].as_str() {
        // Generated rows are measured for what they hold; only an authored cap clips.
        Some("ptcspropertydisplay") => authored,
        Some("ptcslabel") => {
            let text = properties["LabelText"].as_str().unwrap_or("");
            let dynamic = text.is_empty() || text.contains("[[");
            let width = properties["Width"].as_f64().filter(|w| *w > 0.0).or(width);
            authored
                && properties["MultiLine"] == true
                && (dynamic
                    || (text.split_whitespace().count() > 1
                        && !width.is_some_and(|w| text.chars().count() as f64 * 12.0 + 24.0 <= w)))
        }
        _ => mashup::edit::children(widget)
            .iter()
            .any(|kid| wraps_content(kid, authored, width)),
    }
}

/// A binding end on a widget: the widget exists, and a catalogue-known widget allows the property
/// in that direction. Dynamic properties (inputs, a container's or navigation function's
/// parameters) are read from the widget itself.
fn check_ui_end(
    widgets_by_id: &BTreeMap<String, Value>,
    (binding, unbind): (&str, &str),
    id: &str,
    property: &str,
    source: bool,
    problems: &mut Vec<Problem>,
) {
    let Some(properties) = widgets_by_id.get(id) else {
        problems.push(problem(
            "unknown-widget",
            format!(
                "binding {binding} names widget {id}, which the mashup does not have; remove the binding with: {unbind}"
            ),
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
    if let Some(contract) = widgets::dynamic_property(type_id, properties, property, false) {
        if !(if source {
            contract.source
        } else {
            contract.target
        }) {
            problems.push(direction(binding, id, property, source));
        }
        return;
    }
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

/// A collection's `CellMenu:<item>` events: the collection must have a cell menu, and when its
/// StateDefinition is in the repository, the item must be one of its states.
fn cell_menu_problems(
    content: &Value,
    discovered: &workspace::Discovery,
    problems: &mut Vec<Problem>,
) {
    use crate::core::mashup::cell_menu;
    let Ok(document) = mashup::edit::parse(&mashup::to_sidecar(content)) else {
        return;
    };
    for event in content["Events"].as_array().into_iter().flatten() {
        let Some(name) = event["EventTriggerEvent"]
            .as_str()
            .filter(|name| name.starts_with(cell_menu::PREFIX))
        else {
            continue;
        };
        let id = event["EventTriggerId"].as_str().unwrap_or_default();
        let Ok(found) = document.find_exact(id) else {
            continue;
        };
        if found.type_id != "ptcscollection" {
            continue;
        }
        let menu = document.at(&found.path)["Properties"]["CellMenu"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if menu.is_empty() {
            push_unique(
                problems,
                problem(
                    "cell-menu",
                    format!("{id}.{name} fires from a cell menu, but {id} has no CellMenu StateDefinition"),
                ),
            );
            continue;
        }
        let Some(items) = cell_menu::repository_events(discovered, &menu) else {
            continue;
        };
        if !items.iter().any(|item| item == name) {
            push_unique(
                problems,
                problem(
                    "cell-menu",
                    format!(
                        "{id}.{name} is not an item of {menu}, whose cell menu fires {}",
                        if items.is_empty() {
                            "nothing".to_string()
                        } else {
                            items.join(", ")
                        }
                    ),
                ),
            );
        }
    }
}

/// Style settings the runtime is likely to ignore, from the root's StyleProperties.
fn style_problems(content: &Value, problems: &mut Vec<Problem>) {
    let styles = &content["UI"]["Properties"]["StyleProperties"]["data"];
    for (id, entry) in styles.as_object().into_iter().flatten() {
        for (rule, message) in mashup::style_check::entry_notes(id, entry) {
            push_unique(problems, problem(rule, message));
        }
    }
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

fn scalar_result_problems(
    content: &Value,
    resolver: &bindings::Resolver<'_>,
    problems: &mut Vec<Problem>,
) {
    let sections = bindings::data_sections(content);
    for (index, binding) in content["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if binding["SourceArea"] != "Data" {
            continue;
        }
        let section_name = binding["SourceSection"].as_str().unwrap_or_default();
        let call_name = binding["SourceId"].as_str().unwrap_or_default();
        let Some((section, call)) = bindings::service_at(&sections, section_name, call_name) else {
            continue;
        };
        let Some(service) = resolver.service(section, &call.target).service else {
            continue;
        };
        if service.result.base_type.eq_ignore_ascii_case("INFOTABLE") {
            continue;
        }
        for map in binding["PropertyMaps"].as_array().into_iter().flatten() {
            if map["SourcePropertyType"] == "InfoTable"
                || map["SourceProperty"].as_str().is_none_or(str::is_empty)
            {
                let source = if call.instance == call.target {
                    format!("data:{}.{}", section.entity, call.target)
                } else {
                    format!(
                        "data:{}.{}[as {}]",
                        section.entity, call.target, call.instance
                    )
                };
                let id = binding["Id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("DataBindings[{index}]"));
                push_unique(problems, problem("scalar-whole-result", format!(
                    "binding {id} from {source} to {}.{} uses a whole scalar result; use `{source}.result`",
                    binding["TargetId"].as_str().unwrap_or_default(), map["TargetProperty"].as_str().unwrap_or_default()
                )));
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
    fn wrapped_rows_warn_only_when_height_is_capped() {
        for (kind, wrap, size, max, warn) in [
            ("flexcontainer", "wrap", "fixed", "", true),
            ("flexcontainer", "wrap", "range", "80px", true),
            ("flexcontainer", "wrap", "default", "80px", true),
            ("flexcontainer", "wrap", "range", "", false),
            ("flexcontainer", "nowrap", "fixed", "80px", false),
            ("ptcsimage", "wrap", "fixed", "80px", false),
        ] {
            let value = json!({"UI": {"Properties": {"Type": "mashup", "Id": "mashup-root"}, "Widgets": [
                {"Properties": {"Type": kind, "Id": "row-1", "flex-wrap": wrap, "flex-size": size, "flex-max-height": max}}
            ]}});
            let mut found = Vec::new();
            check_content(&value.to_string(), &mut found);
            let warnings: Vec<_> = found
                .iter()
                .filter(|p| p.rule == "wrapped-row-clipping")
                .collect();
            assert_eq!(warnings.len(), usize::from(warn), "{value}");
            if let Some(warning) = warnings.first() {
                assert_eq!(warning.severity, Severity::Advisory);
                assert_eq!(warning.message, "row-1: a wrapped second line is clipped; use flex-size range with no max height; on 10.2.1 set height:auto, min-height:0, max-height:none (!important) on the row bounding box, inner .widget-flexcontainer and parents up to the panel, including rows populated after layout");
            }
        }
    }

    fn layout_rules(root: Value) -> Vec<(String, String)> {
        let value = json!({"UI": {"Properties": {"Type": "mashup", "Id": "mashup-root"}, "Widgets": [root]}});
        let mut found = Vec::new();
        check_content(&value.to_string(), &mut found);
        found.into_iter().map(|p| (p.rule, p.message)).collect()
    }

    fn container_of(id: &str, name: &str, extra: Value, kids: Vec<Value>) -> Value {
        let mut properties = json!({"Type": "flexcontainer", "Id": id, "DisplayName": name});
        for (key, value) in extra.as_object().unwrap() {
            properties[key] = value.clone();
        }
        json!({"Properties": properties, "Widgets": kids})
    }

    #[test]
    fn a_growing_remainder_above_content_warns() {
        for (remainder_first, warn) in [(true, true), (false, false)] {
            let remainder = container_of("flexcontainer-3", "Remaining space", json!({}), vec![]);
            let content = container_of(
                "flexcontainer-4",
                "status",
                json!({}),
                vec![widget("ptcslabel", "ptcslabel-5")],
            );
            let kids = if remainder_first {
                vec![remainder, content]
            } else {
                vec![content, remainder]
            };
            let found = layout_rules(container_of(
                "flexcontainer-2",
                "page",
                json!({"flex-direction": "column"}),
                kids,
            ));
            let hit = found.iter().find(|(rule, _)| rule == "remainder-not-last");
            assert_eq!(hit.is_some(), warn, "{found:?}");
            if let Some((_, message)) = hit {
                assert!(message.starts_with("flexcontainer-3:"), "{message}");
            }
        }
    }

    #[test]
    fn a_slider_and_a_button_centred_in_one_row_warn() {
        for (alignment, warn) in [("center", true), ("flex-end", false)] {
            let found = layout_rules(container_of(
                "flexcontainer-2",
                "controls",
                json!({"flex-direction": "row", "align-items": alignment}),
                vec![
                    widget("ptcsslider", "ptcsslider-3"),
                    widget("ptcsbutton", "ptcsbutton-4"),
                ],
            ));
            assert_eq!(
                found
                    .iter()
                    .any(|(rule, _)| rule == "slider-button-alignment"),
                warn,
                "{found:?}"
            );
        }
    }

    #[test]
    fn short_labels_do_not_need_wrapping_but_sentences_and_property_displays_do() {
        for (text, width, expected) in [
            ("Form", None, false),
            ("Form", Some(20.0), false),
            ("Edit record", Some(240.0), false),
            (
                "A long description needs more than one line in this form",
                Some(180.0),
                true,
            ),
            ("", Some(240.0), true),
        ] {
            let label =
                json!({"Properties": {"Type": "ptcslabel", "MultiLine": true, "LabelText": text}});
            assert_eq!(wraps_content(&label, true, width), expected, "{text}");
        }
        assert!(wraps_content(
            &widget("ptcspropertydisplay", "display-1"),
            true,
            Some(1000.0)
        ));
    }

    #[test]
    fn a_capped_container_holding_wrapping_text_warns() {
        let label = |multi: bool| {
            let mut label = widget("ptcslabel", "ptcslabel-4");
            label["Properties"]["MultiLine"] = json!(multi);
            label
        };
        for (name, extra, kid, warn) in [
            (
                "panel-2",
                json!({"flex-size": "fixed"}),
                widget("ptcspropertydisplay", "ptcspropertydisplay-4"),
                true,
            ),
            (
                "panel-2",
                json!({"flex-max-height": "220px"}),
                widget("ptcspropertydisplay", "ptcspropertydisplay-4"),
                true,
            ),
            ("panel-2", json!({"flex-size": "fixed"}), label(true), true),
            (
                "panel-2",
                json!({"flex-size": "fixed"}),
                label(false),
                false,
            ),
            (
                "container-2",
                json!({"flex-size": "fixed"}),
                label(true),
                false,
            ),
            (
                "container-2",
                json!({"flex-size": "fixed"}),
                widget("ptcspropertydisplay", "ptcspropertydisplay-4"),
                false,
            ),
            (
                "panel-2",
                json!({"flex-size": "range", "flex-max-height": ""}),
                widget("ptcspropertydisplay", "ptcspropertydisplay-4"),
                false,
            ),
        ] {
            let found = layout_rules(container_of(
                "flexcontainer-2",
                name,
                extra.clone(),
                vec![kid],
            ));
            assert_eq!(
                found
                    .iter()
                    .any(|(rule, _)| rule == "wrapped-content-clipping"),
                warn,
                "{extra} {found:?}"
            );
        }
    }

    #[test]
    fn leaf_sizes_warn_only_when_ignored_in_a_responsive_container() {
        for (kind, fill, width, height, responsive, parent, warn) in [
            ("ptcsimage", false, 120, 60, true, true, true),
            ("ptcsimage", false, 120, 0, true, true, true),
            ("ptcsimage", false, 0, 60, true, true, true),
            ("ptcsimage", false, 120, 60, false, true, false),
            ("ptcsimage", true, 120, 60, true, true, false),
            ("ptcsimage", false, 0, 0, true, true, false),
            ("ptcsimage", false, 120, 60, true, false, false),
            ("flexcontainer", false, 120, 60, true, true, false),
        ] {
            let value = json!({"UI": {"Properties": {"Type": "mashup", "Id": "mashup-root"}, "Widgets": [
                {"Properties": {"Type": "flexcontainer", "Id": "row-1", "ResponsiveLayout": parent}, "Widgets": [
                    {"Properties": {"Type": kind, "Id": "leaf-2", "FillContainer": fill, "Width": width, "Height": height, "ResponsiveLayout": responsive}}
                ]}
            ]}});
            let mut found = Vec::new();
            check_content(&value.to_string(), &mut found);
            let warnings: Vec<_> = found
                .iter()
                .filter(|p| p.rule == "responsive-leaf-size")
                .collect();
            assert_eq!(warnings.len(), usize::from(warn), "{value}");
            if let Some(warning) = warnings.first() {
                assert_eq!(warning.severity, Severity::Advisory);
                assert_eq!(
                    warning.message,
                    "leaf-2: Width/Height are ignored at run time; set ResponsiveLayout=false"
                );
            }
        }
    }

    #[test]
    fn authored_mixed_rows_warn_until_bottom_aligned() {
        for alignment in [None, Some("flex-start"), Some("flex-end"), Some("center")] {
            let mut value = serde_json::json!({"UI": {"Properties": {"Type": "mashup", "Id": "mashup-root"}, "Widgets": [
                {"Properties": {"Type": "flexcontainer", "Id": "flexcontainer-1", "DisplayName": "toolbar", "flex-direction": "row"}, "Widgets": [
                    {"Properties": {"Type": "ptcstextfield", "Id": "ptcstextfield-2", "Label": "Filter"}},
                    {"Properties": {"Type": "ptcsbutton", "Id": "ptcsbutton-3", "Label": "Apply"}}
                ]}
            ]}});
            if let Some(alignment) = alignment {
                value["UI"]["Widgets"][0]["Properties"]["align-items"] = Value::from(alignment);
            }
            let mut problems = Vec::new();
            check_content(&mashup::to_sidecar(&value), &mut problems);
            let warnings: Vec<_> = problems
                .iter()
                .filter(|p| p.rule == "mixed-label-alignment")
                .collect();
            assert_eq!(
                warnings.len(),
                usize::from(alignment.is_none() || alignment == Some("flex-start"))
            );
            if let Some(warning) = warnings.first() {
                assert_eq!(warning.severity, Severity::Advisory);
                for text in [
                    "flexcontainer-1",
                    "ptcstextfield-2",
                    "ptcsbutton-3",
                    "twaco mashup set <M> flexcontainer-1 --prop align-items=flex-end",
                ] {
                    assert!(warning.message.contains(text), "{}", warning.message);
                }
            }
        }
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

#[cfg(test)]
mod style_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn style_settings_the_runtime_ignores_are_advisories() {
        let content = json!({"UI": {"Properties": {"StyleProperties": {"data": {
            "ptcstextfield-3": {"widget": "ptcs-textfield", "parts": {"$": {"$": {"top": "50%"}}}}
        }}}}});
        let mut problems = Vec::new();
        style_problems(&content, &mut problems);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].rule, "style-placement-property");
        assert_eq!(problems[0].severity, Severity::Advisory);
    }
}
