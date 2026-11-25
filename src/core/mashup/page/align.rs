//! Adoption: find the widgets and wiring of an existing mashup in what a spec generates, so the
//! generated page keeps their ids instead of the hashed ones.

use super::{
    compile::{self, Artifacts},
    ids::Pins,
    merge::{self, Difference},
};
use crate::core::config::Solution;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn kind(widget: &Value) -> &str {
    widget["Properties"]["Type"].as_str().unwrap_or("")
}

fn children(widget: &Value) -> &[Value] {
    widget["Widgets"].as_array().map_or(&[], Vec::as_slice)
}

/// Pairs of indexes that line the two child lists up by widget type (a longest common subsequence),
/// so one added or missing widget does not shift the rest.
fn pair(current: &[Value], generated: &[Value]) -> Vec<(usize, usize)> {
    let is_function = |widget: &Value| {
        crate::core::widgets::catalog()
            .by_type(kind(widget))
            .is_some_and(|w| w.function)
    };
    // Function widgets have no place on the page and a spec lists them apart from the layout, so
    // their order against the layout means nothing. Prefer an existing id or name, then type.
    let split = |items: &[Value]| -> (Vec<usize>, Vec<usize>) {
        (0..items.len()).partition(|&i| !is_function(&items[i]))
    };
    let ((have_visible, have_functions), (made_visible, made_functions)) =
        (split(current), split(generated));
    if have_functions.is_empty() && made_functions.is_empty() {
        return pair_by_type(current, generated);
    }
    let pick = |items: &[Value], at: &[usize]| -> Vec<Value> {
        at.iter().map(|&i| items[i].clone()).collect()
    };
    let mut out: Vec<(usize, usize)> = pair_by_type(
        &pick(current, &have_visible),
        &pick(generated, &made_visible),
    )
    .into_iter()
    .map(|(i, j)| (have_visible[i], made_visible[j]))
    .collect();
    let mut left: Vec<usize> = have_functions;
    for &j in &made_functions {
        if let Some(at) = left
            .iter()
            .position(|&i| {
                kind(&current[i]) == kind(&generated[j])
                    && current[i]["Properties"]["Id"] == generated[j]["Properties"]["Id"]
            })
            .or_else(|| {
                left.iter().position(|&i| {
                    kind(&current[i]) == kind(&generated[j])
                        && current[i]["Properties"]["DisplayName"].is_string()
                        && current[i]["Properties"]["DisplayName"]
                            == generated[j]["Properties"]["DisplayName"]
                })
            })
            .or_else(|| {
                left.iter()
                    .position(|&i| kind(&current[i]) == kind(&generated[j]))
            })
        {
            out.push((left.remove(at), j));
        }
    }
    out.sort_unstable();
    out
}

fn pair_by_type(current: &[Value], generated: &[Value]) -> Vec<(usize, usize)> {
    let (n, m) = (current.len(), generated.len());
    let mut best = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            best[i][j] = if kind(&current[i]) == kind(&generated[j]) {
                best[i + 1][j + 1] + 1
            } else {
                best[i + 1][j].max(best[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, vec![]);
    while i < n && j < m {
        if kind(&current[i]) == kind(&generated[j]) {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if best[i + 1][j] >= best[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

fn ids<'a>(widget: &'a Value, out: &mut BTreeSet<&'a str>) {
    if let Some(id) = widget["Properties"]["Id"].as_str() {
        out.insert(id);
    }
    for child in children(widget) {
        ids(child, out);
    }
}

fn walk(current: &Value, generated: &Value, found: &mut BTreeMap<String, String>) {
    if kind(current) != kind(generated) {
        return;
    }
    if let (Some(have), Some(made)) = (
        current["Properties"]["Id"].as_str(),
        generated["Properties"]["Id"].as_str(),
    ) {
        if have != made {
            found.insert(made.into(), have.into());
        }
    }
    let (have, made) = (children(current), children(generated));
    for (i, j) in pair(have, made) {
        walk(&have[i], &made[j], found);
    }
}

/// The pins that make `generated` use the widget ids of `current` wherever the two trees line up.
/// `allocated` is what the id allocator handed out as `(node, role, id)`.
pub(super) fn widgets(
    current: &Value,
    generated: &Value,
    allocated: &[(String, String, String)],
) -> Pins {
    let mut found = BTreeMap::new();
    walk(&current["UI"], &generated["UI"], &mut found);
    let mut present = BTreeSet::new();
    ids(&generated["UI"], &mut present);
    let mut taken: BTreeSet<&str> = BTreeSet::new();
    let mut pins = Pins::default();
    for (node, role, hashed) in allocated {
        let Some(have) = found.get(hashed) else {
            continue;
        };
        // Two widgets cannot share an id, and a kept id may not be one that stays with another.
        let stays = present.contains(have.as_str()) && !found.contains_key(have);
        if stays || !taken.insert(have) {
            continue;
        }
        pins.widgets
            .push((node.clone(), role.clone(), have.clone()));
    }
    pins
}

/// What a binding or event connects, without how it is described: source and target ends, and
/// the property pairs of a binding.
fn endpoints(item: &Value) -> Value {
    let pick = |keys: &[&str]| -> Vec<Value> { keys.iter().map(|k| item[*k].clone()).collect() };
    let maps: Vec<Value> = item["PropertyMaps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| serde_json::json!([m["SourceProperty"], m["TargetProperty"]]))
        .collect();
    serde_json::json!([
        pick(&[
            "SourceArea",
            "SourceId",
            "SourceSection",
            "TargetArea",
            "TargetId",
            "TargetSection",
            "EventTriggerArea",
            "EventTriggerId",
            "EventTriggerSection",
            "EventTriggerEvent",
            "EventHandlerArea",
            "EventHandlerId",
            "EventHandlerService",
        ]),
        maps,
        if matches!(
            item["SourceArea"].as_str(),
            Some("Data" | "Session" | "UserExtensions")
        ) {
            item["SourceDetails"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("AllData")
        } else {
            item["SourceDetails"].as_str().unwrap_or("")
        }
    ])
}

fn without_id(value: &Value) -> Value {
    let mut copy = value.clone();
    if let Some(map) = copy.as_object_mut() {
        map.remove("Id");
    }
    copy
}

/// The pins for data sections, bindings and events. A data section and a service call of the same
/// name and target keep the id the mashup has; a generated binding or event takes the id of the
/// first unclaimed current one that is the same apart from its id.
pub(super) fn wiring(current: &Value, generated: &Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (section, made) in generated["Data"].as_object().into_iter().flatten() {
        let have = &current["Data"][section];
        let mut pin = |made: &Value, have: &Value| {
            if let (Some(made), Some(have)) = (made["Id"].as_str(), have["Id"].as_str()) {
                if made != have {
                    out.insert(made.into(), have.into());
                }
            }
        };
        pin(made, have);
        for service in made["Services"].as_array().into_iter().flatten() {
            let found = have["Services"].as_array().and_then(|calls| {
                calls
                    .iter()
                    .find(|c| c["Name"] == service["Name"] && c["Target"] == service["Target"])
            });
            if let Some(found) = found {
                pin(service, found);
            }
        }
    }
    for part in ["DataBindings", "Events"] {
        let have = current[part].as_array().map_or(&[][..], Vec::as_slice);
        let made = generated[part].as_array().map_or(&[][..], Vec::as_slice);
        let mut claimed = vec![false; have.len()];
        // Content first; then, for what is left, the same endpoints, since a mashup written by
        // hand differs from a generated one in details such as base types.
        let mut matched: Vec<Option<usize>> = made
            .iter()
            .map(|item| {
                let bare = without_id(item);
                let found =
                    (0..have.len()).find(|&i| !claimed[i] && without_id(&have[i]) == bare)?;
                claimed[found] = true;
                Some(found)
            })
            .collect();
        for (item, slot) in made.iter().zip(matched.iter_mut()) {
            if slot.is_none() {
                let ends = endpoints(item);
                if let Some(i) =
                    (0..have.len()).find(|&i| !claimed[i] && endpoints(&have[i]) == ends)
                {
                    claimed[i] = true;
                    *slot = Some(i);
                }
            }
        }
        for (item, found) in made.iter().zip(matched) {
            let Some(id) = item["Id"].as_str() else {
                continue;
            };
            if let Some(i) = found {
                claimed[i] = true;
                if let Some(kept) = have[i]["Id"].as_str().filter(|kept| *kept != id) {
                    out.insert(id.into(), kept.into());
                }
            }
        }
    }
    out
}

/// The generated page with the ids of the widgets and wiring it lines up with in `current`, or
/// `None` when nothing needs renaming.
pub(super) fn adopt(
    solution: &Solution,
    project: &str,
    yaml: &str,
    base: &Path,
    current: &Value,
    generated: &Artifacts,
) -> Option<Artifacts> {
    let content: Value = serde_json::from_str(&generated.content).ok()?;
    let mut pins = widgets(current, &content, &generated.allocations);
    let mut made = None;
    if !pins.widgets.is_empty() {
        made = Some(compile::compile_pinned(solution, project, yaml, base, &pins).ok()?);
    }
    let latest: Value = serde_json::from_str(&made.as_ref().unwrap_or(generated).content).ok()?;
    pins.wiring = wiring(current, &latest);
    if !pins.wiring.is_empty() {
        made = Some(compile::compile_pinned(solution, project, yaml, base, &pins).ok()?);
    }
    made
}

/// `(widget id, property)` of everything a binding of the mashup writes into.
fn bound_properties(content: &Value) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for binding in content["DataBindings"].as_array().into_iter().flatten() {
        let id = binding["TargetId"].as_str().unwrap_or_default();
        for map in binding["PropertyMaps"].as_array().into_iter().flatten() {
            if let Some(property) = map["TargetProperty"].as_str() {
                out.insert((id.to_string(), property.to_string()));
            }
        }
    }
    out
}

/// A navigation function offers the target's parameter defaults; a mashup built by hand states
/// none, and the target decides at run time, so they are no difference. Nor is the list's order.
fn strip_navigation_defaults(widget: &mut Value) {
    if kind(widget) == "navigationfunction" {
        for key in ["MashupParameters", "_currentParameterDefs"] {
            let Some(definitions) = widget["Properties"][key].as_array_mut() else {
                continue;
            };
            for definition in definitions.iter_mut() {
                if let Some(map) = definition.as_object_mut() {
                    map.remove("DefaultValue");
                }
            }
            // The platform reads the list as a set.
            definitions.sort_by_key(|d| d["ParameterName"].as_str().unwrap_or("").to_string());
        }
    }
    for child in widget["Widgets"].as_array_mut().into_iter().flatten() {
        strip_navigation_defaults(child);
    }
}

/// Widget id to type for every widget of a content tree.
fn widget_types(content: &Value) -> BTreeMap<String, String> {
    fn walk(widget: &Value, out: &mut BTreeMap<String, String>) {
        if let (Some(id), Some(kind)) = (
            widget["Properties"]["Id"].as_str(),
            widget["Properties"]["Type"].as_str(),
        ) {
            out.insert(id.into(), kind.into());
        }
        for child in widget["Widgets"].as_array().into_iter().flatten() {
            walk(child, out);
        }
    }
    let mut out = BTreeMap::new();
    walk(&content["UI"], &mut out);
    out
}

/// A property the mashup does not state, which the spec generates at the value the platform
/// assumes anyway (empty, false, zero or the catalogue default). Composer leaves those out, and
/// adopting a page must not add them.
fn is_unstated_default(difference: &merge::Difference, kinds: &BTreeMap<String, String>) -> bool {
    let segments: Vec<&str> = difference.path.split('/').collect();
    // The label Composer stores is the type id, and a number may be stored as text.
    if segments.last() == Some(&"__TypeDisplayName") {
        return true;
    }
    // Older content has no top-level mashupType; the platform reads a missing one as a plain
    // mashup, which is what a page spec generates.
    if difference.path == "/mashupType"
        && difference.current.as_ref().is_none_or(Value::is_null)
        && difference.generated == "mashup"
    {
        return true;
    }
    // The styler label and the styler version beside stored styles describe them; content
    // without them (older or hand-written) styles the widget the same.
    if difference.current.as_ref().is_none_or(Value::is_null)
        && (difference.path == "/UI/Properties/StyleProperties/meta"
            || (difference
                .path
                .starts_with("/UI/Properties/StyleProperties/data/")
                && difference.path.ends_with("/widget")))
    {
        return true;
    }
    // Composer records the numeric type it saw when the binding was made (NUMBER, INTEGER or
    // LONG); the runtime resolves the endpoint's own type, so the recorded one is no difference.
    if difference.path.ends_with("/PropertyMaps") {
        if let Some(current) = &difference.current {
            if numeric_types_alike(current) == numeric_types_alike(&difference.generated) {
                return true;
            }
        }
    }
    if let (Some(Value::String(text)), Some(number)) =
        (&difference.current, difference.generated.as_f64())
    {
        if text.trim().parse::<f64>().ok() == Some(number) {
            return true;
        }
    }
    if difference.path == "/UI/Widgets/order" {
        return same_order_without_functions(difference, kinds);
    }
    // An input Composer left as `{}` holds nothing either.
    if difference
        .current
        .as_ref()
        .is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|m| !m.is_empty()))
    {
        return false;
    }
    let generated = &difference.generated;
    if generated.is_null()
        || *generated == ""
        || *generated == false
        || generated.as_f64() == Some(0.0)
    {
        return true;
    }
    let [.., id, "Properties", name] = segments.as_slice() else {
        return false;
    };
    kinds
        .get(*id)
        .and_then(|kind| crate::core::widgets::catalog().by_type(kind))
        .is_some_and(|widget| widget.defaults.get(*name) == Some(generated))
}

/// Property maps with every numeric base type written as NUMBER.
fn numeric_types_alike(maps: &Value) -> Value {
    let mut maps = maps.clone();
    for map in maps.as_array_mut().into_iter().flatten() {
        for key in ["SourcePropertyBaseType", "TargetPropertyBaseType"] {
            if matches!(map[key].as_str(), Some("NUMBER" | "INTEGER" | "LONG")) {
                map[key] = Value::String("NUMBER".into());
            }
        }
    }
    maps
}

/// Whether two child orders agree once function widgets, which have no place on the page, are
/// left out.
fn same_order_without_functions(
    difference: &merge::Difference,
    kinds: &BTreeMap<String, String>,
) -> bool {
    let visible = |value: &Value| -> Option<Vec<String>> {
        Some(
            value
                .as_array()?
                .iter()
                .filter_map(|id| id.as_str())
                .filter(|id| {
                    !kinds
                        .get(*id)
                        .and_then(|kind| crate::core::widgets::catalog().by_type(kind))
                        .is_some_and(|widget| widget.function)
                })
                .map(String::from)
                .collect(),
        )
    };
    match (
        difference.current.as_ref().and_then(visible),
        visible(&difference.generated),
    ) {
        (Some(current), Some(generated)) => current == generated,
        _ => false,
    }
}

/// Older Composer files write a property type in lower case and leave `SourceDetails` out; the
/// platform reads them as the same binding, so neither is a difference.
fn normalise_bindings(content: &mut Value) {
    for binding in content["DataBindings"].as_array_mut().into_iter().flatten() {
        if binding.get("SourceDetails").is_none() {
            binding["SourceDetails"] = Value::String(String::new());
        }
        for map in binding["PropertyMaps"].as_array_mut().into_iter().flatten() {
            for key in ["SourcePropertyType", "TargetPropertyType"] {
                if let Some(text) = map[key].as_str() {
                    map[key] = Value::String(text.to_ascii_lowercase());
                }
            }
        }
    }
}

/// What the generated page still lacks of the current one. A bound property's own value never
/// shows, so the spec need not repeat it.
pub(super) fn remaining(current: &Value, generated: &Value) -> Vec<Difference> {
    let bound = bound_properties(current);
    // Declared parameters are a set to the platform; the spec lists them by name.
    let (mut current, mut generated) = (current.clone(), generated.clone());
    for value in [&mut current, &mut generated] {
        normalise_bindings(value);
        strip_navigation_defaults(&mut value["UI"]);
        if let Some(defs) = value
            .pointer_mut("/UI/Properties/_currentParameterDefs")
            .and_then(Value::as_array_mut)
        {
            defs.sort_by_key(|d| d["ParameterName"].as_str().unwrap_or("").to_string());
        }
    }
    let kinds = widget_types(&generated);
    merge::differences(&current, &generated)
        .into_iter()
        .filter(|difference| !is_unstated_default(difference, &kinds))
        .filter(|difference| {
            // The order of bindings and events carries no meaning, and `id_index` is bookkeeping.
            if difference.path == "/UI/Properties/id_index"
                || (difference.path.ends_with("/order")
                    && !difference.path.ends_with("/Widgets/order"))
            {
                return false;
            }
            let segments: Vec<&str> = difference.path.split('/').collect();
            !segments
                .iter()
                .rposition(|s| *s == "Properties")
                .filter(|at| *at >= 1 && segments.len() == at + 2)
                .is_some_and(|at| {
                    bound.contains(&(segments[at - 1].to_string(), segments[at + 1].to_string()))
                })
        })
        .collect()
}

/// The bindings and events of `current` that nothing in `generated` matches, as `source -> target`.
pub(super) fn lost_wiring(current: &Value, generated: &Value) -> Vec<String> {
    let mut out = vec![];
    for item in current["Events"].as_array().into_iter().flatten() {
        if generated["Events"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|made| endpoints(made) == endpoints(item))
        {
            continue;
        }
        out.push(format!(
            "event not written: {}.{} -> {}.{}",
            item["EventTriggerId"].as_str().unwrap_or(""),
            item["EventTriggerEvent"].as_str().unwrap_or(""),
            item["EventHandlerId"].as_str().unwrap_or(""),
            item["EventHandlerService"].as_str().unwrap_or("")
        ));
    }
    // A binding may be split into several records or gain Composer metadata. Check each
    // connection independently, using the same endpoint policy as id alignment.
    for item in current["DataBindings"].as_array().into_iter().flatten() {
        for map in item["PropertyMaps"].as_array().into_iter().flatten() {
            let mut connection = item.clone();
            connection["PropertyMaps"] = serde_json::json!([map]);
            let carried = generated["DataBindings"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|made| {
                    made["PropertyMaps"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|made_map| {
                            let mut made = made.clone();
                            made["PropertyMaps"] = serde_json::json!([made_map]);
                            endpoints(&made) == endpoints(&connection)
                        })
                });
            if !carried {
                out.push(format!(
                    "binding not written: {}.{} -> {}.{}",
                    item["SourceId"].as_str().unwrap_or(""),
                    map["SourceProperty"].as_str().unwrap_or(""),
                    item["TargetId"].as_str().unwrap_or(""),
                    map["TargetProperty"].as_str().unwrap_or("")
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn widget(id: &str, kind: &str, children: Vec<Value>) -> Value {
        json!({"Properties": {"Id": id, "Type": kind}, "Widgets": children})
    }

    #[test]
    fn lost_wiring_checks_each_connection_without_composer_metadata() {
        let current = json!({"DataBindings": [{"Id":"old", "SourceArea":"Data", "SourceId":"debug", "SourceSection":"Services", "TargetArea":"UI", "TargetId":"grid-dashboards", "TargetSection":"", "PropertyMaps":[
            {"SourceProperty":"GetDashboardsRows", "TargetProperty":"Data", "SourcePropertyType":"infotable"},
            {"SourceProperty":"OtherRows", "TargetProperty":"SelectedRows"}
        ]}], "Events":[{"Id":"e1", "EventTriggerId":"button", "EventTriggerEvent":"Clicked", "EventHandlerId":"debug", "EventHandlerService":"GetDashboardsRows", "ComposerMetadata":true}]});
        let mut generated = current.clone();
        generated["DataBindings"][0]["Id"] = json!("new");
        generated["DataBindings"][0]["SourceDetails"] = json!("");
        generated["DataBindings"][0]["PropertyMaps"][0]["SourcePropertyType"] = json!("INFOTABLE");
        generated["DataBindings"][0]["PropertyMaps"]
            .as_array_mut()
            .unwrap()
            .pop();
        generated["Events"][0]
            .as_object_mut()
            .unwrap()
            .remove("ComposerMetadata");
        assert_eq!(
            lost_wiring(&current, &generated),
            vec!["binding not written: debug.OtherRows -> grid-dashboards.SelectedRows"]
        );
        generated["DataBindings"] = json!([]);
        assert_eq!(lost_wiring(&current, &generated).len(), 2);
        generated["Events"] = json!([]);
        assert_eq!(lost_wiring(&current, &generated).len(), 3);
    }

    #[test]
    fn a_missing_mashup_type_is_the_plain_mashup_a_spec_generates() {
        let kinds = BTreeMap::new();
        let missing = merge::Difference {
            path: "/mashupType".into(),
            current: None,
            generated: json!("mashup"),
        };
        assert!(is_unstated_default(&missing, &kinds));
        let other = merge::Difference {
            path: "/mashupType".into(),
            current: Some(json!("targetmashup")),
            generated: json!("mashup"),
        };
        assert!(!is_unstated_default(&other, &kinds));
    }

    #[test]
    fn widgets_line_up_by_type_even_with_an_extra_widget() {
        let current = json!({"UI": widget("mashup-root", "mashup", vec![
            widget("flexcontainer-2", "flexcontainer", vec![
                widget("ptcslabel-3", "ptcslabel", vec![]),
                widget("ptcsbutton-4", "ptcsbutton", vec![]),
            ]),
        ])});
        let generated = json!({"UI": widget("mashup-root", "mashup", vec![
            widget("flexcontainer-2", "flexcontainer", vec![
                widget("ptcslabel-1001", "ptcslabel", vec![]),
                widget("ptcslabel-1002", "ptcslabel", vec![]),
                widget("ptcsbutton-1003", "ptcsbutton", vec![]),
            ]),
        ])});
        let allocated = vec![
            (
                "a".to_string(),
                "widget".to_string(),
                "ptcslabel-1001".to_string(),
            ),
            (
                "b".to_string(),
                "widget".to_string(),
                "ptcslabel-1002".to_string(),
            ),
            (
                "c".to_string(),
                "widget".to_string(),
                "ptcsbutton-1003".to_string(),
            ),
        ];
        let pins = widgets(&current, &generated, &allocated);
        assert_eq!(
            pins.widgets,
            vec![
                (
                    "a".to_string(),
                    "widget".to_string(),
                    "ptcslabel-3".to_string()
                ),
                (
                    "c".to_string(),
                    "widget".to_string(),
                    "ptcsbutton-4".to_string()
                ),
            ]
        );
    }

    #[test]
    fn wiring_pairs_by_content_apart_from_the_id() {
        let current =
            json!({"DataBindings": [{"Id": "k1", "x": 1}, {"Id": "k2", "x": 2}], "Events": []});
        let generated = json!({"DataBindings": [{"Id": "g2", "x": 2}, {"Id": "g1", "x": 1}, {"Id": "g3", "x": 3}], "Events": []});
        let pins = wiring(&current, &generated);
        assert_eq!(pins.get("g1").map(String::as_str), Some("k1"));
        assert_eq!(pins.get("g2").map(String::as_str), Some("k2"));
        assert!(!pins.contains_key("g3"));
    }

    #[test]
    fn a_missing_styler_label_or_version_is_no_difference() {
        let kinds = BTreeMap::new();
        let missing = |path: &str, generated: Value| merge::Difference {
            path: path.into(),
            current: None,
            generated,
        };
        assert!(is_unstated_default(
            &missing(
                "/UI/Properties/StyleProperties/data/flexcontainer-4/widget",
                json!("flex-container")
            ),
            &kinds
        ));
        assert!(is_unstated_default(
            &missing(
                "/UI/Properties/StyleProperties/meta",
                json!({"type": "WidgetStyler", "version": "1.0"})
            ),
            &kinds
        ));
        assert!(!is_unstated_default(
            &missing(
                "/UI/Properties/StyleProperties/data/flexcontainer-4/parts",
                json!({"$": {"$": {"background": "red"}}})
            ),
            &kinds
        ));
    }

    #[test]
    fn a_recorded_numeric_base_type_is_no_difference() {
        let maps = |base: &str| {
            json!([{"SourceProperty": "Output", "SourcePropertyBaseType": "NUMBER",
                "TargetProperty": "PopupHeight", "TargetPropertyBaseType": base}])
        };
        let difference = |current: &str, generated: &str| merge::Difference {
            path: "/DataBindings/b1/PropertyMaps".into(),
            current: Some(maps(current)),
            generated: maps(generated),
        };
        let kinds = BTreeMap::new();
        assert!(is_unstated_default(
            &difference("INTEGER", "NUMBER"),
            &kinds
        ));
        assert!(!is_unstated_default(
            &difference("STRING", "NUMBER"),
            &kinds
        ));
    }

    #[test]
    fn wiring_claims_a_binding_with_the_same_endpoints_but_other_details() {
        let binding = |id: &str, base: &str| {
            json!({"Id": id, "SourceArea": "Data", "SourceId": "GetRows", "SourceSection": "S",
                "TargetArea": "UI", "TargetId": "grid-1", "TargetSection": "",
                "PropertyMaps": [{"SourceProperty": "result", "TargetProperty": "Data",
                    "SourcePropertyBaseType": base}]})
        };
        let current = json!({"DataBindings": [binding("kept", "INFOTABLE")], "Events": []});
        let generated = json!({"DataBindings": [binding("made", "JSON")], "Events": []});
        let pins = wiring(&current, &generated);
        assert_eq!(pins.get("made").map(String::as_str), Some("kept"));
    }
}
