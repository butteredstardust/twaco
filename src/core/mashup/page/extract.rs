use super::{baseline::Baseline, compile, derive};
use crate::core::{
    config::Solution,
    mashup::authoring::{self, AuthoringError},
    workspace,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// A page spec and how much of it is elements. `elements` counts nodes written as elements or
/// components, `raw` the `widget:` nodes (by type in `raw_types`), and `widgets` the widgets of
/// the mashup they stand for.
#[derive(Debug, Serialize)]
pub struct Extraction {
    pub mashup: String,
    pub elements: usize,
    pub raw: usize,
    pub exact: bool,
    pub spec: String,
    pub widgets: usize,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub raw_types: BTreeMap<String, usize>,
    /// Widgets that differ from the last build, when the spec is the page's own page.yaml.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hand_edited: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn invalid(message: impl ToString) -> AuthoringError {
    AuthoringError::Invalid(message.to_string())
}

const ELEMENTS: [&str; 32] = [
    "component",
    "grid",
    "label",
    "heading",
    "button",
    "widget",
    "row",
    "column",
    "page_header",
    "toolbar",
    "kpi_row",
    "card",
    "filter_panel",
    "tabs",
    "chart",
    "spacer",
    "date_range",
    "dropdown",
    "text_field",
    "pagination",
    "trend_tile",
    "detail_pair",
    "panel",
    "slider",
    "kpi_dial",
    "side_panel",
    "list",
    "collection",
    "nav_menu",
    "card_grid",
    "text",
    "block",
];

/// Count the nodes of a spec file: element nodes and `widget` nodes, wherever they nest.
fn nodes(spec: &Value, elements: &mut usize, raw: &mut usize, types: &mut BTreeMap<String, usize>) {
    match spec {
        Value::Object(map) => {
            if map.len() == 1 {
                let (key, body) = map.iter().next().expect("one entry");
                if ELEMENTS.contains(&key.as_str()) && body.is_object() {
                    if key == "widget" {
                        *raw += 1;
                        let kind = body["type"].as_str().unwrap_or("unknown");
                        *types.entry(kind.into()).or_default() += 1;
                    } else {
                        *elements += 1;
                    }
                }
            }
            for value in map.values() {
                nodes(value, elements, raw, types);
            }
        }
        Value::Array(items) => {
            for item in items {
                nodes(item, elements, raw, types);
            }
        }
        _ => {}
    }
}

fn counted(name: &str, spec: String, widgets: usize, exact: bool) -> Extraction {
    let (mut elements, mut raw, mut raw_types) = (0, 0, BTreeMap::new());
    if let Ok(value) = serde_saphyr::from_str::<Value>(&spec) {
        nodes(&value, &mut elements, &mut raw, &mut raw_types);
    }
    Extraction {
        mashup: name.into(),
        elements,
        raw,
        exact,
        spec,
        widgets,
        raw_types,
        hand_edited: vec![],
        notes: vec![],
    }
}

fn count_widgets(content: &Value) -> usize {
    fn walk(widget: &Value) -> usize {
        widget["Widgets"]
            .as_array()
            .map_or(0, |kids| kids.len() + kids.iter().map(walk).sum::<usize>())
    }
    walk(&content["UI"])
}

/// Widgets by id with their own properties and their children's ids.
fn flatten(widget: &Value, out: &mut BTreeMap<String, Value>) {
    let mut own = widget.clone();
    let kids: Vec<Value> = widget["Widgets"].as_array().cloned().unwrap_or_default();
    own["Widgets"] = Value::Array(kids.iter().map(|k| k["Properties"]["Id"].clone()).collect());
    if let Some(id) = widget["Properties"]["Id"].as_str() {
        out.insert(id.into(), own);
    }
    for kid in &kids {
        flatten(kid, out);
    }
}

/// The widgets that differ between what was built and what is there now, named by their node in
/// the spec where the baseline knows it.
fn hand_edits(current: &Value, baseline: &Baseline) -> Vec<String> {
    let (mut now, mut before) = (BTreeMap::new(), BTreeMap::new());
    flatten(&current["UI"], &mut now);
    flatten(&baseline.content["UI"], &mut before);
    let nodes: BTreeMap<&str, &str> = baseline
        .ids
        .iter()
        .map(|(node, widget)| (widget.as_str(), node.as_str()))
        .collect();
    let named = |id: &str| match nodes.get(id) {
        Some(node) => format!("{node} ({id})"),
        None => id.to_string(),
    };
    let mut changed = BTreeSet::new();
    for id in now.keys().chain(before.keys()) {
        if now.get(id) != before.get(id) {
            changed.insert(named(id));
        }
    }
    let mut out: Vec<String> = changed.into_iter().collect();
    for part in ["Data", "DataBindings", "Events"] {
        if current.get(part) != baseline.content.get(part) {
            out.push(format!("{part} (wiring)"));
        }
    }
    out
}

/// Derive a spec offline. A mashup built from a page.yaml returns that file when it still matches
/// its baseline, and also when it does not, with the hand-edited widgets named; other mashups get
/// a spec derived from their widgets and wiring.
pub fn extract(solution: &Solution, name: &str) -> Result<Extraction, AuthoringError> {
    let loaded = authoring::load_existing(solution, name)?;
    let content = loaded.assets.content;
    let css = loaded.assets.css;
    let project = workspace::entities(solution)
        .into_iter()
        .find(|e| e.info.collection == "Mashups" && e.info.name == name)
        .map(|e| e.info.project)
        .ok_or_else(|| invalid(format!("unknown mashup {name}")))?;
    let directory = solution.src_root().join(name);
    let value: Value = serde_json::from_str(&content).map_err(invalid)?;
    let widgets = count_widgets(&value);
    if let Ok(spec) = std::fs::read_to_string(directory.join("page.yaml")) {
        let baseline = std::fs::read(directory.join(".page-baseline.json"))
            .ok()
            .map(|bytes| Baseline::read(&bytes));
        match baseline {
            Some(Ok(baseline)) => {
                let exact = baseline.content == value && baseline.css == css;
                let mut found = counted(name, spec, widgets, exact);
                if !exact {
                    found.hand_edited = hand_edits(&value, &baseline);
                    found.notes.push(
                        "the mashup was edited by hand after its last build; this is its page.yaml, which does not describe those edits"
                            .into(),
                    );
                }
                return Ok(found);
            }
            Some(Err(_)) => {}
            None => {
                if let Ok(built) = compile::compile(solution, &project, &spec) {
                    if built.content == content && built.css == css {
                        return Ok(counted(name, spec, widgets, true));
                    }
                }
            }
        }
    }
    let derived = derive::derive(name, &content).map_err(invalid)?;
    let mut notes = derived.notes;
    if !css.trim().is_empty() {
        notes.push("custom CSS of the mashup is not part of a spec".into());
    }
    Ok(Extraction {
        mashup: name.into(),
        elements: derived.elements,
        raw: derived.raw,
        exact: false,
        spec: derived.spec,
        widgets: derived.widgets,
        raw_types: derived.raw_types,
        hand_edited: vec![],
        notes,
    })
}

/// Derive a spec and atomically write an explicitly named output while holding the workspace lock.
pub fn export(
    solution: &Solution,
    name: &str,
    out: Option<&std::path::Path>,
) -> Result<Extraction, AuthoringError> {
    let _lock = out
        .map(|_| {
            crate::core::lock::acquire_for(solution, "mashup spec")
                .map_err(|e| AuthoringError::Refused(e.to_string()))
        })
        .transpose()?;
    let result = extract(solution, name)?;
    if let Some(path) = out {
        workspace::atomic_replace(path, result.spec.as_bytes())
            .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    }
    Ok(result)
}

/// A spec derived from content text, for the tests: no solution, no hand edits.
#[cfg(test)]
pub(crate) fn derive_content(name: &str, content: &str) -> Result<Extraction, super::Error> {
    let derived = derive::derive(name, content)?;
    let value: Value = serde_json::from_str(content)
        .map_err(|e| super::Error::at(serde_saphyr::Location::UNKNOWN, e))?;
    Ok(Extraction {
        mashup: name.into(),
        elements: derived.elements,
        raw: derived.raw,
        exact: false,
        spec: derived.spec,
        widgets: count_widgets(&value),
        raw_types: derived.raw_types,
        hand_edited: vec![],
        notes: derived.notes,
    })
}
