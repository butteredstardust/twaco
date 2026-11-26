//! Offline translations from Composer's 10.2 widget migration tables.

use super::{finish_edit, load_existing, AuthoringError, EditPlan};
use crate::core::{config::Solution, mashup, widgets};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::LazyLock;

static TABLE: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../widgets/migrations.json"))
        .expect("the embedded migration overlay is JSON")
});

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
    /// Widgets left as they are: no Composer migrator, or not in a parent the migrator accepts.
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
    names: BTreeMap<String, String>,
    properties: Value,
}

fn mapped_name<'a>(rule: &'a Value, name: &'a str) -> &'a str {
    rule["mappings"][name]
        .as_str()
        .or_else(|| rule["mappings"][name]["newproperty"].as_str())
        .unwrap_or(name)
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
    let rule = &TABLE["newtypes"][&kind];
    let legacy = !rule.is_null()
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
        let target = rule["newtype"].as_str();
        let refusal = if kind == "container" && parent != "ptcsdynamicpanel" {
            Some("Composer only converts container inside ptcsdynamicpanel".to_string())
        } else if target.is_none() {
            Some(
                "no supported Composer newtype migrator; keep this widget or replace it manually"
                    .to_string(),
            )
        } else {
            None
        };
        if let Some(reason) = refusal {
            plan.kept.push(Kept {
                id: id.clone(),
                name,
                widget: kind.clone(),
                reason,
            });
        } else if let Some(target) = target {
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
            let mut props = contract.defaults.clone();
            props.extend(if target == "flexcontainer" {
                catalog.dropped.flexcontainer.clone()
            } else {
                catalog.dropped.ptcs.clone()
            });
            let mut dropped = vec![];
            let mut names = BTreeMap::new();
            for (old, value) in &original {
                let new = mapped_name(rule, old);
                let mapping = &rule["mappings"][old];
                let explicitly_mapped = !mapping.is_null();
                let deleted = rule["deletions"]
                    .as_array()
                    .is_some_and(|d| d.iter().any(|n| n == old));
                let supported = contract.defaults.contains_key(new)
                    || contract
                        .properties
                        .as_ref()
                        .is_some_and(|p| p.contains_key(new))
                    || catalog.base_properties.contains_key(new)
                    || widgets::dynamic_property(
                        target,
                        &Value::Object(original.clone()),
                        new,
                        false,
                    )
                    .is_some()
                    || matches!(
                        new,
                        "Id" | "DisplayName" | "Left" | "Top" | "Width" | "Height" | "ZIndex"
                    );
                if old == "Type" || old == "__TypeDisplayName" {
                    continue;
                }
                if (!deleted || explicitly_mapped) && (supported || explicitly_mapped) {
                    let mut value = value.clone();
                    if let Some(values) = mapping["values"].as_object() {
                        let key = value
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| value.to_string());
                        // Composer retains a value absent from its translation table.
                        if let Some(mapped) = values.get(&key) {
                            value = mapped.clone();
                        }
                    }
                    props.insert(new.into(), value);
                    names.insert(old.clone(), new.into());
                } else {
                    dropped.push(old.clone());
                }
            }
            // Events and services need mapping even when Composer stores no property value.
            for (old, mapping) in rule["mappings"].as_object().into_iter().flatten() {
                if let Some(new) = mapping.as_str().or_else(|| mapping["newproperty"].as_str()) {
                    names.insert(old.clone(), new.into());
                }
            }
            for (key, value) in rule["create"].as_object().into_iter().flatten() {
                props.insert(key.clone(), value.clone());
            }
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
            widget["Properties"] = Value::Object(props);
            endpoints.insert(
                id.clone(),
                Endpoint {
                    to: target.into(),
                    names,
                    properties: widget["Properties"].clone(),
                },
            );
            plan.migrations.push(Migration {
                id,
                name,
                from: kind.clone(),
                to: target.into(),
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
                let new = endpoint.names.get(&old).unwrap_or(&old);
                let contract = widgets::catalog()
                    .by_type(&endpoint.to)
                    .expect("migration target");
                let dynamic =
                    widgets::dynamic_property(&endpoint.to, &endpoint.properties, new, true);
                let found = contract
                    .properties
                    .as_ref()
                    .and_then(|p| p.get(new))
                    .or_else(|| widgets::catalog().base_properties.get(new))
                    .or(dynamic.as_ref());
                if let Some(found) = found {
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
                let new = endpoint.names.get(&old).unwrap_or(&old);
                let contract = widgets::catalog()
                    .by_type(&endpoint.to)
                    .expect("migration target");
                let names = if is_event {
                    &contract.events
                } else {
                    &contract.services
                };
                if names.as_ref().is_some_and(|names| names.contains(new)) {
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
}
