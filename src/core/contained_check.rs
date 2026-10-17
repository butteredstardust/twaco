//! Contained mashups and navigation functions keep a copy of the parameters of the mashup they
//! show (`MashupParameters`). A binding to a parameter that copy lacks, or that the mashup no
//! longer declares, reaches nothing at runtime and no error says so.

use super::config::Solution;
use super::mashup;
use super::mashup::describe::mashup_parameters;
use super::workspace;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const GATE: &str = "contained mashups";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) file: String,
    pub(crate) rule: String,
    pub(crate) message: String,
}

#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) problems: Vec<Problem>,
    /// Containers and navigation functions whose mashup is in the repository.
    pub(crate) examined: usize,
    /// Those showing a mashup outside the repository.
    pub(crate) not_checked: usize,
    /// Caches that differ from the mashup's parameters where no binding is affected yet.
    pub(crate) unbound_drift: usize,
}

/// A widget that shows another mashup: a container (`Name`) or a navigation function
/// (`TargetMashup`).
fn shown_mashup(properties: &Value) -> Option<&str> {
    match properties["Type"].as_str()? {
        "mashupcontainer" => properties["Name"].as_str(),
        "navigationfunction" => properties["TargetMashup"].as_str(),
        _ => None,
    }
    .filter(|name| !name.is_empty())
}

pub(crate) fn run(solution: &Solution) -> Report {
    let mashups: Vec<_> = workspace::discover(solution)
        .entities
        .into_iter()
        .filter(|file| file.info.collection == "Mashups")
        .collect();
    let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut contents = Vec::new();
    for file in &mashups {
        let Ok(bytes) = std::fs::read(&file.path) else {
            continue;
        };
        if let Ok(parameters) = mashup_parameters(&bytes) {
            declared.insert(
                file.info.name.clone(),
                parameters.into_iter().map(|p| p.name).collect(),
            );
        }
        // Invalid mashup JSON belongs to the project gate.
        let Some(content) = mashup::extract(&bytes)
            .ok()
            .and_then(|assets| serde_json::from_str::<Value>(&assets.content).ok())
        else {
            continue;
        };
        let relative = file
            .path
            .strip_prefix(&solution.root)
            .unwrap_or(&file.path)
            .to_string_lossy()
            .replace('\\', "/");
        contents.push((relative, content));
    }
    let mut report = Report::default();
    for (file, content) in &contents {
        check_mashup(file, content, &declared, &mut report);
    }
    report
}

fn check_mashup(
    file: &str,
    content: &Value,
    declared: &BTreeMap<String, BTreeSet<String>>,
    report: &mut Report,
) {
    // id -> (shown mashup, cached parameter names)
    let mut shows: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    fn visit(widget: &Value, shows: &mut BTreeMap<String, (String, BTreeSet<String>)>) {
        let properties = &widget["Properties"];
        if let (Some(id), Some(child)) = (properties["Id"].as_str(), shown_mashup(properties)) {
            let cached = properties["MashupParameters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["ParameterName"].as_str().map(str::to_string))
                .collect();
            shows.insert(id.to_string(), (child.to_string(), cached));
        }
        for child in widget["Widgets"].as_array().into_iter().flatten() {
            visit(child, shows);
        }
    }
    visit(&content["UI"], &mut shows);
    let mut bound: BTreeSet<(String, String)> = BTreeSet::new();
    for binding in content["DataBindings"].as_array().into_iter().flatten() {
        // Only the first property map applies to a container; it is the one that matters.
        let map = &binding["PropertyMaps"][0];
        for (id, property) in [
            (binding["TargetId"].as_str(), map["TargetProperty"].as_str()),
            (binding["SourceId"].as_str(), map["SourceProperty"].as_str()),
        ] {
            let (Some(id), Some(property)) = (id, property) else {
                continue;
            };
            let Some((child, cached)) = shows.get(id) else {
                continue;
            };
            let Some(parameters) = declared.get(child) else {
                continue;
            };
            // A name in neither list is the widget's own property, such as Visible or URL.
            let in_cache = cached.contains(property);
            let in_child = parameters.contains(property) || property == "Entity";
            let rule = match (in_cache, in_child) {
                (true, false) => "parameter-not-in-mashup",
                (false, true) => "parameter-not-cached",
                _ => continue,
            };
            bound.insert((id.to_string(), property.to_string()));
            let message = if rule == "parameter-not-in-mashup" {
                format!(
                    "{id} binds {property}, which {child} no longer declares; the value reaches nothing (remove the binding, or add the parameter back with `twaco mashup param add`)"
                )
            } else {
                format!(
                    "{id} binds {property}, which its copy of {child}'s parameters lacks, so the value is dropped at runtime; `twaco mashup param set {child} {property}` refreshes every copy"
                )
            };
            report.problems.push(Problem {
                file: file.to_string(),
                rule: rule.to_string(),
                message,
            });
        }
    }
    for (id, (child, cached)) in &shows {
        let Some(parameters) = declared.get(child) else {
            report.not_checked += 1;
            continue;
        };
        report.examined += 1;
        let differs = parameters
            .iter()
            .filter(|p| !p.starts_with("ConfigForm_"))
            .any(|p| !cached.contains(p))
            || cached
                .iter()
                .any(|p| !parameters.contains(p) && p != "Entity");
        if differs && !bound.iter().any(|(bound_id, _)| bound_id == id) {
            report.unbound_drift += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn declared() -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::from([(
            "Acme.Card_MU".to_string(),
            BTreeSet::from(["id".to_string(), "title".to_string()]),
        )])
    }

    fn content(cached: &[&str], bindings: Value) -> Value {
        let cache: Vec<Value> = cached
            .iter()
            .map(|name| json!({"ParameterName": name, "BaseType": "STRING"}))
            .collect();
        json!({
            "UI": {"Properties": {"Id": "mashup-root"}, "Widgets": [
                {"Properties": {"Id": "mashupcontainer-7", "Type": "mashupcontainer",
                    "Name": "Acme.Card_MU", "MashupParameters": cache}, "Widgets": []},
                {"Properties": {"Id": "navigationfunction-9", "Type": "navigationfunction",
                    "TargetMashup": "Acme.Nope_MU", "MashupParameters": []}, "Widgets": []}
            ]},
            "DataBindings": bindings
        })
    }

    fn binding(target: &str, property: &str) -> Value {
        json!({"SourceId": "ptcslabel-3", "TargetId": target,
            "PropertyMaps": [{"SourceProperty": "LabelText", "TargetProperty": property}]})
    }

    #[test]
    fn a_binding_the_cache_or_the_mashup_lacks_is_found_and_the_rest_is_counted() {
        let mut report = Report::default();
        let bindings = json!([
            binding("mashupcontainer-7", "title"),
            binding("mashupcontainer-7", "old"),
            binding("mashupcontainer-7", "Visible"),
            binding("navigationfunction-9", "anything")
        ]);
        check_mashup(
            "acme/Mashups/Acme.Board_MU.xml",
            &content(&["id", "old"], bindings),
            &declared(),
            &mut report,
        );
        let rules: Vec<(&str, &str)> = report
            .problems
            .iter()
            .map(|p| (p.rule.as_str(), p.message.split(' ').nth(2).unwrap()))
            .collect();
        assert_eq!(
            rules,
            [
                ("parameter-not-cached", "title,"),
                ("parameter-not-in-mashup", "old,")
            ]
        );
        assert_eq!(report.examined, 1);
        assert_eq!(
            report.not_checked, 1,
            "the navigation function's mashup is not here"
        );
        assert_eq!(
            report.unbound_drift, 0,
            "the drifted container is reported through its bindings"
        );
    }

    #[test]
    fn a_cache_that_differs_with_nothing_bound_is_only_counted() {
        let mut report = Report::default();
        check_mashup(
            "x.xml",
            &content(&["id"], json!([])),
            &declared(),
            &mut report,
        );
        assert!(report.problems.is_empty());
        assert_eq!(report.unbound_drift, 1);
        let mut report = Report::default();
        check_mashup(
            "x.xml",
            &content(
                &["id", "title"],
                json!([binding("mashupcontainer-7", "title")]),
            ),
            &declared(),
            &mut report,
        );
        assert!(report.problems.is_empty());
        assert_eq!(report.unbound_drift, 0);
    }
}
