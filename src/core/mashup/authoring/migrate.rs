//! Offline widget migrations from Composer's 10.2 tables and twaco equivalents.

use super::{finish_edit, load_existing, AuthoringError, EditPlan};
use crate::core::widgets::MIGRATIONS as TABLE;
use crate::core::{config::Solution, mashup, widgets};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct MigrateRequest {
    pub mashup: String,
}

#[derive(Debug, Serialize)]
pub struct Migration {
    pub id: String,
    pub name: String,
    pub from: String,
    pub to: String,
    pub source: &'static str,
    pub dropped: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Kept {
    pub id: String,
    pub name: String,
    pub widget: String,
    pub reason: String,
}

#[derive(Debug)]
pub struct MigratePlan {
    pub(crate) edit: Option<EditPlan>,
    pub mashup: String,
    pub migrations: Vec<Migration>,
    /// Widgets left as they are when no supported conversion applies.
    pub kept: Vec<Kept>,
    pub warnings: Vec<String>,
}

pub fn migrate_plan_json(plan: &MigratePlan, applied: bool) -> Value {
    json!({"mashup": plan.mashup, "applied": applied,
        "changed": plan.edit.is_some(), "planned": plan.migrations.len(),
        "kept_count": plan.kept.len(), "migrations": plan.migrations,
        "kept": plan.kept, "warnings": plan.warnings})
}

#[derive(Clone)]
struct Endpoint {
    to: String,
    names: BTreeMap<String, Vec<String>>,
    deleted: BTreeSet<String>,
    properties: Value,
    preserve_wiring: bool,
}

impl Endpoint {
    fn candidates<'a>(&'a self, old: &'a str) -> impl Iterator<Item = &'a str> {
        let names = self.names.get(old);
        names.into_iter().flatten().map(String::as_str).chain(
            (names.is_none() && (self.preserve_wiring || !self.deleted.contains(old)))
                .then_some(old),
        )
    }
}

fn mapping_parts(mapping: &Value) -> impl Iterator<Item = &Value> {
    mapping
        .as_str()
        .map(|_| mapping)
        .or_else(|| mapping.is_object().then_some(mapping))
        .into_iter()
        .chain(mapping.as_array().into_iter().flatten())
}

fn destination(mapping: &Value) -> Option<&str> {
    mapping.as_str().or_else(|| mapping["newproperty"].as_str())
}

fn matches_properties(matcher: &Value, props: &Value) -> bool {
    matcher.as_object().is_none_or(|conditions| {
        conditions
            .iter()
            .all(|(key, expected)| props.get(key) == Some(expected))
    })
}

fn rule_for<'a>(family: &'a Value, props: &Value) -> Option<&'a Value> {
    let rule = if let Some(variants) = family["variants"].as_array() {
        variants
            .iter()
            .find(|rule| matches_properties(&rule["when"], props))?
    } else {
        family
    };
    (rule["newtype"].is_string() && matches_properties(&rule["when"], props)).then_some(rule)
}

fn mappings(rule: &Value) -> impl Iterator<Item = (&String, &Value)> {
    rule["mappings"].as_object().into_iter().flatten()
}

fn mapped_value(mapping: &Value, value: &Value, original: &Map<String, Value>) -> Value {
    if let Some(name) = mapping["usepropvalue"].as_str() {
        return original.get(name).cloned().unwrap_or_default();
    }
    let key = value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned);
    let value = mapping["values"].get(&key).unwrap_or(value);
    if mapping["casting"] == "String" {
        json!(value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned))
    } else {
        value.clone()
    }
}

fn target_supports(contract: &widgets::Widget, original: &Value, name: &str) -> bool {
    contract.defaults.contains_key(name)
        || contract
            .properties
            .as_ref()
            .is_some_and(|props| props.contains_key(name))
        || widgets::catalog().base_properties.contains_key(name)
        || widgets::dynamic_property(&contract.type_id, original, name, false).is_some()
        || matches!(
            name,
            "Id" | "DisplayName" | "Left" | "Top" | "Width" | "Height" | "ZIndex"
        )
}

fn collection_properties(props: &mut Map<String, Value>) -> Result<(), AuthoringError> {
    for (show, pin, new) in [
        ("ShowHeaders", "PinHeadersToTop", "SectionHeaders"),
        ("ShowFooters", "PinFootersToBottom", "SectionFooters"),
    ] {
        let value = if props.get(show).and_then(Value::as_bool) != Some(true) {
            "none"
        } else if props.get(pin).and_then(Value::as_bool) == Some(true) {
            "pinned"
        } else {
            "visible"
        };
        props.insert(new.into(), json!(value));
    }
    let selection = if props.get("AllowSelection").and_then(Value::as_bool) != Some(true) {
        "none"
    } else if props.get("MultiSelect").and_then(Value::as_bool) == Some(true) {
        "multiple"
    } else {
        "single"
    };
    props.insert("SelectionMode".into(), json!(selection));
    let gap = ["SectionInsetBottom", "SectionInsetTop"]
        .iter()
        .filter_map(|key| props.get(*key).and_then(Value::as_f64))
        .sum::<f64>();
    props.insert("SectionGap".into(), json!(gap));
    for name in ["MashupPropertyBinding", "MashupGlobalPropertyBinding"] {
        if let Some(text) = props.get(name).and_then(Value::as_str) {
            let value = serde_json::from_str(text).map_err(|e| {
                AuthoringError::Invalid(format!(
                    "collection {name}: invalid JSON: {e}; fix the property before migrating"
                ))
            })?;
            props.insert(name.into(), value);
        }
    }
    match props.get("View").and_then(Value::as_str) {
        Some("flow") => {
            for (old, new) in [
                ("RowSpacing", "RowGap"),
                ("MinimumSpacing", "ColumnGap"),
                ("MashupWidth", "CellWidth"),
            ] {
                if let Some(value) = props.get(old).cloned() {
                    props.insert(new.into(), value);
                }
            }
            let alignment = match props
                .get("FlowLayoutContentGravity")
                .and_then(Value::as_str)
            {
                Some("Bottom") => "bottom",
                Some("Center") => "center",
                _ => "top",
            };
            props.insert("ContentAlignment".into(), json!(alignment));
        }
        Some("table") => {
            for name in [
                "RowGap",
                "TopPadding",
                "BottomPadding",
                "ColumnGap",
                "CellWidth",
            ] {
                props.insert(name.into(), json!(0));
            }
            props.insert("ContentAlignment".into(), json!("top"));
        }
        _ => {}
    }
    Ok(())
}

fn repeater_properties(props: &mut Map<String, Value>) {
    let view = props
        .get("View")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    for (old, new, used) in [
        (
            "FixedMashupHeight",
            "CellHeight",
            matches!(view.as_str(), "horizontal-wrap" | "vertical"),
        ),
        (
            "FixedMashupWidth",
            "CellWidth",
            matches!(view.as_str(), "horizontal-wrap" | "horizontal"),
        ),
    ] {
        props.remove(new);
        if used {
            if let Some(value) = props
                .get(old)
                .and_then(|value| {
                    value
                        .as_f64()
                        .or_else(|| value.as_str()?.trim().parse().ok())
                })
                .filter(|number| number.is_finite())
            {
                props.remove(old);
                props.insert(new.into(), json!(value));
            }
        }
    }
    for property in ["RowGap", "TopPadding", "BottomPadding", "RightPadding"] {
        props.insert(property.into(), json!(0));
    }
    if matches!(view.as_str(), "horizontal-wrap" | "horizontal") {
        props.insert("ColumnGap".into(), json!(0));
    }
    if let Some(parameters) = props.get("MashupParameters").and_then(Value::as_array) {
        let bindings: Map<_, _> = parameters
            .iter()
            .filter_map(|parameter| {
                let name = parameter["ParameterName"].as_str()?;
                let field = props
                    .get(name)?
                    .as_str()
                    .filter(|field| !field.is_empty())?;
                Some((field.to_owned(), json!(name)))
            })
            .collect();
        props.insert("MashupPropertyBinding".into(), Value::Object(bindings));
    }
}

fn legacy_grid_columns(kind: &str, config: &Value) -> Result<Value, &'static str> {
    let mut config = match config {
        Value::String(text) if !text.trim().is_empty() => {
            serde_json::from_str(text).map_err(|_| "the grid has unreadable ColumnFormat JSON")?
        }
        other => other.clone(),
    };
    let Some(columns) = config.get_mut("formatInfo").and_then(Value::as_array_mut) else {
        return Ok(config);
    };
    let has_columns = !columns.is_empty();
    for column in columns {
        let Some(column) = column.as_object_mut() else {
            return Err("the grid has an unreadable column definition");
        };
        if column
            .get("ValidationExpression")
            .is_some_and(|expression| !expression.is_null() && expression != "")
        {
            return Err("the grid's column validation expressions need Composer to rewire them");
        }
        for (old, new) in [
            ("Align", "cellHorizontalAlignment"),
            ("AllowEdit", "editable"),
            ("headerTextAlignment", "headerHorizontalAlignment"),
            ("ValidationExpression", "validationExpression"),
            ("ValidationMessage", "validationMessage"),
        ] {
            if let Some(value) = column.remove(old) {
                column.insert(new.into(), value);
            }
        }
        if kind == "dhxgrid" {
            column.insert("sortable".into(), json!(true));
        }
    }
    if kind == "dhxgrid" && has_columns {
        config["__dhxMigrated__"] = json!(true);
    }
    Ok(config)
}

fn migrate_widget(
    widget: &mut Value,
    parent: &str,
    plan: &mut MigratePlan,
    endpoints: &mut BTreeMap<String, Endpoint>,
) -> Result<(), AuthoringError> {
    let kind = widget["Properties"]["Type"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let id = widget["Properties"]["Id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let name = widget["Properties"]["DisplayName"]
        .as_str()
        .unwrap_or(&id)
        .to_string();
    let catalog = widgets::catalog();
    let (family, source) = if TABLE["twaco_newtypes"][&kind].is_object() {
        (&TABLE["twaco_newtypes"][&kind], "twaco")
    } else {
        (&TABLE["newtypes"][&kind], "composer")
    };
    let properties = &widget["Properties"];
    let legacy = !family.is_null()
        || catalog
            .by_type(&kind)
            .is_some_and(|w| w.legacy && !w.function)
        || (catalog.platform_types.contains(&kind) && catalog.by_type(&kind).is_none());
    if legacy
        && !matches!(
            kind.as_str(),
            "mashup" | "panel" | "layout" | "flexcontainer"
        )
    {
        let rule = rule_for(family, properties);
        let columns = if matches!(
            kind.as_str(),
            "dhxgrid" | "gridadvanced" | "treegridadvanced"
        ) {
            properties
                .get("ColumnFormat")
                .map(|config| legacy_grid_columns(&kind, config))
                .transpose()
        } else {
            Ok(None)
        };
        let refusal = match kind.as_str() {
            "container" if parent != "ptcsdynamicpanel" => {
                Some("Composer only converts container inside ptcsdynamicpanel")
            }
            "button" if properties["ConfirmationRequired"] == true => {
                Some("the button needs Composer to create and wire its confirmation function")
            }
            "autorefresh" if properties["ShowControls"] != false && properties["Visible"] != false => {
                Some("visible refresh controls need Composer to create and wire replacement widgets")
            }
            "gauge" if !properties["MinValue"].as_f64()
                .zip(properties["MaxValue"].as_f64()).is_some_and(|(min, max)| max > min) => {
                Some("a KPI dial needs an explicit numeric gauge range")
            }
            "leddisplay" if !(properties["Decimals"].is_number() || properties["Data"].is_number()) => {
                Some("the LED display has no stored numeric display settings")
            }
            "menu" if properties["Menu"].as_str().is_none_or(str::is_empty) => {
                Some("a Menu Bar replacement needs a configured Menu entity")
            }
            "radiobuttonlist" => Some("its StateDefinition-backed choices need a new radio group or a data-backed list"),
            "tree" => Some("its hierarchy and selected-node properties need a new data and selection model"),
            _ if rule.is_none() => TABLE["skipped_newtypes"][&kind]
                .as_str()
                .or(Some("no supported newtype migrator for this configuration; keep this widget or replace it manually")),
            _ => None,
        }.or_else(|| columns.as_ref().err().copied());
        if let Some(reason) = refusal {
            plan.kept.push(Kept {
                id: id.clone(),
                name,
                widget: kind.clone(),
                reason: reason.to_owned(),
            });
        } else if let Some(rule) = rule {
            let target = rule["newtype"]
                .as_str()
                .expect("a selected rule names its target");
            let contract = catalog.by_type(target).ok_or_else(|| {
                AuthoringError::Invalid(format!(
                    "{id}: migration target {target} is missing from the catalogue"
                ))
            })?;
            let mut original = widget["Properties"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            if kind == "collection" {
                collection_properties(&mut original)?;
            }
            if kind == "repeater" {
                repeater_properties(&mut original);
            }
            if kind == "fileupload" && original.get("MultiFilesSelect") != Some(&json!(true)) {
                original.insert("MaxNumberOfFiles".into(), json!(1));
            }
            if let Ok(Some(columns)) = columns {
                original.insert("ColumnFormat".into(), columns);
            }
            if kind == "piechart" {
                if let (Some(format), Some(name)) = (
                    original.get("LabelFormat").cloned(),
                    match original.get("LabelType").and_then(Value::as_str) {
                        Some("NUMERIC") => Some("DataFieldFormat"),
                        Some("DATETIME") => Some("DataFieldDateFormat"),
                        _ => None,
                    },
                ) {
                    original.remove("LabelFormat");
                    original.insert(name.into(), format);
                }
            }
            let mut props = contract.defaults.clone();
            if !contract.function {
                props.extend(if target == "flexcontainer" {
                    catalog.dropped.flexcontainer.clone()
                } else {
                    catalog.dropped.ptcs.clone()
                });
            }
            let mut dropped = vec![];
            let mut declared: BTreeMap<_, _> = mappings(rule).collect();
            let mut create = rule["create"].as_object().cloned().unwrap_or_default();
            let mut deleted: BTreeSet<String> = rule["deletions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let mut delete_patterns = Vec::new();
            let mut read_patterns = |step: &Value| {
                delete_patterns.extend(
                    step["delete_patterns"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|pattern| {
                            regex::Regex::new(pattern).expect("embedded migration deletion pattern")
                        }),
                );
            };
            read_patterns(rule);
            for step in rule["conditional"].as_array().into_iter().flatten() {
                if matches_properties(&step["when"], properties) {
                    declared.extend(mappings(step));
                    create.extend(
                        step["create"]
                            .as_object()
                            .into_iter()
                            .flatten()
                            .map(|(k, v)| (k.clone(), v.clone())),
                    );
                    deleted.extend(
                        step["deletions"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(str::to_owned),
                    );
                    read_patterns(step);
                }
            }
            // The older button's Text and Alignment are spelled as the corresponding
            // compatibility aliases of the new type.
            if kind == "button" {
                declared.extend(TABLE["renames"][target].as_object().into_iter().flatten());
            }
            // In Composer numeric mode is set by a callback rather than a literal step.
            if kind == "numericentry" {
                create.insert("NumericInputMode".into(), json!(true));
                create.insert("ShowValidationFailure".into(), json!(true));
            }
            let original_value = Value::Object(original.clone());
            deleted.extend(
                original
                    .keys()
                    .filter(|name| delete_patterns.iter().any(|pattern| pattern.is_match(name)))
                    .cloned(),
            );
            for (old, value) in &original {
                if old == "Type" || old == "__TypeDisplayName" {
                    continue;
                }
                if let Some(mapping) = declared.get(old) {
                    let mut copied = false;
                    for part in mapping_parts(mapping) {
                        if let Some(new) = destination(part) {
                            if target_supports(contract, &original_value, new)
                                || kind == "collection"
                            {
                                props.insert(new.into(), mapped_value(part, value, &original));
                                copied = true;
                            }
                        }
                    }
                    if !copied {
                        dropped.push(old.clone());
                    }
                } else if !deleted.contains(old) && target_supports(contract, &original_value, old)
                {
                    props.insert(old.clone(), value.clone());
                } else {
                    dropped.push(old.clone());
                }
            }
            // Events and services need mapping even when Composer stores no property value.
            let names = declared
                .into_iter()
                .map(|(old, mapping)| {
                    let names = mapping_parts(mapping)
                        .filter_map(destination)
                        .map(str::to_owned)
                        .collect();
                    (old.clone(), names)
                })
                .collect();
            props.extend(create);
            props.insert("Type".into(), json!(target));
            props.insert(
                "__TypeDisplayName".into(),
                json!(contract.type_display_name),
            );
            props.insert("Id".into(), json!(id));
            if kind == "container" && original.get("ResponsiveLayout") == Some(&json!(true)) {
                if let Some(children) = widget["Widgets"].as_array_mut() {
                    if children.first().is_some_and(|w| {
                        w["Properties"]["Type"] == "panel"
                            && w["Properties"]["ResponsiveLayout"] == true
                    }) {
                        *children = children[0]["Widgets"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        props.insert("ResponsiveLayout".into(), json!(false));
                    }
                }
            }
            for property in &dropped {
                plan.warnings.push(format!(
                    "{id}: dropped property {property}; no target mapping"
                ));
            }
            if let Some(why) = rule["why"].as_str() {
                plan.warnings
                    .push(format!("{id}: twaco-authored replacement. {why}"));
            }
            widget["Properties"] = Value::Object(props);
            endpoints.insert(
                id.clone(),
                Endpoint {
                    to: target.into(),
                    names,
                    deleted,
                    properties: widget["Properties"].clone(),
                    preserve_wiring: TABLE["newtypes_source"]["preserve_wiring"]
                        .as_array()
                        .is_some_and(|types| types.iter().any(|type_id| type_id == &kind)),
                },
            );
            plan.migrations.push(Migration {
                id,
                name,
                from: kind.clone(),
                to: target.into(),
                source,
                dropped,
            });
        }
    }
    let parent = widget["Properties"]["Type"]
        .as_str()
        .unwrap_or("")
        .to_string();
    for child in widget["Widgets"].as_array_mut().into_iter().flatten() {
        migrate_widget(child, &parent, plan, endpoints)?;
    }
    Ok(())
}

fn repoint(
    content: &mut Value,
    endpoints: &BTreeMap<String, Endpoint>,
    warnings: &mut Vec<String>,
) {
    for binding in content["DataBindings"].as_array_mut().into_iter().flatten() {
        let mut removed = vec![];
        for (area, id_key, property, base) in [
            (
                "SourceArea",
                "SourceId",
                "SourceProperty",
                "SourcePropertyBaseType",
            ),
            (
                "TargetArea",
                "TargetId",
                "TargetProperty",
                "TargetPropertyBaseType",
            ),
        ] {
            if binding[area] != "UI" {
                continue;
            }
            let id = binding[id_key].as_str().unwrap_or("").to_string();
            let Some(endpoint) = endpoints.get(&id) else {
                continue;
            };
            for (i, map) in binding["PropertyMaps"]
                .as_array_mut()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let old = map[property].as_str().unwrap_or("").to_string();
                let contract = widgets::catalog()
                    .by_type(&endpoint.to)
                    .expect("migration target");
                let found = endpoint.candidates(&old).find_map(|new| {
                    contract
                        .properties
                        .as_ref()
                        .and_then(|p| p.get(new))
                        .or_else(|| widgets::catalog().base_properties.get(new))
                        .cloned()
                        .or_else(|| {
                            widgets::dynamic_property(&endpoint.to, &endpoint.properties, new, true)
                        })
                        .filter(|property| {
                            endpoint.preserve_wiring
                                || if area == "SourceArea" {
                                    property.source
                                } else {
                                    property.target
                                }
                        })
                        .map(|property| (new, property))
                });
                if let Some((new, found)) = found {
                    map[property] = json!(new);
                    if let Some(kind) = &found.base_type {
                        map[base] = json!(kind);
                    }
                } else {
                    warnings.push(format!(
                        "{id}: dropped binding property {old}; no target contract"
                    ));
                    removed.push(i);
                }
            }
        }
        if let Some(maps) = binding["PropertyMaps"].as_array_mut() {
            let mut i = 0;
            maps.retain(|_| {
                let keep = !removed.contains(&i);
                i += 1;
                keep
            });
        }
    }
    if let Some(bindings) = content["DataBindings"].as_array_mut() {
        bindings.retain(|b| b["PropertyMaps"].as_array().is_some_and(|m| !m.is_empty()));
    }
    if let Some(events) = content["Events"].as_array_mut() {
        events.retain_mut(|event| {
            for (area, id_key, name_key, is_event) in [
                (
                    "EventTriggerArea",
                    "EventTriggerId",
                    "EventTriggerEvent",
                    true,
                ),
                (
                    "EventHandlerArea",
                    "EventHandlerId",
                    "EventHandlerService",
                    false,
                ),
            ] {
                if event[area] != "UI" {
                    continue;
                }
                let id = event[id_key].as_str().unwrap_or("").to_string();
                let Some(endpoint) = endpoints.get(&id) else {
                    continue;
                };
                let old = event[name_key].as_str().unwrap_or("").to_string();
                let contract = widgets::catalog()
                    .by_type(&endpoint.to)
                    .expect("migration target");
                let names = if is_event {
                    &contract.events
                } else {
                    &contract.services
                };
                let mapped = endpoint.candidates(&old).find(|new| {
                    names
                        .as_ref()
                        .is_some_and(|names| names.iter().any(|name| name == *new))
                });
                if let Some(new) = mapped {
                    event[name_key] = json!(new);
                } else {
                    warnings.push(format!(
                        "{id}: dropped event connection {old}; no target contract"
                    ));
                    return false;
                }
            }
            true
        });
    }
}

/// Composer keeps function widgets at the root, outside visual containers.
fn relocate_refresh_functions(root: &mut Value, ids: &BTreeSet<&str>) {
    fn take_nested(widget: &mut Value, ids: &BTreeSet<&str>, out: &mut Vec<Value>) {
        if let Some(children) = widget["Widgets"].as_array_mut() {
            children.retain_mut(|child| {
                if child["Properties"]["Id"]
                    .as_str()
                    .is_some_and(|id| ids.contains(id))
                {
                    out.push(std::mem::take(child));
                    false
                } else {
                    take_nested(child, ids, out);
                    true
                }
            });
        }
    }
    if let Some(children) = root["Widgets"].as_array_mut() {
        let mut moved = Vec::new();
        for child in &mut *children {
            take_nested(child, ids, &mut moved);
        }
        children.extend(moved);
    }
}

pub(crate) fn plan_migrate(
    solution: &Solution,
    request: &MigrateRequest,
) -> Result<MigratePlan, AuthoringError> {
    let mut loaded = load_existing(solution, &request.mashup)?;
    let mut document = mashup::edit::parse(&loaded.assets.content).map_err(AuthoringError::Edit)?;
    let mut plan = MigratePlan {
        edit: None,
        mashup: request.mashup.clone(),
        migrations: vec![],
        kept: vec![],
        warnings: vec![],
    };
    let mut endpoints = BTreeMap::new();
    migrate_widget(
        &mut document.value_mut()["UI"],
        "",
        &mut plan,
        &mut endpoints,
    )?;
    let refresh_ids: BTreeSet<_> = plan
        .migrations
        .iter()
        .filter(|migration| migration.from == "autorefresh")
        .map(|migration| migration.id.as_str())
        .collect();
    if !refresh_ids.is_empty() {
        relocate_refresh_functions(&mut document.value_mut()["UI"], &refresh_ids);
    }
    repoint(document.value_mut(), &endpoints, &mut plan.warnings);
    if !plan.migrations.is_empty() {
        let mut before = vec![];
        let mut after = vec![];
        mashup::validate::check_content(&loaded.assets.content, &mut before);
        mashup::validate::check_content(&document.to_content(), &mut after);
        let new_errors: Vec<_> = after
            .iter()
            .filter(|p| p.severity == mashup::validate::Severity::Error && !before.contains(p))
            .collect();
        if !new_errors.is_empty() {
            return Err(AuthoringError::Refused(format!(
                "{}: migration would introduce validation errors: {}; nothing written",
                request.mashup,
                new_errors
                    .iter()
                    .map(|p| p.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        // Migration preserves authored layout; it must not run the authoring reflow pass.
        loaded.deterministic = true;
        document.set_verbatim(true);
        plan.edit = Some(finish_edit(loaded, document, "mashup migrate")?);
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> MigratePlan {
        MigratePlan {
            edit: None,
            mashup: "Acme.View".into(),
            migrations: vec![],
            kept: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn numeric_migration_repoints_both_ends_and_reports_unmapped_properties() {
        let mut plan = plan();
        let mut endpoints = BTreeMap::new();
        let mut widget = json!({"Properties":{"Type":"numericentry","Id":"entry-1","DisplayName":"Amount","Value":12,"AllowNegatives":false,"OldStyle":"Acme.Style"},"Widgets":[]});
        migrate_widget(&mut widget, "flexcontainer", &mut plan, &mut endpoints).unwrap();
        assert_eq!(widget["Properties"]["Type"], "ptcstextfield");
        assert_eq!(widget["Properties"]["NumericValue"], 12);
        assert_eq!(widget["Properties"]["AllowNegativeValues"], false);
        assert_eq!(widget["Properties"]["NumericInputMode"], true);
        assert_eq!(widget["Properties"]["Id"], "entry-1");
        assert_eq!(plan.migrations[0].dropped, ["OldStyle"]);
        let mut content = json!({"DataBindings":[{"SourceArea":"UI","SourceId":"entry-1","TargetArea":"UI","TargetId":"entry-1","PropertyMaps":[{"SourceProperty":"Value","TargetProperty":"Value"}]}],"Events":[]});
        repoint(&mut content, &endpoints, &mut plan.warnings);
        let map = &content["DataBindings"][0]["PropertyMaps"][0];
        assert_eq!(map["SourceProperty"], "NumericValue");
        assert_eq!(map["TargetProperty"], "NumericValue");
        assert_eq!(map["TargetPropertyBaseType"], "NUMBER");
        assert!(plan.warnings.iter().any(|w| w.contains("OldStyle")));
    }

    #[test]
    fn collection_configuration_values_and_event_names_follow_composer() {
        let mut plan = plan();
        let mut endpoints = BTreeMap::new();
        let mut widget = json!({"Properties":{"Type":"collection","Id":"collection-1","View":"flow","MashupWidth":200,"MashupHeight":80,"AllowSelection":true,"MultiSelect":true,"ShowHeaders":true,"PinHeadersToTop":true,"SectionInsetBottom":2,"SectionInsetTop":4,"FlowLayoutGravity":"Edge","MashupPropertyBinding":"{\"name\":\"title\"}"},"Widgets":[]});
        migrate_widget(&mut widget, "flexcontainer", &mut plan, &mut endpoints).unwrap();
        let props = &widget["Properties"];
        assert_eq!(props["Layout"], "flex");
        assert_eq!(props["CellWidth"], 200);
        assert_eq!(props["CellHeight"], 80);
        assert_eq!(props["SelectionMode"], "multiple");
        assert_eq!(props["SectionHeaders"], "pinned");
        assert_eq!(props["SectionGap"], 6.0);
        assert_eq!(props["RowHorizontalAlignment"], "space-between");
        assert_eq!(props["MashupPropertyBinding"]["name"], "title");
        let mut content = json!({"Events":[{"EventTriggerArea":"UI","EventTriggerId":"collection-1","EventTriggerEvent":"CellWasClicked","EventHandlerArea":"Data"}]});
        repoint(&mut content, &endpoints, &mut plan.warnings);
        assert_eq!(content["Events"][0]["EventTriggerEvent"], "CellClicked");
    }

    #[test]
    fn containers_only_convert_in_the_platforms_supported_parent() {
        for (parent, expected) in [("ptcsdynamicpanel", true), ("flexcontainer", false)] {
            let mut plan = plan();
            let mut endpoints = BTreeMap::new();
            let mut widget = json!({"Properties":{"Type":"container","Id":"container-1","DisplayName":"Body"},"Widgets":[]});
            migrate_widget(&mut widget, parent, &mut plan, &mut endpoints).unwrap();
            assert_eq!(!plan.migrations.is_empty(), expected);
            assert_eq!(!plan.kept.is_empty(), !expected);
        }
    }

    #[test]
    fn unsupported_legacy_widgets_are_named_without_changing_them() {
        for kind in ["gauge", "radiobuttonlist", "menu", "tree", "leddisplay"] {
            let mut plan = plan();
            let mut endpoints = BTreeMap::new();
            let mut widget = json!({"Properties":{"Type":kind,"Id":"old-1","DisplayName":"Acme.Legacy"},"Widgets":[]});
            let before = widget.clone();
            migrate_widget(&mut widget, "flexcontainer", &mut plan, &mut endpoints).unwrap();
            assert_eq!(widget, before);
            assert_eq!(plan.kept[0].name, "Acme.Legacy");
            assert_eq!(plan.kept[0].widget, kind);
        }
    }

    #[test]
    fn generated_composer_and_twaco_migration_families_map_properties() {
        let cases = [
            (
                "autorefresh",
                "autorefreshfunction",
                json!({"ShowControls":false,"RefreshInterval":7}),
                "RefreshInterval",
                json!(7),
            ),
            (
                "button",
                "ptcsbutton",
                json!({"Text":"Acme"}),
                "Label",
                json!("Acme"),
            ),
            (
                "datetimepicker",
                "ptcsdatepicker",
                json!({"DateOrder":"dd-mm-yyyy"}),
                "DateOrder",
                json!("DMY"),
            ),
            (
                "dhxgrid",
                "ptcsgrid",
                json!({"CellTextWrapping":true}),
                "SingleLineRows",
                json!(false),
            ),
            (
                "dhxlist",
                "ptcsdropdown",
                json!({"View":"dropdown","TextIfNoSelection":"Choose"}),
                "HintText",
                json!("Choose"),
            ),
            (
                "dhxlist",
                "ptcslist",
                json!({"View":"list","DisplayField":"Name"}),
                "DisplayField",
                json!("Name"),
            ),
            (
                "fileupload",
                "ptcsfileupload",
                json!({"MaximumFileSize":42}),
                "MaxFileSize",
                json!(42),
            ),
            (
                "gridadvanced",
                "ptcsgrid",
                json!({"EnableAddDeleteButtons":true}),
                "RowDeleteButton",
                json!(true),
            ),
            (
                "maskedtextbox",
                "ptcstextfield",
                json!({"TextAlign":"center"}),
                "TextAlignment",
                json!("left"),
            ),
            (
                "piechart",
                "ptcschartpie",
                json!({"ShowLegend":false}),
                "HideLegend",
                json!(true),
            ),
            (
                "ptc1divider",
                "ptcsdivider",
                json!({"BandStyle":"Acme.Style"}),
                "UseTheme",
                json!(true),
            ),
            (
                "propertydisplay",
                "ptcspropertydisplay",
                json!({}),
                "TableLayout",
                json!(true),
            ),
            (
                "repeater",
                "ptcscollection",
                json!({"View":"vertical"}),
                "Layout",
                json!("table"),
            ),
            (
                "slider",
                "ptcsslider",
                json!({"Minimum":5}),
                "MinValueLabel",
                json!("5"),
            ),
            (
                "treegridadvanced",
                "ptcsgrid",
                json!({"ExpandRows":true}),
                "ExpandableRows",
                json!(true),
            ),
            (
                "valuedisplay",
                "ptcsvaluedisplay",
                json!({"VerticalAlignment":"bottom"}),
                "VerticalAlignment",
                json!("flex-end"),
            ),
            (
                "verticalslider",
                "ptcsslider",
                json!({"Vertical":true}),
                "ReverseMinMaxValues",
                json!(true),
            ),
            (
                "gauge",
                "ptcskpidial",
                json!({"Data":23,"MinValue":0,"MaxValue":50}),
                "Value",
                json!(23),
            ),
            (
                "leddisplay",
                "ptcsvaluedisplay",
                json!({"Label":"Acme","Decimals":2}),
                "DataBaseType",
                json!("NUMBER"),
            ),
            (
                "menu",
                "ptcsmenubar",
                json!({"ConfiguredOrDataDriven":"configured","DrivePageMashupOrBindToMashup":"pagemashup","Menu":"Acme.Menu"}),
                "PrimaryNavMenu",
                json!("Acme.Menu"),
            ),
        ];
        for (legacy, target, properties, name, expected) in cases {
            let mut properties = properties.as_object().unwrap().clone();
            properties.extend([
                ("Type".into(), json!(legacy)),
                ("Id".into(), json!("acme-1")),
                ("OldStyle".into(), json!("Acme.Style")),
            ]);
            let mut plan = plan();
            let mut endpoints = BTreeMap::new();
            let mut content = json!({"UI":{"Properties":{"Type":"mashup","Id":"mashup-root","ResponsiveLayout":false},"Widgets":[
                {"Properties":properties,"Widgets":[]},
                {"Properties":{"Type":"ptcstextfield","Id":"acme-peer"},"Widgets":[]},
                {"Properties":{"Type":"ptcsbutton","Id":"acme-trigger"},"Widgets":[]}
            ]},"Data":{},"DataBindings":[],"Events":[]});
            let (old, new, reads_from_widget) = match legacy {
                "autorefresh" => ("RefreshInterval", "RefreshInterval", false),
                "button" => ("Text", "Label", false),
                "datetimepicker" | "valuedisplay" => ("ToolTipField", "TooltipField", false),
                "dhxgrid" => ("RowHeight", "MinRowHeight", false),
                "dhxlist" if target == "ptcsdropdown" => ("TextIfNoSelection", "HintText", false),
                "fileupload" => ("FileName", "FileNames", true),
                "gridadvanced" => ("EditedTable", "EditedData", true),
                "maskedtextbox" => ("PlaceholderText", "HintText", false),
                "piechart" => ("ChartTitle", "Label", false),
                "slider" | "verticalslider" => ("Step", "StepSize", false),
                "treegridadvanced" => ("ParentIDFieldName", "ParentIDFieldName", false),
                "gauge" => ("Data", "Value", false),
                "leddisplay" => ("Data", "Data", false),
                "menu" => ("Mashup", "Mashup", true),
                _ => ("Visible", "Visible", false),
            };
            let ((source_id, source_property), (target_id, target_property), mapped_key) =
                if reads_from_widget {
                    (("acme-1", old), ("acme-peer", "Text"), "SourceProperty")
                } else {
                    (("acme-peer", "Text"), ("acme-1", old), "TargetProperty")
                };
            content["DataBindings"] = json!([{"Id":"binding-1","SourceArea":"UI","SourceId":source_id,"TargetArea":"UI","TargetId":target_id,"PropertyMaps":[{"SourceProperty":source_property,"TargetProperty":target_property}]}]);
            let event = match legacy {
                "autorefresh" => Some(("Refresh", "Refresh")),
                "button" | "valuedisplay" => Some(("Clicked", "Clicked")),
                "datetimepicker" => Some(("Changed", "Changed")),
                "dhxlist" if target == "ptcslist" => Some(("DoubleClicked", "DoubleClicked")),
                "fileupload" => Some(("UploadComplete", "UploadComplete")),
                "repeater" => Some(("DoubleClicked", "CellDoubleClicked")),
                "gridadvanced" | "treegridadvanced" => Some(("EditCancelled", "EditCanceled")),
                _ => None,
            };
            if let Some((old, _)) = event {
                content["Events"] = json!([{"Id":"event-1","EventTriggerArea":"UI","EventTriggerId":"acme-1","EventTriggerEvent":old,"EventHandlerArea":"UI","EventHandlerId":"acme-trigger","EventHandlerService":"TriggerClick"}]);
            }
            migrate_widget(&mut content["UI"], "", &mut plan, &mut endpoints).unwrap();
            repoint(&mut content, &endpoints, &mut plan.warnings);
            let props = &content["UI"]["Widgets"][0]["Properties"];
            assert_eq!(props["Type"], target, "{legacy}");
            assert_eq!(props[name], expected, "{legacy}");
            assert!(
                plan.migrations[0].dropped.iter().any(|p| p == "OldStyle"),
                "{legacy}"
            );
            let source = if matches!(legacy, "gauge" | "leddisplay" | "menu") {
                "twaco"
            } else {
                "composer"
            };
            assert_eq!(
                migrate_plan_json(&plan, false)["migrations"][0]["source"],
                source
            );
            assert_eq!(
                content["DataBindings"][0]["PropertyMaps"][0][mapped_key], new,
                "{legacy}"
            );
            if let Some((_, new)) = event {
                assert_eq!(content["Events"][0]["EventTriggerEvent"], new, "{legacy}");
            }
            let mut problems = vec![];
            mashup::validate::check_content(&content.to_string(), &mut problems);
            assert!(
                problems
                    .iter()
                    .all(|p| p.severity != mashup::validate::Severity::Error),
                "{legacy}: {problems:?}"
            );
        }
    }

    #[test]
    fn generated_migration_repoints_an_event_and_a_renamed_binding() {
        let mut plan = plan();
        let mut endpoints = BTreeMap::new();
        let mut content = json!({
            "UI":{"Properties":{"Type":"mashup","Id":"mashup-root","ResponsiveLayout":false},"Widgets":[
                {"Properties":{"Type":"gridadvanced","Id":"acme-grid"},"Widgets":[]},
                {"Properties":{"Type":"fileupload","Id":"acme-upload"},"Widgets":[]},
                {"Properties":{"Type":"textbox","Id":"acme-text"},"Widgets":[]}
            ]},
            "Data":{},
            "DataBindings":[{"Id":"binding-1","SourceArea":"UI","SourceId":"acme-upload","TargetArea":"UI","TargetId":"acme-text","PropertyMaps":[{"SourceProperty":"FileName","TargetProperty":"Text"}]}],
            "Events":[{"Id":"event-1","EventTriggerArea":"UI","EventTriggerId":"acme-grid","EventTriggerEvent":"EditCancelled","EventHandlerArea":"UI","EventHandlerId":"acme-upload","EventHandlerService":"ClearList"}]
        });
        migrate_widget(&mut content["UI"], "", &mut plan, &mut endpoints).unwrap();
        repoint(&mut content, &endpoints, &mut plan.warnings);
        assert_eq!(
            content["DataBindings"][0]["PropertyMaps"][0]["SourceProperty"],
            "FileNames"
        );
        assert_eq!(content["Events"][0]["EventTriggerEvent"], "EditCanceled");
        let mut problems = vec![];
        mashup::validate::check_content(&content.to_string(), &mut problems);
        assert!(
            problems
                .iter()
                .all(|p| p.severity != mashup::validate::Severity::Error),
            "{problems:?}"
        );
    }

    #[test]
    fn generated_grid_migration_translates_column_options_and_keeps_custom_expressions() {
        let mut plan = plan();
        let mut endpoints = BTreeMap::new();
        let mut grid = json!({"Properties":{"Type":"dhxgrid","Id":"acme-1","ColumnFormat":{"formatInfo":[
            {"FieldName":"Name","Align":"center","AllowEdit":true,"headerTextAlignment":"left","ValidationMessage":"Review"}
        ]}},"Widgets":[]});
        migrate_widget(&mut grid, "flexcontainer", &mut plan, &mut endpoints).unwrap();
        let config = &grid["Properties"]["ColumnFormat"];
        assert_eq!(config["__dhxMigrated__"], true);
        let name = &config["formatInfo"][0];
        assert_eq!(name["cellHorizontalAlignment"], "center");
        assert_eq!(name["editable"], true);
        assert_eq!(name["headerHorizontalAlignment"], "left");
        assert_eq!(name["validationMessage"], "Review");
        assert_eq!(name["sortable"], true);
        assert!(name.get("AllowEdit").is_none());
        let mut custom = json!({"Properties":{"Type":"gridadvanced","Id":"acme-2","ColumnFormat":{"formatInfo":[
            {"FieldName":"Count","ValidationExpression":"value > 0"}
        ]}},"Widgets":[]});
        let before = custom.clone();
        migrate_widget(&mut custom, "flexcontainer", &mut plan, &mut endpoints).unwrap();
        assert_eq!(custom, before);
        assert!(plan.kept[0].reason.contains("validation expressions"));
    }

    #[test]
    fn migration_moves_invisible_refresh_to_the_root_and_keeps_visible_controls() {
        let mut plan = plan();
        let mut endpoints = BTreeMap::new();
        let mut root = json!({"Properties":{"Type":"mashup","Id":"mashup-root"},"Widgets":[
            {"Properties":{"Type":"flexcontainer","Id":"acme-container"},"Widgets":[
                {"Properties":{"Type":"autorefresh","Id":"acme-hidden","ShowControls":false},"Widgets":[]},
                {"Properties":{"Type":"autorefresh","Id":"acme-visible","ShowControls":true,"Visible":true},"Widgets":[]}
            ]}
        ]});
        migrate_widget(&mut root, "", &mut plan, &mut endpoints).unwrap();
        relocate_refresh_functions(&mut root, &BTreeSet::from(["acme-hidden"]));
        assert_eq!(root["Widgets"][0]["Widgets"].as_array().unwrap().len(), 1);
        assert_eq!(
            root["Widgets"][0]["Widgets"][0]["Properties"]["Id"],
            "acme-visible"
        );
        assert_eq!(
            root["Widgets"][1]["Properties"]["Type"],
            "autorefreshfunction"
        );
        assert_eq!(plan.kept[0].id, "acme-visible");
    }
}
