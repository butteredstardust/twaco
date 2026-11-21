//! A compact, binding-oriented view of the embedded widget catalogue.

use super::widgets::{catalog, CatalogError, PropertyContract, Widget};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct WidgetSummary {
    #[serde(rename = "type")]
    pub type_id: String,
    pub type_display_name: String,
    pub aliases: Vec<String>,
    pub container: bool,
    pub bindable: bool,
    pub(crate) auto_resize: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PropertyRow {
    pub name: String,
    pub base_type: String,
    pub direction: String,
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct WidgetDetail {
    #[serde(rename = "type")]
    pub type_id: String,
    pub type_display_name: String,
    pub aliases: Vec<String>,
    pub(crate) container: bool,
    pub(crate) bindable: bool,
    pub(crate) auto_resize: bool,
    pub properties: Vec<PropertyRow>,
    pub events: Vec<String>,
    pub services: Vec<String>,
    pub(crate) defaults_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// List the catalogue in Composer's order, optionally narrowing it by type, label, alias or discovery hint.
pub fn list(search: Option<&str>) -> Vec<WidgetSummary> {
    match search {
        Some(text) => {
            let wanted = text.to_ascii_lowercase().replace('-', " ");
            let words: Vec<_> = wanted.split_whitespace().collect();
            catalog()
                .widgets
                .iter()
                .filter(|widget| {
                    widget
                        .names()
                        .chain(std::iter::once(widget.type_display_name.as_str()))
                        .chain(discovery_hint(&widget.type_id))
                        .any(|name| {
                            let name = name.to_ascii_lowercase().replace('-', " ");
                            words.iter().all(|word| name.contains(word))
                        })
                })
                .map(summary)
                .collect()
        }
        None => catalog().widgets.iter().map(summary).collect(),
    }
}

/// Why a searched word finds nothing although Composer knows the thing it names, and the closest
/// native widget to use instead.
pub fn absent_note(search: &str) -> Option<&'static str> {
    let wanted = search.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| wanted.contains(word));
    if has(&["gauge", "dial", "meter"]) {
        Some("ThingWorx 10.2 has a legacy `gauge` type that twaco cannot author (no contract); the closest native widget is ptcskpidial, the KPI dial: twaco widgets ptcskpidial")
    } else if has(&["webframe", "web frame", "iframe", "frame"]) {
        Some("Web Frame (`webframe`) is a legacy widget twaco cannot author (no contract). To show a page or a document, open it from a Link (ptcslink), or a navigation function with twaco mashup recipe popup; an image can go in ptcsimage")
    } else {
        None
    }
}

/// One widget's bindable surface, named by type or alias.
pub fn detail(word: &str) -> Result<WidgetDetail, CatalogError> {
    let widget = catalog().resolve(word)?;
    let bindable = widget.has_contract();
    Ok(WidgetDetail {
        type_id: widget.type_id.clone(),
        type_display_name: widget.type_display_name.clone(),
        aliases: widget.aliases.clone(),
        container: widget.container,
        bindable,
        auto_resize: widget.auto_resize,
        properties: properties(widget),
        events: widget.events.clone().unwrap_or_default(),
        services: widget.services.clone().unwrap_or_default(),
        defaults_count: widget.defaults.len(),
        note: if bindable {
            schema_note(&widget.type_id).map(str::to_string)
        } else {
            Some("no binding contract in the catalogue: it can be placed but not bound".to_string())
        },
    })
}

/// Structure of a property the catalogue types only as a JSON value.
fn schema_note(type_id: &str) -> Option<&'static str> {
    match type_id {
        "ptcstabset" => Some(
            "Items is a JSON array of objects, one per tab: [{\"name\":\"Orders\",\"value\":\"TabValue1\",\"visible\":true,\"disabled\":false}]. Tab<n>Name and Tab<n>Value carry the same text and value; NumberOfTabs says how many. mashup add keeps all of them in step and names an unnamed tab `Tab <n>`; set NumberOfTabs (not Items) to add or remove tabs.",
        ),
        _ => None,
    }
}

fn discovery_hint(type_id: &str) -> Option<&'static str> {
    match type_id {
        "ptcsrte" => Some("Display-only HTML: set ReadOnly=true and HideToolbar=true on ptcsrte."),
        "flexcontainer" => {
            Some("For a collapsible container, see twaco mashup recipe collapsible --help.")
        }
        _ => None,
    }
}

fn summary(widget: &Widget) -> WidgetSummary {
    WidgetSummary {
        type_id: widget.type_id.clone(),
        type_display_name: widget.type_display_name.clone(),
        aliases: widget.aliases.clone(),
        container: widget.container,
        bindable: widget.has_contract(),
        auto_resize: widget.auto_resize,
        hint: discovery_hint(&widget.type_id).map(str::to_string),
    }
}

fn properties(widget: &Widget) -> Vec<PropertyRow> {
    let Some(own) = &widget.properties else {
        return Vec::new();
    };
    let mut all: BTreeMap<&str, &PropertyContract> = own
        .iter()
        .map(|(name, contract)| (name.as_str(), contract))
        .collect();
    for (name, contract) in &catalog().base_properties {
        all.entry(name).or_insert(contract);
    }
    all.into_iter()
        .filter(|(_, contract)| contract.source || contract.target || contract.editable)
        .map(|(name, contract)| PropertyRow {
            name: name.to_string(),
            base_type: contract.base_type.clone().unwrap_or_default(),
            direction: direction(contract),
            default: widget.defaults.get(name).cloned(),
        })
        .collect()
}

fn direction(contract: &PropertyContract) -> String {
    match (contract.source, contract.target) {
        (true, true) => "in/out",
        (true, false) => "out",
        (false, true) => "in",
        (false, false) => "setting",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::codes::{Coded, ErrorCode};

    #[test]
    fn a_widget_composer_has_but_the_catalogue_lacks_names_the_closest_one() {
        for word in ["gauge", "webframe", "Web Frame", "iframe"] {
            assert!(list(Some(word)).is_empty(), "{word}");
        }
        assert!(absent_note("gauge").unwrap().contains("ptcskpidial"));
        assert!(absent_note("webframe").unwrap().contains("ptcslink"));
        assert!(absent_note("grid").is_none());
        let error = catalog().resolve("webframe").unwrap_err().to_string();
        assert!(error.contains("legacy widget"), "{error}");
        assert!(!list(Some("kpi")).is_empty());
    }

    #[test]
    fn the_tab_set_detail_documents_the_items_objects() {
        let note = detail("tabs").unwrap().note.unwrap();
        assert!(
            note.contains(r#""name":"Orders""#) && note.contains("Tab <n>"),
            "{note}"
        );
        assert!(detail("button").unwrap().note.is_none());
    }

    #[test]
    fn list_keeps_catalogue_order_and_searches_names_ignoring_case() {
        let all = list(None);
        let expected: Vec<&str> = catalog()
            .widgets
            .iter()
            .map(|widget| widget.type_id.as_str())
            .collect();
        assert_eq!(
            all.iter()
                .map(|widget| widget.type_id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        let charts = list(Some("chart"));
        assert!(!charts.is_empty());
        assert!(charts.iter().all(|widget| widget.type_id.contains("chart")));
        assert_eq!(list(Some("Grid"))[0].type_id, "ptcsgrid");
    }

    #[test]
    fn search_normalizes_alias_words() {
        for query in ["date picker", "picker date", "DATE-PICKER"] {
            assert!(list(Some(query))
                .iter()
                .any(|w| w.type_id == "ptcsdatepicker"));
        }
        assert!(list(Some("picker nonexistent")).is_empty());
    }

    #[test]
    fn detail_describes_bindings_events_and_unbound_widgets() {
        let grid = detail("grid").unwrap();
        assert!(grid
            .properties
            .iter()
            .any(|property| property.name == "Data"
                && property.base_type == "INFOTABLE"
                && property.direction == "in"));
        let text = detail("text-field").unwrap();
        assert!(text
            .properties
            .iter()
            .any(|property| property.name == "Text" && property.direction == "in/out"));
        let button = detail("button").unwrap();
        assert!(button.events.iter().any(|event| event == "Clicked"));
        assert!(button
            .services
            .iter()
            .any(|service| service == "TriggerClick"));
        assert!(button
            .properties
            .iter()
            .any(|property| property.name == "Visible"));
        let container = detail("container").unwrap();
        assert!(container.note.is_none());
        assert!(container
            .properties
            .iter()
            .any(|property| property.name == "CustomClass" && property.direction == "in/out"));
        assert!(container
            .properties
            .iter()
            .any(|property| property.name == "Visible" && property.direction == "in"));
        assert_eq!(
            detail("gizmo").unwrap_err().code(),
            ErrorCode::InvalidArguments
        );
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    #[test]
    fn text_field_lists_trailing_zero_setting_and_default() {
        let detail = detail("ptcstextfield").unwrap();
        let property = detail
            .properties
            .iter()
            .find(|property| property.name == "DisplayTrailingZeros")
            .unwrap();
        assert_eq!(property.base_type, "BOOLEAN");
        assert_eq!(property.default, Some(serde_json::Value::Bool(true)));
        assert_eq!(
            catalog()
                .by_type("ptcstextfield")
                .unwrap()
                .property("DisplayTrailingZeros")
                .unwrap()
                .base_type
                .as_deref(),
            Some("BOOLEAN")
        );
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    #[test]
    fn html_and_collapsible_are_discovery_hints_without_authoring_aliases() {
        let html = list(Some("HTML"));
        let rte = html
            .iter()
            .find(|widget| widget.type_id == "ptcsrte")
            .unwrap();
        let hint = rte.hint.as_deref().unwrap();
        assert!(hint.contains("ReadOnly=true") && hint.contains("HideToolbar=true"));
        let collapsible = list(Some("collapsible"));
        assert!(collapsible.iter().any(|widget| widget
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains("twaco mashup recipe collapsible")));
        assert!(catalog().resolve("html").is_err());
        assert!(catalog().resolve("collapsible").is_err());
        let tabs = catalog().by_type("ptcstabset").unwrap();
        for property in [
            "NumberOfTabs",
            "Tab1Name",
            "Tab2Name",
            "Tab3Name",
            "TabStyle",
        ] {
            assert!(
                tabs.property(property).is_some() || tabs.defaults.contains_key(property),
                "{property}"
            );
        }
        assert!(tabs
            .property("TabStyle")
            .unwrap()
            .options
            .contains(&serde_json::json!("contained-tabs")));
    }
}
