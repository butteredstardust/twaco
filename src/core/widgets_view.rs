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
    pub auto_resize: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PropertyRow {
    pub name: String,
    pub base_type: String,
    pub direction: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct WidgetDetail {
    #[serde(rename = "type")]
    pub type_id: String,
    pub type_display_name: String,
    pub aliases: Vec<String>,
    pub container: bool,
    pub bindable: bool,
    pub auto_resize: bool,
    pub properties: Vec<PropertyRow>,
    pub events: Vec<String>,
    pub services: Vec<String>,
    pub defaults_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// List the catalogue in Composer's order, optionally narrowing it by type, label or alias.
pub fn list(search: Option<&str>) -> Vec<WidgetSummary> {
    match search {
        Some(text) => catalog().search(text).into_iter().map(summary).collect(),
        None => catalog().widgets.iter().map(summary).collect(),
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
        note: (!bindable).then(|| {
            "no binding contract in the catalogue: it can be placed but not bound".to_string()
        }),
    })
}

fn summary(widget: &Widget) -> WidgetSummary {
    WidgetSummary {
        type_id: widget.type_id.clone(),
        type_display_name: widget.type_display_name.clone(),
        aliases: widget.aliases.clone(),
        container: widget.container,
        bindable: widget.has_contract(),
        auto_resize: widget.auto_resize,
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
        .filter(|(_, contract)| contract.source || contract.target)
        .map(|(name, contract)| PropertyRow {
            name: name.to_string(),
            base_type: contract.base_type.clone().unwrap_or_default(),
            direction: direction(contract),
        })
        .collect()
}

fn direction(contract: &PropertyContract) -> String {
    match (contract.source, contract.target) {
        (true, true) => "in/out",
        (true, false) => "out",
        (false, true) => "in",
        (false, false) => "",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::codes::{Coded, ErrorCode};

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
        assert_eq!(
            container.note.as_deref(),
            Some("no binding contract in the catalogue: it can be placed but not bound")
        );
        assert!(container.properties.is_empty());
        assert_eq!(
            detail("gizmo").unwrap_err().code(),
            ErrorCode::InvalidArguments
        );
    }
}
