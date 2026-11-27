use super::{align, baseline::Baseline, compile, derive};
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
    pub raw_types: BTreeMap<String, usize>,
    /// Widgets that differ from the last build, when the spec is the page's own page.yaml.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hand_edited: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// The stylesheet the spec's `page.css` names, written beside the spec by `export`.
    #[serde(skip)]
    pub css: Option<String>,
}

fn invalid(message: impl ToString) -> AuthoringError {
    AuthoringError::Invalid(message.to_string())
}

const ELEMENTS: [&str; 47] = [
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
    "link",
    "image",
    "icon",
    "divider",
    "shape",
    "checkbox",
    "radio",
    "toggle",
    "textarea",
    "date_picker",
    "value_display",
    "embed",
    "events_router",
    "page_host",
    "menu_bar",
];

/// Count the nodes of a spec file: element nodes and `widget` nodes, wherever they nest.
fn nodes(spec: &Value, elements: &mut usize, raw: &mut usize, types: &mut BTreeMap<String, usize>) {
    match spec {
        Value::Object(map) => {
            if (map.get("expression").is_some_and(Value::is_string) && map.contains_key("output"))
                || map.get("navigate").is_some_and(Value::is_object)
            {
                *elements += 1;
            }
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
        css: None,
    }
}

/// The stylesheet an existing page.yaml names with `page.css`, read from beside it.
fn with_css(mut found: Extraction, directory: &std::path::Path) -> Extraction {
    found.css = serde_saphyr::from_str::<Value>(&found.spec)
        .ok()
        .and_then(|v| v["page"]["css"].as_str().map(str::to_string))
        .and_then(|relative| std::fs::read_to_string(directory.join(relative)).ok());
    found
}

/// Contained mashups and navigations store a copy of their target's parameter definitions. A
/// source whose copies are stale gets the target's current ones from a build; say so, so the
/// difference is explained.
fn refreshed_definitions(source: &Value, built: &Value) -> Vec<String> {
    fn collect<'a>(widget: &'a Value, out: &mut BTreeMap<&'a str, (&'a str, Vec<&'a str>)>) {
        let properties = &widget["Properties"];
        if let (Some(id), Some(entries)) = (
            properties["Id"].as_str(),
            properties["MashupParameters"].as_array(),
        ) {
            let target = properties["Name"]
                .as_str()
                .or_else(|| properties["Mashup"].as_str())
                .unwrap_or("its mashup");
            let mut names: Vec<_> = entries
                .iter()
                .filter_map(|e| e["ParameterName"].as_str())
                .collect();
            names.sort_unstable();
            out.insert(id, (target, names));
        }
        for child in widget["Widgets"].as_array().into_iter().flatten() {
            collect(child, out);
        }
    }
    let (mut had, mut wrote) = (BTreeMap::new(), BTreeMap::new());
    collect(&source["UI"], &mut had);
    collect(&built["UI"], &mut wrote);
    let mut notes = vec![];
    for (id, (target, now)) in wrote {
        let Some((_, before)) = had.get(id) else {
            continue;
        };
        let added: Vec<_> = now.iter().filter(|n| !before.contains(n)).collect();
        let removed: Vec<_> = before.iter().filter(|n| !now.contains(n)).collect();
        if added.is_empty() && removed.is_empty() {
            continue;
        }
        let list = |names: Vec<&&str>| names.iter().map(|n| **n).collect::<Vec<_>>().join(", ");
        let mut note = format!("refreshed the stored parameter definitions of {id} from {target}");
        if !added.is_empty() {
            note.push_str(&format!(": added {}", list(added)));
        }
        if !removed.is_empty() {
            note.push_str(&format!(
                "{} removed {}",
                if note.ends_with(target) { ":" } else { ";" },
                list(removed)
            ));
        }
        notes.push(note);
    }
    notes
}

fn count_widgets(content: &Value) -> usize {
    fn walk(widget: &Value) -> usize {
        widget["Widgets"]
            .as_array()
            .map_or(0, |kids| kids.len() + kids.iter().map(walk).sum::<usize>())
    }
    walk(&content["UI"])
}

/// The widgets that differ between what was built and what is there now, named by their node in
/// the spec where the baseline knows it.
fn hand_edits(current: &Value, baseline: &Baseline) -> Vec<String> {
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
    for path in baseline.content_edits(current) {
        if let Some(id) = path
            .strip_prefix("content/widgets/")
            .and_then(|p| p.split('/').next())
        {
            changed.insert(named(id));
        } else {
            changed.insert(path);
        }
    }
    changed.into_iter().collect()
}

/// `<columns>x<rows>` when the entity is a dashboard gadget.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn gadget_size(entity: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(entity);
    let start = text.find("<Mashup ")?;
    let tag = &text[start..start + text[start..].find('>')?];
    let attribute = |name: &str| {
        let (_, rest) = tag.split_once(&format!(" {name}=\""))?;
        Some(rest.split_once('"')?.0.to_string())
    };
    if attribute("aspect.mashupType")? != "gadget" {
        return None;
    }
    let count = |name: &str| {
        attribute(name)?
            .parse::<f64>()
            .ok()
            .map(|n| n.max(0.0) as u32)
    };
    Some(format!("{}x{}", count("columns")?, count("rows")?))
}

/// A gadget's widgets sit in its root, so a spec that claims a template container for the root
/// cannot say it: that page stays an ordinary mashup.
fn mark_gadget(derived: &mut derive::Derived, size: &str) {
    let Some(page) = derived.value["page"].as_object_mut() else {
        return;
    };
    if page.contains_key("root") || page.contains_key("kind") {
        return;
    }
    let entries: Vec<_> = std::mem::take(page).into_iter().collect();
    for (key, value) in entries {
        page.insert(key.clone(), value);
        if key == "name" {
            page.insert("kind".into(), serde_json::json!("gadget"));
            page.insert("gadget_size".into(), serde_json::json!(size));
        }
    }
    derived.spec = derive::render(&derived.value);
}

/// A call to a service its entity no longer declares is carried as found, with the parameter
/// types the mashup stores, and the spec says so.
fn keep_unknown_services(
    solution: &Solution,
    content: &Value,
    derived: &mut derive::Derived,
) -> Vec<String> {
    let (model, _) = crate::core::types::load_model(solution);
    let platform = crate::core::types::cached_platform(solution);
    let Some(data) = derived.value["page"]["data"].as_object().cloned() else {
        return vec![];
    };
    let mut notes = vec![];
    let mut changed = false;
    for (alias, entry) in data {
        let Some(service) = entry["service"].as_str() else {
            continue;
        };
        if let Ok((entity, name)) =
            authoring::session::source_target(&model, platform.as_ref(), service)
        {
            // A binding that stores another type than the service declares now keeps it.
            let declared = model
                .entities
                .iter()
                .find(|e| e.name == entity)
                .and_then(|e| {
                    crate::core::catalog::lookup_service(
                        e,
                        &model.entities,
                        platform.as_ref(),
                        &name,
                    )
                    .service
                });
            let Some(declared) = declared else { continue };
            let mut differing = serde_json::Map::new();
            for binding in content["DataBindings"].as_array().into_iter().flatten() {
                if binding["TargetArea"] != "Data"
                    || binding["TargetId"] != name.as_str()
                    || !binding["TargetSection"]
                        .as_str()
                        .is_some_and(|s| s.ends_with(&format!("_{entity}")))
                {
                    continue;
                }
                for map in binding["PropertyMaps"].as_array().into_iter().flatten() {
                    let (Some(parameter), Some(stored)) = (
                        map["TargetProperty"].as_str(),
                        map["TargetPropertyBaseType"].as_str(),
                    ) else {
                        continue;
                    };
                    if map["TargetPropertyType"] == "Parameter"
                        && declared
                            .parameters
                            .iter()
                            .find(|p| p.name == parameter)
                            .is_some_and(|p| p.value.base_type != stored)
                    {
                        differing.insert(parameter.into(), stored.into());
                    }
                }
            }
            if !differing.is_empty() {
                if let Some(target) = derived.value["page"]["data"][&alias].as_object_mut() {
                    target.insert("param_types".into(), Value::Object(differing));
                    changed = true;
                }
            }
            continue;
        }
        let Some((entity, name)) = service.rsplit_once('.') else {
            continue;
        };
        if !model.entities.iter().any(|e| e.name == entity) {
            continue;
        }
        let mut parameters = serde_json::Map::new();
        for binding in content["DataBindings"].as_array().into_iter().flatten() {
            if binding["TargetArea"] != "Data"
                || binding["TargetId"] != name
                || !binding["TargetSection"]
                    .as_str()
                    .is_some_and(|s| s.ends_with(&format!("_{entity}")))
            {
                continue;
            }
            for map in binding["PropertyMaps"].as_array().into_iter().flatten() {
                if map["TargetPropertyType"] == "Parameter" {
                    if let Some(parameter) = map["TargetProperty"].as_str() {
                        parameters.insert(
                            parameter.into(),
                            map["TargetPropertyBaseType"]
                                .as_str()
                                .unwrap_or("STRING")
                                .into(),
                        );
                    }
                }
            }
        }
        if let Some(target) = derived.value["page"]["data"][&alias].as_object_mut() {
            target.insert("unknown_service".into(), Value::Object(parameters));
        }
        notes.push(format!(
            "{service} is not a service {entity} declares; the call is kept as found"
        ));
    }
    if !notes.is_empty() || changed {
        derived.spec = derive::render(&derived.value);
    }
    notes
}

/// The spec node with this id.
fn node_mut<'a>(value: &'a mut Value, id: &str) -> Option<&'a mut serde_json::Map<String, Value>> {
    match value {
        Value::Object(map) => {
            if map.len() == 1
                && map
                    .values()
                    .next()
                    .is_some_and(|body| body.get("id").and_then(Value::as_str) == Some(id))
            {
                return map.values_mut().next().and_then(Value::as_object_mut);
            }
            map.values_mut().find_map(|v| node_mut(v, id))
        }
        Value::Array(items) => items.iter_mut().find_map(|v| node_mut(v, id)),
        _ => None,
    }
}

fn widget_props_at<'a>(widget: &'a Value, id: &str) -> Option<&'a Value> {
    if widget["Properties"]["Id"] == id {
        return Some(&widget["Properties"]);
    }
    widget["Widgets"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|child| widget_props_at(child, id))
}

/// The column recipe also fills hidden fields from the current result shape and sets
/// grid styles. Use the actual page compiler to check both effects before keeping it.
fn friendly_grid_columns(
    solution: &Solution,
    project: &str,
    directory: &std::path::Path,
    stored: &Value,
    derived: &mut derive::Derived,
    tokens: Option<&str>,
) {
    fn candidates(value: &Value, grids: &mut Vec<String>) {
        if let Some(id) = value["grid"]["id"].as_str() {
            if value["grid"]["column_config"]["formatInfo"].is_array()
                && value["grid"]["props"]["Data"]["bind"].is_string()
            {
                grids.push(id.into());
            }
        }
        if let Some(map) = value.as_object() {
            for child in map.values() {
                candidates(child, grids);
            }
        } else if let Some(items) = value.as_array() {
            for item in items {
                candidates(item, grids);
            }
        }
    }

    let mut grids = vec![];
    candidates(&derived.value, &mut grids);
    let compile = |spec: &Value| {
        let spec = with_tokens(derive::render(spec), tokens);
        let built = compile::compile_at(solution, project, &spec, directory).ok()?;
        let content: Value = serde_json::from_str(&built.content).ok()?;
        Some((built, content))
    };
    let mut baseline = None;
    for id in grids {
        let mut trial = derived.value.clone();
        let Some(body) = node_mut(&mut trial, &id) else {
            continue;
        };
        if body["props"].get("ColumnFormat").is_some() {
            continue;
        }
        let Some(feed) = body["props"]["Data"]["bind"].as_str().map(str::to_owned) else {
            continue;
        };
        let Some(columns) = column_candidates(&body["column_config"]) else {
            continue;
        };
        let stored_id = body
            .get("widget_id")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_owned();
        body.remove("column_config");
        let props = body
            .get_mut("props")
            .and_then(Value::as_object_mut)
            .expect("Data binding");
        props.remove("Data");
        if props.is_empty() {
            body.remove("props");
        }
        body.insert("data".into(), Value::String(feed.clone()));
        body.insert("columns".into(), Value::Array(columns));
        if !trial["page"]["data"][&feed]["service"].is_string() {
            continue;
        }
        let Some((built, candidate)) = compile(&trial) else {
            continue;
        };
        let Some(widget_id) = built.ids.get(&id) else {
            continue;
        };
        let Some(stored_props) = widget_props_at(&stored["UI"], &stored_id) else {
            continue;
        };
        if widget_props_at(&candidate["UI"], widget_id) != Some(stored_props) {
            continue;
        }
        let Some((_, native)) = baseline.get_or_insert_with(|| compile(&derived.value)) else {
            break;
        };
        use crate::core::mashup::diff::{compare, Tier};
        if [Tier::Executable, Tier::Layout]
            .into_iter()
            .all(|tier| compare("native", "recipe", native, &candidate, None, None, tier).equal)
        {
            derived.value = trial;
            derived.spec = derive::render(&derived.value);
        }
    }
}

fn with_tokens(spec: String, tokens: Option<&str>) -> String {
    match tokens {
        Some(tokens) => spec.replacen(
            "rules: v1\n",
            &format!(
                "rules: v1\ntokens: {}\n",
                serde_json::to_string(tokens).expect("token path")
            ),
            1,
        ),
        None => spec,
    }
}

fn column_candidates(config: &Value) -> Option<Vec<Value>> {
    let mut columns = Vec::new();
    for column in config["formatInfo"].as_array()? {
        if column["hidden"] == true && column["inLayout"] == false {
            continue;
        }
        let field = column["FieldName"].as_str()?;
        let mut entry = serde_json::json!({"field": field});
        if let Some(title) = column["Title"].as_str().filter(|title| *title != field) {
            entry["title"] = serde_json::json!(title);
        }
        if let Some(width) = column["Width"].as_u64() {
            entry["width"] = serde_json::json!(width);
        }
        if column["editable"] == true {
            entry["editable"] = Value::Bool(true);
        }
        if let Some(editor) = column["cellEditor"]
            .as_str()
            .filter(|e| matches!(*e, "textfield" | "textarea"))
        {
            entry["editor"] = serde_json::json!(editor);
        }
        if column["preserveWhiteSpaces"] == true {
            entry["preserve_whitespace"] = Value::Bool(true);
        }
        if column["FormatOptions"]["renderer"] == "DATETIME" {
            if let Some(format) = column["FormatOptions"]["FormatString"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                entry["format"] = serde_json::json!(format);
            }
        }
        columns.push(entry);
    }
    (!columns.is_empty()).then_some(columns)
}

/// Build the derived spec offline and state, as props, what the mashup has that the build does not
/// reproduce, so adopting the spec changes nothing. Returns the content the spec last built.
fn refine(
    solution: &Solution,
    project: &str,
    directory: &std::path::Path,
    current: &Value,
    derived: &mut derive::Derived,
) -> Option<Value> {
    let root = derive::root_id();
    let mut last = None;
    // Which widgets and wiring keep their ids is settled by the first build; properties added
    // after that change neither.
    let mut pins = None;
    for _ in 0..4 {
        let text = derive::render(&derived.value);
        let made = match &pins {
            Some(pins) => {
                compile::compile_pinned(solution, project, &text, directory, pins).ok()?
            }
            None => {
                let first = compile::compile_at(solution, project, &text, directory).ok()?;
                let made = align::adopt(solution, project, &text, directory, current, &first)
                    .unwrap_or(first);
                pins = Some(made.pins.clone());
                made
            }
        };
        let content: Value = serde_json::from_str(&made.content).ok()?;
        let widgets: BTreeMap<&str, &str> = made
            .ids
            .iter()
            .map(|(node, widget)| (widget.as_str(), node.as_str()))
            .collect();
        let mut changed = false;
        for difference in align::remaining(current, &content) {
            // A property the mashup lacks is stated as null, which removes it.
            let value = difference.current.unwrap_or(Value::Null);
            let parts: Vec<&str> = difference.path.split('/').collect();
            let Some(at) = parts
                .iter()
                .rposition(|part| *part == "Properties")
                .filter(|at| *at >= 1 && parts.len() == at + 2)
            else {
                continue;
            };
            let (widget, key) = (parts[at - 1], parts[at + 1].to_string());
            let type_id =
                widget_props_at(&current["UI"], widget).and_then(|props| props["Type"].as_str());
            if type_id
                .and_then(|t| crate::core::widgets::catalog().by_type(t))
                .is_some_and(|w| !authoring::property_editable(w, &key))
            {
                continue;
            }
            let node = widgets.get(widget).copied();
            let is_root =
                widget == root || derived.value["page"]["root"]["id"].as_str() == Some(widget);
            let props = if widget == "UI" {
                derived.value["page"]
                    .as_object_mut()
                    .map(|page| page.entry("props").or_insert_with(|| serde_json::json!({})))
            } else if is_root {
                derived.value["page"].as_object_mut().and_then(|page| {
                    page.entry("root")
                        .or_insert_with(|| serde_json::json!({}))
                        .as_object_mut()
                        .map(|root| root.entry("props").or_insert_with(|| serde_json::json!({})))
                })
            } else if let Some(alias) = node.and_then(|n| n.strip_prefix("$data:")) {
                // An expression's widget: its props are the data entry's.
                derived.value["page"]["data"][alias]
                    .as_object_mut()
                    .map(|entry| {
                        entry
                            .entry("props")
                            .or_insert_with(|| serde_json::json!({}))
                    })
            } else if let Some((tabs, label)) = node.and_then(|n| n.split_once("--tab-")) {
                node_mut(&mut derived.value, tabs)
                    .and_then(|body| body["tabs"].as_array_mut())
                    .and_then(|tabs| tabs.iter_mut().find(|t| t["label"] == label))
                    .map(|tab| tab.as_object_mut())
                    .and_then(|tab| tab)
                    .map(|tab| tab.entry("props").or_insert_with(|| serde_json::json!({})))
            } else {
                node.and_then(|node| node_mut(&mut derived.value, node))
                    .map(|body| body.entry("props").or_insert_with(|| serde_json::json!({})))
            };
            if let Some(map) = props.and_then(Value::as_object_mut) {
                if map.get(&key) != Some(&value) {
                    map.insert(key, value);
                    changed = true;
                }
            }
        }
        last = Some(content);
        if !changed {
            break;
        }
    }
    derived.spec = derive::render(&derived.value);
    last
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
                let exact =
                    baseline.content_edits(&value).is_empty() && baseline.css_matches(&value, &css);
                let mut found = counted(name, spec, widgets, exact);
                if !exact {
                    found.hand_edited = hand_edits(&value, &baseline);
                    found.notes.push(
                        "the mashup was edited by hand after its last build; this is its page.yaml, which does not describe those edits"
                            .into(),
                    );
                }
                return Ok(with_css(found, &directory));
            }
            Some(Err(_)) => {}
            None => {
                if let Ok(built) = compile::compile_at(solution, &project, &spec, &directory) {
                    if built.content == content && built.css == css {
                        return Ok(with_css(counted(name, spec, widgets, true), &directory));
                    }
                }
            }
        }
    }
    let tokens = std::fs::read_to_string(directory.join("page.yaml"))
        .ok()
        .and_then(|text| serde_saphyr::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("tokens")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let adopting = tokens.is_none() && value["twacoPageRules"] != "v1";
    let mut derived = if tokens.is_some() {
        derive::derive_with_styles(name, &content, true)
    } else {
        derive::derive_exact(name, &content)
    }
    .map_err(invalid)?;
    if let Some(size) = gadget_size(&loaded.entity_before) {
        mark_gadget(&mut derived, &size);
    }
    let kept = keep_unknown_services(solution, &value, &mut derived);
    let refined = if adopting {
        refine(solution, &project, &directory, &value, &mut derived)
    } else {
        None
    };
    friendly_grid_columns(
        solution,
        &project,
        &directory,
        &value,
        &mut derived,
        tokens.as_deref(),
    );
    let mut notes = derived.notes;
    notes.extend(kept);
    if let Some(content) = refined {
        // The compiled page is the truth about what the spec writes: say what it does not.
        notes.retain(|n| {
            !n.contains("bindings or events could not be written")
                && !n.starts_with("unsupported wiring left out:")
        });
        notes.extend(align::lost_wiring(&value, &content));
        notes.extend(refreshed_definitions(&value, &content));
        let left = align::remaining(&value, &content).len();
        if left > 0 {
            notes.push(format!(
                "{left} differences remain between the mashup and the page this spec builds; mashup build lists each"
            ));
        }
    }
    let carried = (!css.trim().is_empty()).then(|| {
        // Right after the template (and its layout), where a reader looks for the page's look.
        if let Some(page) = derived.value["page"].as_object_mut() {
            let entries: Vec<_> = std::mem::take(page).into_iter().collect();
            let after = if entries.iter().any(|(k, _)| k == "layout") {
                "layout"
            } else {
                "template"
            };
            for (key, value) in entries {
                page.insert(key.clone(), value);
                if key == after {
                    page.insert("css".into(), serde_json::json!("page.css"));
                }
            }
        }
        notes.push(format!(
            "custom CSS sidecar {} ({} bytes) is carried as page.css, written beside the spec by --out",
            solution.relative(&directory.join("mashup/custom.css")),
            css.len()
        ));
        css.clone()
    });
    if carried.is_some() {
        derived.spec = derive::render(&derived.value);
    }
    let spec = with_tokens(derived.spec, tokens.as_deref());
    Ok(Extraction {
        mashup: name.into(),
        elements: derived.elements,
        raw: derived.raw,
        exact: false,
        spec,
        widgets: derived.widgets,
        raw_types: derived.raw_types,
        hand_edited: vec![],
        notes,
        css: carried,
    })
}

/// A spec handed back as text, with no file written beside it, cannot reference `page.css`:
/// building it would fail on the missing file. Leave the reference out and say how to carry the
/// stylesheet; the report keeps its text in `css` for a caller that writes it.
pub fn without_written_css(mut result: Extraction) -> Extraction {
    if result.css.is_some() {
        result.spec = result.spec.replacen(
            "  css: page.css
",
            "",
            1,
        );
        for note in &mut result.notes {
            if note.contains("is carried as page.css") {
                *note = note.replace(
                    "is carried as page.css, written beside the spec by --out",
                    &format!("is not referenced by this spec; `twaco mashup spec {} --out <file>` writes it beside the spec as page.css (its text is in `css`)", result.mashup),
                );
            }
        }
    }
    result
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
    let Some(path) = out else {
        return Ok(without_written_css(result));
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| invalid(format!("{}: {e}", parent.display())))?;
    }
    {
        if let Some(css) = &result.css {
            let beside = path
                .parent()
                .unwrap_or(std::path::Path::new(""))
                .join("page.css");
            workspace::atomic_replace(&beside, css.as_bytes())
                .map_err(|e| invalid(format!("{}: {e}", beside.display())))?;
        }
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
        css: None,
    })
}

#[cfg(test)]
mod refreshed_tests {
    use super::*;
    use serde_json::json;

    fn page(entries: Value) -> Value {
        json!({"UI": {"Properties": {"Id": "root"}, "Widgets": [
            {"Properties": {"Id": "box", "Name": "Acme.Child_MU", "MashupParameters": entries}}
        ]}})
    }

    #[test]
    fn stale_stored_definitions_are_named_with_what_a_build_adds() {
        let stale = page(json!([{"ParameterName": "Old"}]));
        let current =
            page(json!([{"ParameterName": "Old"}, {"ParameterName": "A"}, {"ParameterName": "B"}]));
        assert_eq!(
            refreshed_definitions(&stale, &current),
            vec![
                "refreshed the stored parameter definitions of box from Acme.Child_MU: added A, B"
            ]
        );
        assert!(refreshed_definitions(&current, &current).is_empty());
        let notes = refreshed_definitions(&current, &page(json!([{"ParameterName": "A"}])));
        assert_eq!(
            notes,
            vec!["refreshed the stored parameter definitions of box from Acme.Child_MU: removed B, Old"]
        );
    }
}
