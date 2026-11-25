//! Derive a readable page spec from mashup content, offline.
//!
//! Widgets become elements where the structure is recognised and `widget:` nodes otherwise,
//! carrying properties that differ from the widget catalogue. Token specs also carry literal
//! styles for represented widgets; custom CSS and styles folded into elements remain outside it.

use super::Error;
use crate::core::widgets;
use serde_json::{json, Map, Value};
use serde_saphyr::Location;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug)]
pub(crate) struct Derived {
    pub(crate) spec: String,
    /// The spec before it was written out, for what refines it.
    pub(crate) value: Value,
    pub(crate) elements: usize,
    pub(crate) raw: usize,
    pub(crate) raw_types: BTreeMap<String, usize>,
    pub(crate) widgets: usize,
    pub(crate) notes: Vec<String>,
}

#[cfg(test)]
pub(crate) fn derive(name: &str, content: &str) -> Result<Derived, Error> {
    derive_with_styles(name, content, false)
}

pub(crate) fn derive_with_styles(
    name: &str,
    content: &str,
    carry_styles: bool,
) -> Result<Derived, Error> {
    derive_as(name, content, carry_styles, false)
}

/// A spec to adopt a mashup with: nothing is recognised as a composite element, and the
/// geometry is stated as the mashup has it, so building the spec reproduces the mashup. A mashup
/// that twaco built with the v1 rules (it carries their marker) is derived as before: its
/// composites are what the recipes made.
pub(crate) fn derive_exact(name: &str, content: &str) -> Result<Derived, Error> {
    derive_as(name, content, false, true)
}

fn derive_as(name: &str, content: &str, carry_styles: bool, exact: bool) -> Result<Derived, Error> {
    let content: Value =
        serde_json::from_str(content).map_err(|e| Error::at(Location::UNKNOWN, e))?;
    let exact = exact && content["twacoPageRules"] != "v1";
    let root = content
        .get("UI")
        .ok_or_else(|| Error::at(Location::UNKNOWN, "mashup needs UI"))?;
    if content["mashupType"] == "targetmashup" {
        fn hosts(w: &Value) -> usize {
            usize::from(kind(w) == "pagemashupcontainer")
                + widgets_of(w).iter().map(hosts).sum::<usize>()
        }
        if hosts(root) != 1 {
            return Err(Error::at(Location::UNKNOWN, format!("{name}: master needs exactly one page_host (pagemashupcontainer) where pages render; repair its page container before deriving")));
        }
    }
    let mut run = Run::new(&content);
    run.notes.extend(widgets::configuration_warnings(&content));
    run.carry_styles = carry_styles;
    run.exact = exact;
    let parameters = run.parameters(&root["Properties"]);
    let fresh = fresh_root();
    // The template's own root container stands for the mashup's: its properties become the page's
    // `root` and its children the slot, so an adopted page gains no extra level.
    // As built, a first container stands for it by position whatever its id (a master's root is
    // `flexcontainer-1`, not the template's); the first build pins the id.
    let template_root = widgets_of(root)
        .iter()
        .find(|w| {
            !widgets::catalog()
                .by_type(kind(w))
                .is_some_and(|w| w.function)
        })
        .filter(|w| {
            is_container(w)
                && (exact || prop(w, "Id").as_str() == fresh.get("Id").and_then(Value::as_str))
        });
    // The id of a claimed root that is not the template's, so a build renames the template's.
    let root_name = template_root
        .map(|w| prop(w, "Id").clone())
        .filter(|id| id.as_str() != fresh.get("Id").and_then(Value::as_str));
    let (slot, page_root): (Vec<Value>, Option<Map<String, Value>>) = match template_root {
        Some(first) => {
            let direction = if prop(first, "flex-direction") == "column" {
                "column"
            } else {
                "row"
            };
            let page_root = run.root_props(first, &fresh);
            let mut slot: Vec<Value> = widgets_of(first)
                .iter()
                .map(|w| run.node(w, direction))
                .collect();
            slot.extend(
                widgets_of(root)
                    .iter()
                    .filter(|w| !std::ptr::eq(*w, first))
                    .map(|w| run.node(w, "column")),
            );
            (slot, Some(page_root))
        }
        None => (
            widgets_of(root)
                .iter()
                .map(|w| run.node(w, "column"))
                .collect(),
            None,
        ),
    };
    let mut page = Map::new();
    page.insert("name".into(), json!(name));
    if content["mashupType"] == "targetmashup" {
        page.insert("kind".into(), json!("master"));
        let fresh = crate::core::mashup::templates::master_root();
        let props: Map<String, Value> = root["Properties"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, v)| {
                !matches!(
                    k.as_str(),
                    "Id" | "Type"
                        | "id_index"
                        | "_currentParameterDefs"
                        | "StyleProperties"
                        | "MigratedOn"
                ) && !run.parameters.contains(*k)
                    && fresh.get(*k) != Some(*v)
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !props.is_empty() {
            page.insert("props".into(), json!(props));
        }
    }
    page.insert("template".into(), json!("blank"));
    if exact && content["twacoPageRules"] != "v1" {
        page.insert("layout".into(), json!("as-built"));
    }
    let props = &root["Properties"];
    if let Some(theme) = props["StyleTheme"]
        .as_str()
        .filter(|t| !t.is_empty() && *t != "PTC Convergence Theme")
    {
        page.insert("theme".into(), json!(theme));
    }
    if exact && content["mashupType"] != "targetmashup" {
        // A mashup built by hand keeps the root style it has.
        let fresh = crate::core::mashup::authoring::session::Session::new(
            "Acme.Probe_MU",
            "Acme",
            "blank",
            "PTC Convergence Theme",
        )
        .ok()
        .and_then(|s| s.document.root_properties().get("Style").cloned());
        if let Some(style) = props["Style"].as_str().filter(|style| {
            !style.is_empty() && fresh.as_ref().and_then(Value::as_str) != Some(style)
        }) {
            page.insert("props".into(), json!({"Style": style}));
        }
    }
    if let Some(master) = props["Master"].as_str().filter(|m| !m.is_empty()) {
        page.insert("master".into(), json!(master));
    }
    if props["UseMasterTheme"] == json!(true) {
        page.insert("use_master_theme".into(), json!(true));
    }
    // Root properties have no widget node to carry them. Parameter values belong to their
    // declarations, and stored style parts use the same syntax as a widget's styles.
    {
        let carried = page
            .entry("props")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("props");
        for (key, value) in props.as_object().into_iter().flatten() {
            if !matches!(
                key.as_str(),
                "Id" | "Type"
                    | "id_index"
                    | "_currentParameterDefs"
                    | "StyleProperties"
                    | "MigratedOn"
                    | "StyleTheme"
                    | "Master"
                    | "UseMasterTheme"
            ) && !run.parameters.contains(key)
            {
                carried.insert(key.clone(), value.clone());
            }
        }
        if let Some(parts) = props["StyleProperties"]["data"]
            [props["Id"].as_str().unwrap_or("mashup-root")]
        .get("parts")
        {
            page.insert("styles".into(), parts.clone());
            run.styled
                .insert(props["Id"].as_str().unwrap_or("mashup-root").into());
        }
    }
    if root_name.is_some() || page_root.as_ref().is_some_and(|p| !p.is_empty()) {
        let mut spec_root = Map::new();
        if let Some(id) = root_name {
            spec_root.insert("id".into(), id);
        }
        if let Some(page_root) = page_root.filter(|p| !p.is_empty()) {
            spec_root.insert("props".into(), json!(page_root));
        }
        page.insert("root".into(), Value::Object(spec_root));
    }
    if !parameters.is_empty() {
        page.insert("parameters".into(), Value::Object(parameters));
    }
    let (data, events) = run.wiring();
    let bindings = run.remaining_bindings();
    if !data.is_empty() {
        page.insert("data".into(), Value::Object(data));
    }
    page.insert("slots".into(), json!({"center": slot}));
    if !events.is_empty() {
        page.insert("events".into(), Value::Array(events));
    }
    if !bindings.is_empty() {
        page.insert("bindings".into(), Value::Array(bindings));
    }
    let mut spec = json!({"version": 1, "rules": "v1", "page": page});
    run.resolve(&mut spec);
    {
        let mut kinds = BTreeMap::new();
        fn collect_kinds(widget: &Value, kinds: &mut BTreeMap<String, String>) {
            if let Some(id) = prop(widget, "Id").as_str() {
                kinds.insert(id.into(), kind(widget).into());
            }
            for child in widgets_of(widget) {
                collect_kinds(child, kinds);
            }
        }
        collect_kinds(root, &mut kinds);
        for (id, style) in content["UI"]["Properties"]["StyleProperties"]["data"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if run.styled.contains(id) {
                continue;
            }
            let parts: Vec<_> = style["parts"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(part, _)| {
                    if part == "$" {
                        "the widget itself"
                    } else {
                        part.as_str()
                    }
                })
                .collect();
            let extra = style
                .as_object()
                .is_some_and(|e| e.keys().any(|k| k != "parts" && k != "widget"));
            if parts.is_empty() && !extra {
                continue;
            }
            let Some(what) = kinds.get(id) else {
                run.notes.push(format!(
                    "stale style entry for removed widget {id} (harmless; dropped)"
                ));
                continue;
            };
            run.notes.push(format!(
                "style properties not carried: {what} {id}, parts {}",
                parts.join(", ")
            ));
        }
    }
    let text = render(&spec);
    Ok(Derived {
        spec: text,
        value: spec,
        elements: run.elements,
        raw: run.raw,
        raw_types: run.raw_types,
        widgets: run.widgets,
        notes: run.notes,
    })
}

/// The properties of the container a fresh page starts with, which a page spec's `root` overrides.
fn fresh_root() -> Map<String, Value> {
    let session = crate::core::mashup::authoring::session::Session::new(
        "Acme.Probe_MU",
        "Acme",
        "blank",
        "PTC Convergence Theme",
    )
    .expect("blank template");
    let path = session.document.find("center").expect("center slot").path;
    session.document.at(&path)["Properties"]
        .as_object()
        .cloned()
        .unwrap_or_default()
}

/// The id of the root container every page starts with.
pub(super) fn root_id() -> String {
    fresh_root()["Id"].as_str().unwrap_or_default().to_string()
}

/// A spec as the YAML a person reads.
pub(super) fn render(spec: &Value) -> String {
    let mut text = String::new();
    block(spec, 0, &mut text);
    text
}

fn widgets_of(widget: &Value) -> &[Value] {
    widget["Widgets"].as_array().map_or(&[], Vec::as_slice)
}

fn kind(widget: &Value) -> &str {
    widget["Properties"]["Type"].as_str().unwrap_or("")
}

fn prop<'a>(widget: &'a Value, key: &str) -> &'a Value {
    &widget["Properties"][key]
}

fn text(widget: &Value, key: &str) -> Option<String> {
    prop(widget, key)
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

/// A whole, non-negative number, as the platform writes counts and sizes (a number or a string).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is checked to be a whole number below a million"
)]
fn whole(value: &Value) -> Option<usize> {
    number(value)
        .filter(|n| n.fract() == 0.0 && (0.0..1_000_000.0).contains(n))
        .map(|n| n as usize)
}

fn pixels(value: &Value) -> Option<u64> {
    value
        .as_str()
        .and_then(|s| s.strip_suffix("px"))
        .and_then(|s| s.trim().parse().ok())
}

fn same(a: &Value, b: &Value) -> bool {
    a == b
        || (a.is_string() || a.is_number() || a.is_boolean())
            && (b.is_string() || b.is_number() || b.is_boolean())
            && a.to_string().trim_matches('"') == b.to_string().trim_matches('"')
}

/// Whether a display name is the one placement gives a widget: a word and the id's number.
fn generated_name(widget: &Value, name: &Value) -> bool {
    let number = prop(widget, "Id")
        .as_str()
        .and_then(|id| id.rsplit('-').next())
        .unwrap_or("");
    name.as_str().is_some_and(|name| {
        name.rsplit_once('-').is_some_and(|(stem, n)| {
            n == number && stem.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
        })
    })
}

fn noise(key: &str) -> bool {
    matches!(
        key,
        "Id" | "Type"
            | "DisplayName"
            | "Area"
            | "LastContainer"
            | "ResponsiveLayout"
            | "Left"
            | "Top"
            | "iconClass"
            | "isLocked"
            | "userCannotRemove"
            | "prev-align"
            | "flex-size"
            | "StyleProperties"
            | "Z-index"
    ) || key.starts_with('_')
        || key.ends_with("_StateParameters")
}

/// The `name:TYPE` inputs an expression or validator declares.
fn inputs(widget: &Value) -> Vec<String> {
    if !matches!(
        kind(widget),
        "expression" | "expression2" | "validator" | "validator2"
    ) {
        return vec![];
    }
    prop(widget, "_currentParameterDefs")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(format!(
                "{}:{}",
                d["ParameterName"].as_str()?,
                d["BaseType"].as_str()?
            ))
        })
        .collect()
}

/// Whether a spec can write this property of the widget: one the catalogue knows, or an input the
/// widget declares.
fn known_property(widget: &Value, property: &str, data_bound: bool) -> bool {
    let type_id = kind(widget);
    if widgets::dynamic_property(type_id, &widget["Properties"], property, data_bound).is_some() {
        return true;
    }
    if crate::core::mashup::authoring::function_property(widget, type_id, property).is_some() {
        return true;
    }
    if inputs(widget)
        .iter()
        .any(|i| i.split(':').next() == Some(property))
    {
        return true;
    }
    let catalog = widgets::catalog();
    catalog.by_type(type_id).is_none_or(|widget| {
        widget.defaults.contains_key(property)
            || widget
                .properties
                .as_ref()
                .is_none_or(|p| p.contains_key(property))
            || catalog.base_properties.contains_key(property)
    })
}

/// The `Type` of the widget with this id, searching the tree below `widget`.
fn widget_type<'a>(widget: &'a Value, id: &str) -> Option<&'a str> {
    if prop(widget, "Id").as_str() == Some(id) {
        return widget["Properties"]["Type"].as_str();
    }
    widgets_of(widget).iter().find_map(|w| widget_type(w, id))
}

/// Whether the catalogue types `key` of `type_id` as a number: by its default, or its contract.
fn wants_number(type_id: &str, key: &str, default: Option<&Value>) -> bool {
    match default {
        Some(Value::Number(_)) => true,
        Some(_) => false,
        None => widgets::catalog()
            .by_type(type_id)
            .and_then(|w| w.property(key))
            .map_or_else(
                // Unlisted on this widget: numeric when it is numeric everywhere it is listed,
                // which is what the build checks.
                || super::compile::always_numeric(key),
                |p| matches!(p.base_type.as_deref(), Some("NUMBER" | "INTEGER")),
            ),
    }
}

fn is_container(widget: &Value) -> bool {
    kind(widget) == "flexcontainer"
}

fn is_leaf(widget: &Value) -> bool {
    !is_container(widget) && widgets_of(widget).is_empty()
}

fn is_spacer(widget: &Value) -> bool {
    is_container(widget)
        && widgets_of(widget).is_empty()
        && text(widget, "CustomClass").is_none()
        && prop(widget, "flex-min-height")
            .as_str()
            .is_none_or(str::is_empty)
        && prop(widget, "flex-min-width")
            .as_str()
            .is_none_or(str::is_empty)
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

struct Call {
    section: String,
    name: String,
    alias: String,
    service: String,
    params: Map<String, Value>,
    refresh: u64,
    /// A dynamic source: the shape or template it runs on a Thing of, chosen at runtime.
    dynamic: bool,
}

#[derive(Clone)]
struct Link {
    source: (String, String, String),
    target: (String, String, String, String),
    consumed: bool,
    /// Reads the rows a widget selected rather than the whole result.
    selected: bool,
}

struct Run<'a> {
    carry_styles: bool,
    /// Widgets whose stored styles the spec carries, by widget id.
    styled: BTreeSet<String>,
    /// The mashup was not laid out by the v1 rules, so the spec states its geometry as it is.
    exact: bool,
    content: &'a Value,
    calls: Vec<Call>,
    expressions: Map<String, Value>,
    navigations: HashMap<String, Value>,
    expression_names: HashMap<String, String>,
    links: Vec<Link>,
    ids: HashMap<String, String>,
    used: BTreeSet<String>,
    parameters: BTreeSet<String>,
    elements: usize,
    raw: usize,
    raw_types: BTreeMap<String, usize>,
    widgets: usize,
    notes: Vec<String>,
    dropped: usize,
    unknown: BTreeSet<(String, String)>,
}

impl<'a> Run<'a> {
    fn new(content: &'a Value) -> Self {
        let mut run = Self {
            carry_styles: false,
            styled: BTreeSet::new(),
            exact: false,
            content,
            calls: vec![],
            expressions: Map::new(),
            navigations: HashMap::new(),
            expression_names: HashMap::new(),
            links: vec![],
            ids: HashMap::new(),
            used: BTreeSet::new(),
            parameters: BTreeSet::new(),
            elements: 0,
            raw: 0,
            raw_types: BTreeMap::new(),
            widgets: 0,
            notes: vec![],
            dropped: 0,
            unknown: BTreeSet::new(),
        };
        let mut aliases = BTreeSet::new();
        let mut other = 0;
        for (section, entry) in content["Data"].as_object().into_iter().flatten() {
            let dynamic = matches!(
                entry["EntityType"].as_str(),
                Some("DynamicThingShapes" | "DynamicThingTemplates")
            );
            let named = entry["EntityName"].as_str().unwrap_or("");
            // A dynamic section may name its shape only in its key.
            let entity = if named.is_empty() && dynamic {
                section
                    .split_once('_')
                    .map(|(_, shape)| shape)
                    .unwrap_or("")
            } else {
                named
            };
            if !(entry["EntityType"] == "Things" || dynamic) || entity.is_empty() {
                if !entry["Services"].as_array().is_none_or(Vec::is_empty)
                    && !matches!(
                        entry["EntityType"].as_str(),
                        Some("Session" | "UserExtensions")
                    )
                {
                    other += 1;
                }
                continue;
            }
            for service in entry["Services"].as_array().into_iter().flatten() {
                let (Some(name), Some(target)) =
                    (service["Name"].as_str(), service["Target"].as_str())
                else {
                    continue;
                };
                let mut alias: String = sanitize(name).replace('-', "_");
                if alias.is_empty() {
                    alias = "data".into();
                }
                let base = alias.clone();
                let mut n = 2;
                while !aliases.insert(alias.clone()) {
                    alias = format!("{base}_{n}");
                    n += 1;
                }
                run.used.insert(alias.clone());
                run.calls.push(Call {
                    section: section.clone(),
                    name: name.into(),
                    alias,
                    service: format!("{entity}.{target}"),
                    params: service["Parameters"]
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                    refresh: service["RefreshInterval"].as_u64().unwrap_or(0),
                    dynamic,
                });
            }
        }
        if other > 0 {
            run.notes.push(format!(
                "{other} data entries that are not Thing services were left out"
            ));
        }
        for binding in content["DataBindings"].as_array().into_iter().flatten() {
            let get = |k: &str| binding[k].as_str().unwrap_or("").to_string();
            for map in binding["PropertyMaps"].as_array().into_iter().flatten() {
                let prop = |k: &str| map[k].as_str().unwrap_or("").to_string();
                run.links.push(Link {
                    source: (get("SourceArea"), get("SourceId"), get("SourceSection")),
                    target: (
                        get("TargetArea"),
                        get("TargetId"),
                        get("TargetSection"),
                        prop("TargetProperty"),
                    ),
                    consumed: false,
                    selected: binding["SourceDetails"] == "SelectedRows",
                });
                let last = run.links.last_mut().expect("link");
                let field = prop("SourceProperty");
                last.source.2 = format!("{}\u{1}{field}", last.source.2);
            }
        }
        let mut count = 0;
        fn walk(widget: &Value, run: &mut Run, count: &mut usize) {
            for child in widgets_of(widget) {
                *count += 1;
                let id = child["Properties"]["Id"].as_str().unwrap_or("widget");
                let mut spec = sanitize(id);
                if spec.is_empty() {
                    spec = "widget".into();
                }
                let base = spec.clone();
                let mut n = 2;
                while !run.used.insert(spec.clone()) {
                    spec = format!("{base}_{n}");
                    n += 1;
                }
                if kind(child) == "expression2" {
                    let mut alias = spec.replace('-', "_");
                    let base = alias.clone();
                    let mut n = 2;
                    while !run.used.insert(alias.clone()) {
                        alias = format!("{base}_{n}");
                        n += 1;
                    }
                    run.expression_names.insert(id.into(), alias.clone());
                    spec = alias;
                }
                run.ids.insert(id.into(), spec);
                walk(child, run, count);
            }
        }
        walk(&content["UI"], &mut run, &mut count);
        run.widgets = count;
        run
    }

    fn parameters(&mut self, props: &Value) -> Map<String, Value> {
        let mut out = Map::new();
        for definition in props["_currentParameterDefs"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let (Some(name), Some(base)) = (
                definition["ParameterName"].as_str(),
                definition["BaseType"].as_str(),
            ) {
                let mut text = base.to_string();
                if self.exact {
                    // What the mashup declares beyond the type: its data shape and direction.
                    if let Some(shape) = definition["Aspects"]["dataShape"].as_str() {
                        text.push_str(&format!(":{shape}"));
                    }
                    match definition["Aspects"]["bindingDirection"].as_str() {
                        Some(direction) if direction != "IN" => {
                            text.push_str(&format!(":{direction}"));
                        }
                        _ => {}
                    }
                }
                let description = definition["Description"].as_str().unwrap_or("");
                let default = props.get(name);
                let value = if description.is_empty() && default.is_none() {
                    json!(text)
                } else {
                    let mut details = json!({"type": base, "description": description});
                    if let Some(default) = default {
                        details["default"] = default.clone();
                    }
                    if self.exact {
                        if let Some(direction) = definition["Aspects"]["bindingDirection"].as_str()
                        {
                            details["direction"] = json!(direction);
                        }
                        if let Some(shape) = definition["Aspects"]["dataShape"].as_str() {
                            details["data_shape"] = json!(shape);
                        }
                    }
                    details
                };
                out.insert(name.into(), value);
                self.parameters.insert(name.into());
            }
        }
        out
    }

    /// The text of what a link reads from, with widget ids left as `@@id@@` for `resolve`.
    fn source(&self, link: &Link) -> Option<String> {
        let (area, id, section) = &link.source;
        let (section, field) = section.split_once('\u{1}')?;
        match area.as_str() {
            "Session" | "UserExtensions" => Some(format!("{area}.{field}")),
            "Data" => {
                let call = self
                    .calls
                    .iter()
                    .find(|c| c.section == section && c.name == *id)?;
                Some(match (field.is_empty(), link.selected) {
                    (true, false) => call.alias.clone(),
                    (false, false) => format!("{}.{field}", call.alias),
                    (true, true) => format!("{}.SelectedRows", call.alias),
                    (false, true) => format!("{}.SelectedRows.{field}", call.alias),
                })
            }
            "UI" if self.expression_names.contains_key(id) && !field.is_empty() => {
                Some(if field == "Output" {
                    self.expression_names[id].clone()
                } else {
                    format!("{}.{field}", self.expression_names[id])
                })
            }
            "UI" if !field.is_empty() => Some(format!("@@{id}@@.{field}")),
            "Mashup" if self.parameters.contains(field) => Some(format!("param.{field}")),
            _ => None,
        }
    }

    /// The service a widget's `Data` is fed from; the link is then the element's own.
    fn feed(&mut self, widget: &Value) -> Option<String> {
        let id = prop(widget, "Id").as_str()?;
        let found = self.links.iter().position(|l| {
            !l.consumed
                && l.target.0 == "UI"
                && l.target.1 == id
                && l.target.3 == "Data"
                && l.source.0 == "Data"
                && !l.selected
        })?;
        let alias = self.source(&self.links[found])?;
        self.links[found].consumed = true;
        Some(alias)
    }

    /// A bound property the element keeps as its own, such as a label's text.
    fn take(&mut self, widget: &Value, property: &str) -> Option<Value> {
        let id = prop(widget, "Id").as_str()?;
        let found = self.links.iter().position(|l| {
            !l.consumed && l.target.0 == "UI" && l.target.1 == id && l.target.3 == property
        })?;
        let source = self.source(&self.links[found])?;
        self.links[found].consumed = true;
        Some(json!({"bind": source}))
    }

    fn id(&self, widget: &Value) -> String {
        let id = prop(widget, "Id").as_str().unwrap_or("widget");
        self.ids.get(id).cloned().unwrap_or_else(|| sanitize(id))
    }

    /// Properties that differ from the catalogue, minus the ones the element writes itself, plus
    /// the widget's remaining bound properties.
    fn props(&mut self, widget: &Value, skip: &[&str]) -> Map<String, Value> {
        let type_id = kind(widget);
        let data_bound = widgets::property_bound(
            self.content,
            prop(widget, "Id").as_str().unwrap_or_default(),
            "Data",
        );
        let mut base = Map::new();
        if let Some(known) = widgets::catalog().by_type(type_id) {
            base.extend(known.defaults.clone());
        }
        let dropped = &widgets::catalog().dropped;
        if type_id == "flexcontainer" {
            base.extend(dropped.flexcontainer.clone());
        } else if type_id.starts_with("ptcs") {
            base.extend(dropped.ptcs.clone());
        }
        let mut out = Map::new();
        for (key, value) in widget["Properties"].as_object().into_iter().flatten() {
            if self.exact && type_id == "navigationfunction" && key == "_currentParameterDefs" {
                out.insert(key.clone(), value.clone());
                continue;
            }
            if self.exact && key == "DisplayName" && !generated_name(widget, value) {
                out.insert(key.clone(), value.clone());
                continue;
            }
            // On a v1 page the layout rules write `scrolling`; an as-built page keeps its own.
            // As built, a container stays non-responsive and a positioned widget keeps its
            // place; the catalogue's default would make the first responsive and the second 0.
            let kept = self.exact
                && (key == "flex-size"
                    || matches!(key.as_str(), "ResponsiveLayout" | "Z-index")
                    || matches!(key.as_str(), "Left" | "Top"));
            if (noise(key) && !kept)
                || (!self.exact && key == "scrolling")
                || skip.contains(&key.as_str())
                || skip
                    .iter()
                    .any(|s| s.ends_with('*') && key.starts_with(&s[..s.len() - 1]))
            {
                continue;
            }
            if widgets::catalog()
                .by_type(type_id)
                .is_some_and(|w| !crate::core::mashup::authoring::property_editable(w, key))
            {
                continue;
            }
            if base.get(key).is_some_and(|d| same(d, value)) {
                continue;
            }
            if key == "MashupParameters"
                && !(self.exact && matches!(type_id, "mashupcontainer" | "navigationfunction"))
            {
                // Managed by the contained mashup; `embed` writes it.
                continue;
            }
            if !known_property(widget, key, data_bound) && !self.exact {
                self.unknown.insert((
                    prop(widget, "Id").as_str().unwrap_or("").into(),
                    key.clone(),
                ));
                continue;
            }
            if let Some(text) = value.as_str() {
                if !text.trim().is_empty() && wants_number(type_id, key, base.get(key)) {
                    // The build writes numbers as numbers; text that is none cannot be built.
                    match text.trim().parse::<f64>() {
                        Ok(n) => {
                            let whole = text.trim().parse::<i64>().ok();
                            out.insert(key.clone(), whole.map_or_else(|| json!(n), |w| json!(w)));
                        }
                        Err(_) => self.notes.push(format!(
                            "property left out: {} {key} (\"{text}\" is not a number)",
                            prop(widget, "Id").as_str().unwrap_or("")
                        )),
                    }
                    continue;
                }
            }
            if value.get("StateFormats").is_some() {
                self.notes_once(
                    "state formatting snapshots are not carried; use state: on a detail_pair or status: on a grid column",
                );
                continue;
            }
            if value.is_string() && value.as_str() == Some("") && !base.contains_key(key) {
                continue;
            }
            out.insert(key.clone(), value.clone());
        }
        let id = prop(widget, "Id").as_str().unwrap_or("").to_string();
        let pending: Vec<usize> = (0..self.links.len())
            .filter(|&i| {
                let l = &self.links[i];
                !l.consumed && l.target.0 == "UI" && l.target.1 == id
            })
            .collect();
        for i in pending {
            if !known_property(widget, &self.links[i].target.3, data_bound) {
                continue;
            }
            if self
                .links
                .iter()
                .any(|l| l.consumed && l.target == self.links[i].target)
            {
                continue;
            }
            // A bound property that also has its own starting value (Visible: false driven by a
            // toggle) keeps the value here; the binding goes to page.bindings.
            if out.contains_key(&self.links[i].target.3) {
                continue;
            }
            if let Some(source) = self.source(&self.links[i]) {
                self.links[i].consumed = true;
                let property = self.links[i].target.3.clone();
                out.insert(property, json!({"bind": source}));
            }
        }
        out
    }

    /// What the page's root container has that a fresh page's does not, as the spec's `root.props`.
    fn root_props(&mut self, widget: &Value, fresh: &Map<String, Value>) -> Map<String, Value> {
        let mut out = Map::new();
        let mut seen = vec![];
        for (key, value) in widget["Properties"].as_object().into_iter().flatten() {
            seen.push(key.as_str());
            // An as-built root that is not responsive stays so, like the containers below it.
            let fixed = noise(key)
                && !matches!(key.as_str(), "DisplayName" | "flex-size")
                && !(self.exact && matches!(key.as_str(), "Left" | "Top"))
                && !(self.exact && key == "ResponsiveLayout" && *value == false);
            if fixed || fresh.get(key).is_some_and(|f| same(f, value)) {
                continue;
            }
            if fresh.get(key).is_none()
                && self
                    .default_for(widget, key)
                    .is_some_and(|d| same(&d, value))
            {
                continue;
            }
            out.insert(key.clone(), value.clone());
        }
        out.extend(self.props(widget, &seen));
        out
    }

    /// The catalogue's value of a property for the widget's type, if it has one.
    fn default_for(&self, widget: &Value, key: &str) -> Option<Value> {
        let mut base = Map::new();
        if let Some(known) = widgets::catalog().by_type(kind(widget)) {
            base.extend(known.defaults.clone());
        }
        base.extend(widgets::catalog().dropped.flexcontainer.clone());
        base.get(key).cloned()
    }

    fn count(&mut self, element: &str, type_id: Option<&str>) {
        if element == "widget" {
            self.raw += 1;
            *self
                .raw_types
                .entry(type_id.unwrap_or("unknown").into())
                .or_default() += 1;
        } else {
            self.elements += 1;
        }
    }

    fn make(&mut self, element: &str, id: String, mut body: Map<String, Value>) -> Value {
        self.count(element, None);
        let mut ordered = Map::new();
        ordered.insert("id".into(), json!(id));
        ordered.append(&mut body);
        json!({ element: ordered })
    }

    fn node(&mut self, widget: &Value, parent: &str) -> Value {
        let type_id = kind(widget).to_string();
        let mut node = match type_id.as_str() {
            "flexcontainer" => self.container(widget, parent),
            "ptcsmenubar" => {
                let props = self.props(widget, &[]);
                self.finish("menu_bar", widget, Map::new(), props)
            }
            "pagemashupcontainer" => {
                let props = self.props(widget, &[]);
                self.finish("page_host", widget, Map::new(), props)
            }
            "ptcslabel" => self.label(widget),
            "expression2" => self.expression(widget),
            "navigationfunction" => self.navigation(widget),
            "eventsrouter" => self.router(widget),
            "ptcsbutton" => self.button(widget),
            "ptcslink" | "ptcsimage" | "ptcsicon" | "ptcsdivider" | "ptcscheckbox"
            | "ptcsradio" | "ptcstogglebutton" | "ptcstextarea" | "ptcsvaluedisplay"
            | "ptcspagination" | "ptcskpidial" | "ptcsrte" | "statusmessage" => self.native(widget),
            "mashupcontainer" if text(widget, "Name").is_some_and(|s| !s.is_empty()) => {
                self.embed(widget)
            }
            "ptcstextfield" => {
                let mut body = Map::new();
                if let Some(hint) = text(widget, "HintText") {
                    body.insert("hint".into(), json!(hint));
                }
                if let Some(label) = text(widget, "Label") {
                    body.insert("text".into(), json!(label));
                }
                let props = self.props(widget, &["HintText", "Label"]);
                self.finish("text_field", widget, body, props)
            }
            "ptcsdatepicker" if prop(widget, "DateRange") == &json!(true) => {
                let mut body = Map::new();
                if let Some(hint) = text(widget, "HintText").filter(|h| h != "Autohint") {
                    body.insert("hint".into(), json!(hint));
                }
                let props = self.props(widget, &["DateRange", "HintText"]);
                self.finish("date_range", widget, body, props)
            }
            "ptcsdatepicker" => self.native(widget),
            "ptcsdropdown" | "ptcslist" => self.picker(widget),
            // A grid's column formats are kept exactly as they are when adopting.
            "ptcsgrid" if self.exact => self.other(widget),
            "ptcsgrid" => self.grid(widget),
            "ptcschartline" | "ptcschartbar" | "ptcschartpie" | "ptcschartcombo"
            | "ptcschartpareto" | "ptcschartwaterfall" => self.chart(widget),
            "ptcscollection" => self.collection(widget),
            "ptcstabset" => self.tabs(widget),
            _ => self.other(widget),
        };
        if self.exact {
            let id = prop(widget, "Id").as_str().unwrap_or("");
            let numeric = id
                .rsplit_once('-')
                .is_some_and(|(_, suffix)| suffix.parse::<u64>().is_ok());
            if !numeric || sanitize(id) != id {
                if let Some(body) = node
                    .as_object_mut()
                    .and_then(|m| m.values_mut().next())
                    .and_then(Value::as_object_mut)
                    .filter(|b| b.contains_key("id"))
                {
                    body.insert("widget_id".into(), json!(id));
                }
            }
        }
        if self.carry_styles {
            let id = prop(widget, "Id").as_str().unwrap_or("");
            let parts = &self.content["UI"]["Properties"]["StyleProperties"]["data"][id]["parts"];
            let mut styles = vec![];
            let mut skipped = false;
            for (part, states) in parts.as_object().into_iter().flatten() {
                for (state, properties) in states.as_object().into_iter().flatten() {
                    for (property, value) in properties.as_object().into_iter().flatten() {
                        let Some(value) = value.as_str() else {
                            skipped = true;
                            continue;
                        };
                        let scope = match (part.as_str(), state.as_str()) {
                            ("$", "$") => String::new(),
                            (_, "$") => format!("{part}:"),
                            _ => format!("{part}@{state}:"),
                        };
                        styles.push(format!("{scope}{property}={value}"));
                    }
                }
            }
            if skipped {
                self.notes.push(format!(
                    "style properties partly not carried: widget {id}, non-text values"
                ));
            }
            if !styles.is_empty() {
                if let Some(body) = node
                    .as_object_mut()
                    .and_then(|m| m.values_mut().next())
                    .and_then(Value::as_object_mut)
                {
                    body.insert("style".into(), json!(styles));
                    self.styled.insert(id.to_string());
                }
            }
        } else {
            // What `style:` cannot say stays verbatim, so the look survives a build.
            let id = prop(widget, "Id").as_str().unwrap_or("");
            let entry = &self.content["UI"]["Properties"]["StyleProperties"]["data"][id];
            let parts = &entry["parts"];
            // `widget` is the styler label Composer records; a build writes it back.
            let others = entry
                .as_object()
                .is_some_and(|e| e.keys().any(|k| k != "parts" && k != "widget"));
            if parts.as_object().is_some_and(|p| !p.is_empty()) && !others {
                if let Some(body) = node
                    .as_object_mut()
                    .and_then(|m| m.values_mut().next())
                    .and_then(Value::as_object_mut)
                {
                    body.insert("styles".into(), parts.clone());
                    self.styled.insert(id.to_string());
                }
            }
        }
        node
    }

    fn finish(
        &mut self,
        element: &str,
        widget: &Value,
        mut body: Map<String, Value>,
        props: Map<String, Value>,
    ) -> Value {
        if !props.is_empty() {
            body.insert("props".into(), Value::Object(props));
        }
        let id = self.id(widget);
        self.make(element, id, body)
    }

    /// A widget without an element: its type and the properties that differ from the catalogue.
    fn other(&mut self, widget: &Value) -> Value {
        let type_id = kind(widget).to_string();
        let reason = match type_id.as_str() {
            "shape" => "decorative shapes remain raw",
            "googlemap" | "OpenStreetMap" | "openstreetmap" => "map extension contracts remain raw",
            "ptcscollection" => "collection needs a static cell, nonempty row map and a supported data feed; dynamic entity and mashup selection remain raw",
            "mashupcontainer" => "embed needs a static mashup; dynamic Name bindings remain raw",
            "ptcsdynamicpanel" => "dynamic panels have two native panes and varied push/trigger controls; side_panel has a different structure",
            "eventsrouter" => "router needs 1 to 20 typed native inputs; unsupported counts remain raw",
            "navigationfunction" => "navigation needs a static target and a single representable Navigate trigger; dynamic targets remain raw",
            "ptcschartschedule" => "schedule charts need nested resource intervals rather than flat numeric series",
            "ptcschartbar" | "ptcschartline" | "ptcschartpie" | "ptcschartpareto" | "ptcschartwaterfall" | "ptcschartcombo" => "chart needs a supported flat table feed and axis fields (pie: DataField and ValueField); unconfigured or dynamic feeds remain raw",
            _ => "no matching element contract; explicit widget properties are retained",
        };
        let extension = matches!(
            type_id.as_str(),
            "googlemap" | "OpenStreetMap" | "openstreetmap"
        );
        if !self.exact && !extension && widgets::catalog().resolve(&type_id).is_err() {
            // A page built by the layout rules refuses a widget the catalogue does not know;
            // only a page built as found carries one verbatim.
            self.notes.push(format!(
                "widget left out: {type_id} {} (not in the widget catalogue; `mashup spec` on a mashup built by hand keeps it)",
                self.id(widget)
            ));
            return Value::Null;
        }
        self.notes_once(&format!("{type_id}: {reason}"));
        let mut body = Map::new();
        body.insert("type".into(), json!(type_id));
        let declared = inputs(widget);
        if !declared.is_empty() {
            body.insert("inputs".into(), json!(declared));
        }
        let props = self.props(widget, &[]);
        if !props.is_empty() {
            body.insert("props".into(), Value::Object(props));
        }
        if self.exact && !widgets_of(widget).is_empty() {
            body.insert(
                "children".into(),
                widgets_of(widget)
                    .iter()
                    .map(|child| self.node(child, "column"))
                    .collect(),
            );
        } else if !widgets_of(widget).is_empty() {
            self.notes.push(format!(
                "the children of {} {} were left out",
                type_id,
                self.id(widget)
            ));
        }
        let id = self.id(widget);
        self.count("widget", Some(&type_id));
        let mut ordered = Map::new();
        ordered.insert("id".into(), json!(id));
        ordered.append(&mut body);
        json!({"widget": ordered})
    }

    fn label(&mut self, widget: &Value) -> Value {
        let label_type = text(widget, "LabelType").unwrap_or_else(|| "label".into());
        let (element, skip_type) = match label_type.as_str() {
            "header" => ("heading", true),
            "body" => ("text", true),
            "label" => ("label", true),
            _ => ("label", false),
        };
        let bound_link = self
            .links
            .iter()
            .find(|link| {
                !link.consumed
                    && link.target.0 == "UI"
                    && link.target.1 == prop(widget, "Id").as_str().unwrap_or_default()
                    && link.target.3 == "LabelText"
            })
            .cloned();
        let bound = self.take(widget, "LabelText");
        let mut body = Map::new();
        if bound.is_none() {
            if let Some(content) = text(widget, "LabelText").filter(|t| t != "[[Label]]") {
                body.insert("text".into(), json!(content));
            }
        }
        let skip: &[&str] = if skip_type {
            &["LabelText", "LabelType"]
        } else {
            &["LabelText"]
        };
        let mut props = self.props(widget, skip);
        if let Some(bound) = bound {
            // A binding tells the runtime where a value comes from; it must not erase the
            // design-time text visible before that source fires. With such text the label keeps
            // it and the binding is listed under `bindings`; without, the inline `bind:` stays.
            let design = prop(widget, "LabelText")
                .as_str()
                .filter(|value| !value.starts_with("[["))
                .map(str::to_string);
            match (design, bound_link) {
                (Some(value), Some(link)) => {
                    props.insert("LabelText".into(), json!(value));
                    self.links.push(link);
                }
                _ => {
                    props.insert("LabelText".into(), bound);
                }
            }
        }
        self.finish(element, widget, body, props)
    }

    fn button(&mut self, widget: &Value) -> Value {
        let mut body = Map::new();
        if let Some(label) = text(widget, "Label").filter(|l| l != "[[Button]]") {
            body.insert("text".into(), json!(label));
        }
        if let Some(variant) = text(widget, "ButtonType").filter(|v| v != "primary") {
            body.insert("variant".into(), json!(variant));
        }
        if let Some(icon) = text(widget, "SVGIcon") {
            body.insert("icon".into(), json!(icon));
        }
        let props = self.props(widget, &["Label", "ButtonType", "SVGIcon"]);
        self.finish("button", widget, body, props)
    }

    fn native(&mut self, widget: &Value) -> Value {
        let (element, fields): (&str, &[(&str, &str)]) = match kind(widget) {
            "statusmessage" => (
                "status_message",
                &[("Message", "message"), ("MessageType", "message_type")],
            ),
            "ptcsrte" => (
                "rich_text",
                &[
                    ("Data", "content"),
                    ("ReadOnly", "read_only"),
                    ("DefaultFontSize", "font_size"),
                ],
            ),
            "ptcslink" => (
                "link",
                &[
                    ("Text", "text"),
                    ("LinkURL", "url"),
                    ("TargetWindow", "target"),
                ],
            ),
            "ptcsimage" => (
                "image",
                &[
                    ("SourceURL", "source"),
                    ("AlternateText", "alt"),
                    ("Scaling", "scaling"),
                    ("Position", "position"),
                ],
            ),
            "ptcsicon" => (
                "icon",
                &[
                    ("SVGIcon", "icon"),
                    ("SourceURL", "source"),
                    ("AlternateText", "alt"),
                    ("Size", "icon_size"),
                ],
            ),
            "ptcsdivider" => ("divider", &[]),
            "ptcscheckbox" => ("checkbox", &[("Label", "text"), ("State", "value")]),
            "ptcsradio" => (
                "radio",
                &[
                    ("Label", "text"),
                    ("State", "value"),
                    ("Radiogroup", "group"),
                ],
            ),
            "ptcstogglebutton" => ("toggle", &[("Label", "text"), ("State", "value")]),
            "ptcstextarea" => (
                "textarea",
                &[
                    ("Label", "text"),
                    ("Text", "value"),
                    ("HintText", "hint"),
                    ("MaxNumberOfCharacters", "max_length"),
                ],
            ),
            "ptcsdatepicker" => (
                "date_picker",
                &[
                    ("Label", "text"),
                    ("DateTime", "value"),
                    ("HintText", "hint"),
                    ("DateOnly", "date_only"),
                    ("FormatToken", "format"),
                ],
            ),
            "ptcsvaluedisplay" => ("value_display", &[("Label", "text"), ("Data", "value")]),
            "ptcspagination" => (
                "pagination",
                &[
                    ("ResultsNumber", "total"),
                    ("PageNumber", "page_number"),
                    ("PageSize", "page_size"),
                ],
            ),
            "ptcskpidial" => (
                "kpi_dial",
                &[
                    ("Label", "title"),
                    ("Value", "value"),
                    ("MinimumValue", "min"),
                    ("MaximumValue", "max"),
                    ("UnitOfMeasure", "unit"),
                ],
            ),
            _ => unreachable!(),
        };
        let mut body = Map::new();
        let mut extra = Map::new();
        let mut skip: Vec<&str> = fields.iter().map(|(property, _)| *property).collect();
        let mut carried_bindings = Vec::new();
        for (property, field) in fields {
            let binding = self
                .links
                .iter()
                .find(|link| {
                    !link.consumed
                        && link.target.0 == "UI"
                        && link.target.1 == prop(widget, "Id").as_str().unwrap_or_default()
                        && link.target.3 == *property
                })
                .cloned();
            if binding.is_some() {
                // Consume the original before `props` sees it; it is restored after the
                // authored field has become a real design-time value in the spec.
                self.take(widget, property);
            }
            let value = widget["Properties"].get(*property).cloned();
            if let Some(binding) = binding {
                carried_bindings.push(binding);
            }
            if let Some(value) = value.filter(|v| !v.is_null() && *v != "") {
                // These scalar fields describe catalogue choices; retain an unusual binding as a prop.
                if value.is_object()
                    && ![
                        "text", "source", "alt", "value", "group", "total", "message", "content",
                    ]
                    .contains(field)
                {
                    extra.insert((*property).into(), value);
                    continue;
                }
                body.insert((*field).into(), value);
            }
        }
        if element == "rich_text" {
            if let Some(hidden) = prop(widget, "HideToolbar").as_bool() {
                body.insert("show_toolbar".into(), json!(!hidden));
            }
            skip.push("HideToolbar");
        }
        if element == "divider" {
            body.insert(
                "orientation".into(),
                json!(if prop(widget, "VerticalDivider") == true {
                    "vertical"
                } else {
                    "horizontal"
                }),
            );
            skip.push("VerticalDivider");
        }
        if element == "date_picker" {
            skip.push("DateRange");
        }
        if element == "pagination" {
            body.insert("mode".into(), json!("server"));
        }
        if element == "kpi_dial" {
            let layout = text(widget, "Layout").unwrap_or_else(|| "dial".into());
            if !["dial", "horizontal", "vertical"].contains(&layout.as_str()) {
                return self.other(widget);
            }
            body.insert("kind".into(), json!(layout));
            skip.push("Layout");
        }
        let mut props = self.props(widget, &skip);
        self.links.extend(carried_bindings);
        props.extend(extra);
        self.finish(element, widget, body, props)
    }

    fn embed(&mut self, widget: &Value) -> Value {
        let mut body = Map::new();
        body.insert("mashup".into(), prop(widget, "Name").clone());
        let mut params = Map::new();
        let mut skip = vec!["Name", "MashupParameters"];
        for entry in prop(widget, "MashupParameters")
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(name) = entry["ParameterName"].as_str() else {
                continue;
            };
            if let Some(value) = self
                .take(widget, name)
                .or_else(|| widget["Properties"].get(name).cloned())
            {
                params.insert(name.into(), value);
            }
            skip.push(name);
        }
        if !params.is_empty() {
            body.insert("params".into(), Value::Object(params));
        }
        let props = self.props(widget, &skip);
        self.finish("embed", widget, body, props)
    }

    fn router(&mut self, widget: &Value) -> Value {
        let count = whole(prop(widget, "NumberOfInputs")).unwrap_or(0);
        if !(1..=20).contains(&count) {
            return self.other(widget);
        }
        let mut body = Map::new();
        body.insert("input_count".into(), json!(count));
        body.insert("data_type".into(), prop(widget, "DataType").clone());
        let mut routes = vec![];
        let names: Vec<_> = (1..=count).map(|n| format!("Input{n}")).collect();
        let mut skip = vec!["NumberOfInputs", "DataType"];
        for name in &names {
            let route = self.take(widget, name);
            // An input holding anything but a binding is no route; a literal stays a prop, which
            // build accepts, and an empty object is just the absence of one.
            let literal = route.is_none()
                && widget["Properties"].get(name).is_some_and(|v| {
                    !v.is_null() && *v != "" && v.as_object().is_none_or(|m| !m.is_empty())
                });
            if !literal {
                skip.push(name);
            }
            routes.push(route.unwrap_or(Value::Null));
        }
        if routes.iter().any(|v| !v.is_null()) {
            body.insert("routes".into(), Value::Array(routes));
        }
        let props = self.props(widget, &skip);
        self.finish("events_router", widget, body, props)
    }

    fn navigation(&mut self, widget: &Value) -> Value {
        let id = prop(widget, "Id").as_str().unwrap_or("");
        let events: Vec<_> = self.content["Events"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| {
                e["EventHandlerArea"] == "UI"
                    && e["EventHandlerId"] == id
                    && e["EventHandlerService"] == "Navigate"
            })
            .collect();
        let target = text(widget, "TargetMashup").filter(|s| !s.is_empty());
        if self.exact
            || target.is_none()
            || events.len() != 1
            || (self.exact
                && !id
                    .rsplit_once('-')
                    .is_some_and(|(_, suffix)| suffix.parse::<u64>().is_ok()))
            || self.links.iter().any(|l| {
                (l.source.0 == "UI" && l.source.1 == id)
                    || (l.target.0 == "UI"
                        && l.target.1 == id
                        && matches!(
                            l.target.3.as_str(),
                            "PopupTitle" | "PopupWidth" | "PopupHeight"
                        ))
            })
            || self
                .links
                .iter()
                .any(|l| l.target.1 == id && l.target.3 == "TargetMashup")
        {
            return self.other(widget);
        }
        let mut action = Map::new();
        action.insert("id".into(), json!(self.id(widget)));
        action.insert("mashup".into(), json!(target.expect("target")));
        action.insert("mode".into(), prop(widget, "TargetWindowType").clone());
        let mut params = Map::new();
        let mut skip = vec![
            "TargetMashup",
            "TargetWindowType",
            "MashupParameters",
            "_currentParameterDefs",
            "PopupTitle",
            "PopupWidth",
            "PopupHeight",
        ];
        for definition in prop(widget, "MashupParameters")
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(name) = definition["ParameterName"].as_str() else {
                continue;
            };
            if let Some(value) = self
                .take(widget, name)
                .or_else(|| widget["Properties"].get(name).cloned())
            {
                params.insert(name.into(), value);
            }
            skip.push(name);
        }
        if !params.is_empty() {
            action.insert("params".into(), Value::Object(params));
        }
        for (prop_name, field) in [
            ("PopupTitle", "title"),
            ("PopupWidth", "width"),
            ("PopupHeight", "height"),
        ] {
            if let Some(value) = widget["Properties"].get(prop_name).filter(|v| !v.is_null()) {
                action.insert(field.into(), value.clone());
            }
        }
        let props = self.props(widget, &skip);
        if !props.is_empty() {
            action.insert("props".into(), Value::Object(props));
        }
        self.navigations.insert(id.into(), Value::Object(action));
        self.count("navigate", None);
        Value::Null
    }

    fn expression(&mut self, widget: &Value) -> Value {
        let alias = self.expression_names[prop(widget, "Id").as_str().unwrap_or("")].clone();
        let mut entry = Map::new();
        if self.exact {
            entry.insert("widget_id".into(), prop(widget, "Id").clone());
        }
        entry.insert("expression".into(), prop(widget, "Expression").clone());
        entry.insert("output".into(), prop(widget, "OutputBaseType").clone());
        let mut declared = Map::new();
        for definition in prop(widget, "_currentParameterDefs")
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(name) = definition["ParameterName"].as_str() else {
                continue;
            };
            let mut input = Map::new();
            input.insert("type".into(), definition["BaseType"].clone());
            if let Some(value) = self
                .take(widget, name)
                .or_else(|| widget["Properties"].get(name).cloned())
            {
                input.insert("value".into(), value);
            }
            declared.insert(name.into(), Value::Object(input));
        }
        let input_names: Vec<String> = declared.keys().cloned().collect();
        if !declared.is_empty() {
            entry.insert("inputs".into(), Value::Object(declared));
        }
        for (prop_name, field) in [
            ("AutoEvaluate", "auto_evaluate"),
            ("FireOnFirstValue", "fire_on_first_value"),
            ("DataChangeType", "data_change"),
        ] {
            if let Some(value) = widget["Properties"].get(prop_name) {
                entry.insert(field.into(), value.clone());
            }
        }
        if self.exact {
            // What the function carries beyond the spec's own fields, so an adopted page keeps it.
            let defaults = widgets::catalog()
                .by_type("expression2")
                .map(|w| w.defaults.clone())
                .unwrap_or_default();
            let mut props = Map::new();
            for (key, value) in widget["Properties"].as_object().into_iter().flatten() {
                let named = key == "DisplayName" && !generated_name(widget, value);
                if (noise(key) && !named)
                    || matches!(
                        key.as_str(),
                        "Expression"
                            | "OutputBaseType"
                            | "AutoEvaluate"
                            | "FireOnFirstValue"
                            | "DataChangeType"
                    )
                    || input_names.contains(key)
                    || value.is_object()
                    || value.is_array()
                    || defaults.get(key).is_some_and(|d| same(d, value))
                    || (named && value.as_str() == Some(""))
                {
                    continue;
                }
                props.insert(key.clone(), value.clone());
            }
            if !props.is_empty() {
                entry.insert("props".into(), Value::Object(props));
            }
        }
        self.expressions.insert(alias, Value::Object(entry));
        self.count("expression", None);
        Value::Null
    }

    fn picker(&mut self, widget: &Value) -> Value {
        let is_list = kind(widget) == "ptcslist";
        let display = text(widget, "DisplayField");
        let fed = display.as_ref().and_then(|_| self.feed(widget));
        let (Some(display), Some(data)) = (display, fed) else {
            if is_list {
                return self.other(widget);
            }
            let mut body = Map::new();
            body.insert("unbound".into(), json!(true));
            if let Some(display) = text(widget, "DisplayField") {
                body.insert("display".into(), json!(display));
            }
            if let Some(hint) = text(widget, "HintText") {
                body.insert("hint".into(), json!(hint));
            }
            let props = self.props(widget, &["DisplayField", "HintText"]);
            return self.finish("dropdown", widget, body, props);
        };
        let mut body = Map::new();
        let mut skip = vec!["DisplayField", "DisplayFieldBaseType", "HintText"];
        if is_list {
            body.insert("data".into(), json!(data));
            body.insert("display".into(), json!(display));
            if prop(widget, "MultiSelect") == &json!(true) {
                body.insert("selection".into(), json!("multiple"));
            }
            skip.extend(["MultiSelect", "ClearSelectionItem"]);
        } else {
            body.insert("options".into(), json!(data));
            body.insert("display".into(), json!(display));
            if let Some(hint) = text(widget, "HintText").filter(|h| h != "[[Select]]") {
                body.insert("hint".into(), json!(hint));
            }
        }
        let props = self.props(widget, &skip);
        self.finish(
            if is_list { "list" } else { "dropdown" },
            widget,
            body,
            props,
        )
    }

    fn grid(&mut self, widget: &Value) -> Value {
        let data = self.feed(widget);
        let mut body = Map::new();
        if let Some(data) = data {
            body.insert("data".into(), json!(data));
        }
        let columns: Vec<Value> = prop(widget, "ColumnFormat")["formatInfo"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let field = c["FieldName"].as_str()?;
                let title = c["Title"].as_str().filter(|t| *t != field);
                let width = whole(&c["Width"]).filter(|w| *w > 0);
                Some(if title.is_none() && width.is_none() {
                    json!(field)
                } else {
                    let mut column = Map::new();
                    column.insert("field".into(), json!(field));
                    if let Some(title) = title {
                        column.insert("title".into(), json!(title));
                    }
                    if let Some(width) = width {
                        column.insert("width".into(), json!(width));
                    }
                    Value::Object(column)
                })
            })
            .collect();
        if !columns.is_empty() {
            body.insert("columns".into(), Value::Array(columns));
        }
        let props = self.props(
            widget,
            if body.contains_key("data") {
                &["ColumnFormat", "Label"]
            } else {
                body.remove("columns");
                &["Label"]
            },
        );
        self.finish("grid", widget, body, props)
    }

    fn chart(&mut self, widget: &Value) -> Value {
        let pie = kind(widget) == "ptcschartpie";
        if pie && text(widget, "ValueField").is_none() {
            return self.other(widget);
        }
        if kind(widget) == "ptcschartcombo" {
            let count = whole(prop(widget, "NumberOfSeries")).unwrap_or(0);
            if count > 8
                || !(1..=count).any(|n| {
                    text(widget, &format!("DataField{n}")).is_some()
                        && text(widget, &format!("Series{n}Type")).is_some()
                })
            {
                return self.other(widget);
            }
        }
        let x = text(widget, if pie { "DataField" } else { "XAxisField" });
        let data = x.as_ref().and_then(|_| self.feed(widget));
        let (Some(x), Some(data)) = (x, data) else {
            return self.other(widget);
        };
        let chart = kind(widget).strip_prefix("ptcschart").unwrap_or("line");
        let mut body = Map::new();
        body.insert("kind".into(), json!(chart));
        body.insert("data".into(), json!(data));
        body.insert("x".into(), json!(x));
        if chart == "pie" {
            let Some(value) = text(widget, "ValueField") else {
                return self.other(widget);
            };
            body.insert("value".into(), json!(value));
        }
        if chart == "combo" {
            let count = whole(prop(widget, "NumberOfSeries")).unwrap_or(0);
            let mut series = vec![];
            for n in 1..=count.min(8) {
                let (Some(field), Some(draw)) = (
                    text(widget, &format!("DataField{n}")),
                    text(widget, &format!("Series{n}Type")),
                ) else {
                    continue;
                };
                let mut entry = Map::new();
                entry.insert("field".into(), json!(field));
                entry.insert(
                    "kind".into(),
                    json!(if draw == "line" { "line" } else { "bar" }),
                );
                if text(widget, &format!("DisplaySeries{n}On")).as_deref() == Some("yaxis2") {
                    entry.insert("axis".into(), json!("secondary"));
                }
                series.push(Value::Object(entry));
            }
            if series.is_empty() {
                return self.other(widget);
            }
            body.insert("series".into(), Value::Array(series));
        }
        let props = self.props(
            widget,
            &[
                "XAxisField",
                "ValueField",
                "DataField*",
                "Series*",
                "DisplaySeries*",
                "DataSeries*",
                "NumberOfSeries",
                "NumberOfYaxes",
                "YAxis2Label",
                "ChartType",
                "Label",
            ],
        );
        self.finish("chart", widget, body, props)
    }

    fn collection(&mut self, widget: &Value) -> Value {
        let Some(cell) = text(widget, "Mashup").filter(|s| !s.is_empty()) else {
            return self.other(widget);
        };
        let map = prop(widget, "MashupPropertyBinding")
            .as_object()
            .cloned()
            .unwrap_or_default();
        if map.values().any(|v| !v.is_string()) {
            return self.other(widget);
        }
        let mut body = Map::new();
        body.insert("mashup".into(), json!(cell));
        if !map.is_empty() {
            body.insert("map".into(), Value::Object(map));
        }
        if let Some(source) = self.take(widget, "Data") {
            body.insert("source".into(), source);
        } else {
            body.insert("unbound".into(), json!(true));
        }
        let props = self.props(widget, &["Mashup", "MashupPropertyBinding", "Data"]);
        self.finish("collection", widget, body, props)
    }

    fn notes_once(&mut self, note: &str) {
        if !self.notes.iter().any(|n| n == note) {
            self.notes.push(note.into());
        }
    }

    fn tabs(&mut self, widget: &Value) -> Value {
        let kids = widgets_of(widget);
        let count = whole(prop(widget, "NumberOfTabs")).unwrap_or(kids.len());
        let labels: Vec<String> = (1..=count)
            .map(|n| {
                prop(widget, "Items")
                    .get(n - 1)
                    .and_then(|v| v["name"].as_str())
                    .map(str::to_owned)
                    .or_else(|| text(widget, &format!("Tab{n}Name")))
                    .unwrap_or_default()
            })
            .collect();
        let unique: BTreeSet<_> = labels.iter().collect();
        if count == 0
            || count > 16
            || kids.len() != count
            || unique.len() != count
            || labels.iter().any(String::is_empty)
            || !kids.iter().all(is_container)
        {
            return self.other(widget);
        }
        let skip: Vec<String> = std::iter::once("NumberOfTabs".to_string())
            .chain((1..=16).map(|n| format!("Tab{n}Name")))
            .collect();
        let skip: Vec<&str> = skip.iter().map(String::as_str).collect();
        let props = self.props(widget, &skip);
        let tabs: Vec<Value> = labels
            .iter()
            .zip(kids)
            .map(|(label, tab)| {
                let own = if self.exact {
                    self.props(tab, &["DisplayName"])
                } else {
                    Map::new()
                };
                let content: Vec<Value> = widgets_of(tab)
                    .iter()
                    .map(|w| self.node(w, "column"))
                    .collect();
                let mut entry = json!({"label": label});
                if !own.is_empty() {
                    entry["props"] = Value::Object(own);
                }
                entry["content"] = json!(content);
                entry
            })
            .collect();
        let mut body = Map::new();
        body.insert("tabs".into(), Value::Array(tabs));
        self.finish("tabs", widget, body, props)
    }

    fn container(&mut self, widget: &Value, parent: &str) -> Value {
        let direction = if prop(widget, "flex-direction") == "column" {
            "column"
        } else {
            "row"
        };
        let found = (!self.exact)
            .then(|| {
                self.side_panel(widget)
                    .or_else(|| self.filter_panel(widget))
                    .or_else(|| self.card(widget))
                    .or_else(|| self.kpi_row(widget))
                    .or_else(|| self.page_header(widget, direction))
                    .or_else(|| self.toolbar(widget, direction))
            })
            .flatten();
        if let Some(found) = found {
            return found;
        }
        if !self.exact && is_spacer(widget) && parent == "row" {
            let id = self.id(widget);
            return self.make("spacer", id, Map::new());
        }
        let mut body = Map::new();
        let mut skip = vec!["flex-direction"];
        let grow = number(prop(widget, "flex-grow")).unwrap_or(1.0);
        if self.exact {
            // Shorthands would let the rules decide; the spec states what the mashup has.
        } else if grow == 0.0 {
            let dimension = if parent == "row" { "width" } else { "height" };
            let min = pixels(prop(widget, &format!("flex-min-{dimension}")));
            let max = pixels(prop(widget, &format!("flex-max-{dimension}")));
            if min.is_some() && min == max {
                body.insert("size".into(), json!({"fixed": min}));
                skip.extend([
                    if parent == "row" {
                        "flex-min-width"
                    } else {
                        "flex-min-height"
                    },
                    if parent == "row" {
                        "flex-max-width"
                    } else {
                        "flex-max-height"
                    },
                ]);
            } else {
                body.insert("size".into(), json!("fit"));
            }
            skip.push("flex-grow");
        } else if grow > 0.0 && (grow - 1.0).abs() > f64::EPSILON {
            body.insert(
                "weight".into(),
                whole(&json!(grow)).map_or_else(|| json!(grow), |n| json!(n)),
            );
            skip.push("flex-grow");
        }
        let props = self.props(widget, &skip);
        let kids: Vec<Value> = widgets_of(widget)
            .iter()
            .map(|w| self.node(w, direction))
            .collect();
        if !props.is_empty() {
            body.insert("props".into(), Value::Object(props));
        }
        if !kids.is_empty() {
            body.insert("children".into(), Value::Array(kids));
        }
        let id = self.id(widget);
        self.make(direction, id, body)
    }

    fn side_panel(&mut self, widget: &Value) -> Option<Value> {
        let [head, rest] = widgets_of(widget) else {
            return None;
        };
        if prop(widget, "flex-direction") != "column"
            || pixels(prop(head, "flex-min-height")) != Some(72)
            || !is_container(rest)
        {
            return None;
        }
        let title = match widgets_of(head) {
            [label] | [label, _] if kind(label) == "ptcslabel" => label,
            _ => return None,
        };
        let help = match widgets_of(head) {
            [_, button]
                if kind(button) == "ptcsbutton" && prop(button, "Disabled") == &json!(true) =>
            {
                Some(text(button, "TooltipField")?)
            }
            [_] => None,
            _ => return None,
        };
        let [tabset] = widgets_of(rest) else {
            return None;
        };
        let tab_count = number(prop(tabset, "NumberOfTabs")).unwrap_or(1.0);
        let [content] = widgets_of(tabset) else {
            return None;
        };
        if kind(tabset) != "ptcstabset" || tab_count != 1.0 || !is_container(content) {
            return None;
        }
        let mut body = Map::new();
        body.insert("title".into(), json!(text(title, "LabelText")?));
        if let Some(help) = help {
            body.insert("help".into(), json!(help));
        }
        if let Some(tab) = text(tabset, "Tab1Name").filter(|t| t != "Filters") {
            body.insert("tab".into(), json!(tab));
        }
        let kids: Vec<Value> = widgets_of(content)
            .iter()
            .map(|w| self.node(w, "column"))
            .collect();
        if !kids.is_empty() {
            body.insert("children".into(), Value::Array(kids));
        }
        let id = self.id(widget);
        Some(self.make("side_panel", id, body))
    }

    fn filter_panel(&mut self, widget: &Value) -> Option<Value> {
        let [fields, footer] = widgets_of(widget) else {
            return None;
        };
        if prop(widget, "flex-direction") != "column"
            || !is_container(fields)
            || !is_container(footer)
        {
            return None;
        }
        let [divider, buttons] = widgets_of(footer) else {
            return None;
        };
        if !is_container(divider)
            || !widgets_of(divider).is_empty()
            || pixels(prop(divider, "flex-min-height")) != Some(1)
            || !is_container(buttons)
        {
            return None;
        }
        let row = widgets_of(buttons);
        let mut footer_names = vec![];
        if row.is_empty() || row.len() > 2 || row.iter().any(|b| kind(b) != "ptcsbutton") {
            return None;
        }
        for button in row {
            let name = if prop(button, "ButtonType") == "primary" {
                "apply"
            } else {
                "reset"
            };
            if footer_names.contains(&name) {
                return None;
            }
            footer_names.push(name);
        }
        let mut controls = widgets_of(fields);
        let mut title = None;
        if let [first, rest @ ..] = controls {
            if kind(first) == "ptcslabel" && prop(first, "LabelType") == "title" {
                title = text(first, "LabelText");
                controls = rest;
            }
        }
        if controls.is_empty() || !controls.iter().all(is_leaf) {
            return None;
        }
        let id = self.id(widget);
        for (button, name) in row.iter().zip(&footer_names) {
            if let Some(orig) = prop(button, "Id").as_str() {
                self.ids.insert(orig.into(), format!("{id}--{name}"));
            }
        }
        let fields: Vec<Value> = controls.iter().map(|w| self.node(w, "column")).collect();
        let mut body = Map::new();
        if let Some(title) = title {
            body.insert("title".into(), json!(title));
        }
        body.insert("fields".into(), Value::Array(fields));
        body.insert("footer".into(), json!(footer_names));
        Some(self.make("filter_panel", id, body))
    }

    /// A framed section: a column holding a 40px title row over a body row.
    fn card(&mut self, widget: &Value) -> Option<Value> {
        if prop(widget, "flex-direction") != "column" {
            return None;
        }
        let [head, body_row] = widgets_of(widget) else {
            return None;
        };
        let [label] = widgets_of(head) else {
            return None;
        };
        if pixels(prop(head, "flex-min-height")) != Some(40)
            || kind(label) != "ptcslabel"
            || prop(label, "LabelType") != "sub-header"
            || !is_container(body_row)
        {
            return None;
        }
        let centered = prop(body_row, "justify-content") == "center"
            && prop(body_row, "align-items") == "center";
        let mut body = Map::new();
        body.insert("title".into(), json!(text(label, "LabelText")?));
        body.insert(
            "align".into(),
            json!(if centered { "center" } else { "start" }),
        );
        let kids: Vec<Value> = widgets_of(body_row)
            .iter()
            .map(|w| self.node(w, "row"))
            .collect();
        if !kids.is_empty() {
            body.insert("children".into(), Value::Array(kids));
        }
        let id = self.id(widget);
        Some(self.make("card", id, body))
    }

    fn kpi_row(&mut self, widget: &Value) -> Option<Value> {
        let tiles = widgets_of(widget);
        if prop(widget, "flex-direction") == "column" || tiles.is_empty() {
            return None;
        }
        let mut found = vec![];
        for tile in tiles {
            let labels = widgets_of(tile);
            if !is_container(tile)
                || labels.len() != 3
                || labels.iter().any(|l| kind(l) != "ptcslabel")
                || prop(&labels[0], "LabelType") != "label"
                || prop(&labels[1], "LabelType") != "title"
                || prop(&labels[2], "LabelType") != "label"
            {
                return None;
            }
            let id = prop(&labels[1], "Id").as_str()?;
            let tied = self.links.iter().any(|l| {
                !l.consumed
                    && l.target.0 == "UI"
                    && l.target.1 == id
                    && l.target.3 == "LabelText"
                    && l.source.0 == "Data"
                    && !l.source.2.ends_with('\u{1}')
            });
            if !tied {
                return None;
            }
            found.push(tile);
        }
        let mut entries = vec![];
        for tile in found {
            let labels = widgets_of(tile);
            let value = self.take(&labels[1], "LabelText")?;
            let mut entry = Map::new();
            entry.insert("id".into(), json!(self.id(tile)));
            entry.insert(
                "label".into(),
                json!(text(&labels[0], "LabelText").unwrap_or_default()),
            );
            entry.insert("value".into(), value);
            entry.insert(
                "unit".into(),
                json!(text(&labels[2], "LabelText").unwrap_or_default()),
            );
            entries.push(Value::Object(entry));
        }
        let mut body = Map::new();
        body.insert("tiles".into(), Value::Array(entries));
        let id = self.id(widget);
        Some(self.make("kpi_row", id, body))
    }

    fn page_header(&mut self, widget: &Value, direction: &str) -> Option<Value> {
        let kids = widgets_of(widget);
        let [title, rest @ ..] = kids else {
            return None;
        };
        if direction != "row"
            || kind(title) != "ptcslabel"
            || prop(title, "LabelType") != "header"
            || !rest.iter().all(|w| kind(w) == "ptcsbutton" || is_spacer(w))
            || rest.iter().any(|w| prop(w, "Disabled") == &json!(true))
        {
            return None;
        }
        let mut body = Map::new();
        body.insert("title".into(), json!(text(title, "LabelText")?));
        let mut actions = vec![];
        for button in rest.iter().filter(|w| kind(w) == "ptcsbutton") {
            let mut entry = Map::new();
            entry.insert("id".into(), json!(self.id(button)));
            if let Some(label) = text(button, "Label") {
                entry.insert("label".into(), json!(label));
            }
            if let Some(variant) = text(button, "ButtonType").filter(|v| v != "primary") {
                entry.insert("variant".into(), json!(variant));
            }
            if let Some(icon) = text(button, "SVGIcon") {
                entry.insert("icon".into(), json!(icon));
            }
            actions.push(Value::Object(entry));
        }
        if !actions.is_empty() {
            body.insert("actions".into(), Value::Array(actions));
        }
        let id = self.id(widget);
        Some(self.make("page_header", id, body))
    }

    fn toolbar(&mut self, widget: &Value, direction: &str) -> Option<Value> {
        let kids = widgets_of(widget);
        if direction != "row"
            || pixels(prop(widget, "flex-min-height")) != Some(48)
            || kids.is_empty()
            || !kids.iter().all(|w| is_leaf(w) || is_spacer(w))
        {
            return None;
        }
        let nodes: Vec<Value> = kids.iter().map(|w| self.node(w, "row")).collect();
        let mut body = Map::new();
        body.insert("children".into(), Value::Array(nodes));
        let id = self.id(widget);
        Some(self.make("toolbar", id, body))
    }

    fn wiring(&mut self) -> (Map<String, Value>, Vec<Value>) {
        let mut data = self.expressions.clone();
        for kind in ["Session", "UserExtensions"] {
            if self.content["Data"].get(kind).is_some() {
                let mut fields = Map::new();
                for binding in self.content["DataBindings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if binding["SourceArea"] == kind {
                        for map in binding["PropertyMaps"].as_array().into_iter().flatten() {
                            if let Some(name) = map["SourceProperty"].as_str() {
                                fields.insert(name.into(), map["SourcePropertyBaseType"].clone());
                            }
                        }
                    }
                }
                if !fields.is_empty() {
                    data.insert(kind.into(), json!({"session":kind,"fields":fields}));
                }
            }
        }
        let mut events = vec![];
        let mut loaded: BTreeSet<String> = BTreeSet::new();
        for event in self.content["Events"].as_array().into_iter().flatten() {
            let get = |k: &str| event[k].as_str().unwrap_or("");
            let trigger =
                if get("EventTriggerArea") == "Mashup" && get("EventTriggerEvent") == "Loaded" {
                    None
                } else if get("EventTriggerArea") == "Mashup" {
                    Some(format!("mashup.{}", get("EventTriggerEvent")))
                } else if get("EventTriggerArea") == "UI" {
                    Some(format!(
                        "@@{}@@.{}",
                        get("EventTriggerId"),
                        get("EventTriggerEvent")
                    ))
                } else if get("EventTriggerArea") == "Data" {
                    let Some(call) = self.calls.iter().find(|c| {
                        c.section == get("EventTriggerSection") && c.name == get("EventTriggerId")
                    }) else {
                        self.dropped += 1;
                        continue;
                    };
                    let event = match get("EventTriggerEvent") {
                        "ServiceInvokeCompleted" => "done",
                        "Data_Error" | "ServiceInvokeError" => "error",
                        event => event,
                    };
                    Some(format!("{}.{event}", call.alias))
                } else {
                    self.dropped += 1;
                    continue;
                };
            if get("EventHandlerArea") == "UI" && get("EventHandlerService") == "Navigate" {
                if let Some(navigate) = self.navigations.get(get("EventHandlerId")) {
                    events.push(
                        json!({"on": trigger.as_deref().unwrap_or("load"), "navigate": navigate}),
                    );
                    continue;
                }
            }
            if get("EventHandlerArea") == "UI" && get("EventHandlerService") == "ShowMessage" {
                events.push(json!({"on": trigger.as_deref().unwrap_or("load"), "show": format!("@@{}@@", get("EventHandlerId"))}));
                continue;
            }
            if get("EventHandlerArea") == "UI" {
                let unlisted = widget_type(&self.content["UI"], get("EventHandlerId"))
                    .and_then(|t| widgets::catalog().by_type(t))
                    .filter(|w| w.services.is_some())
                    .is_some_and(|w| !w.has_service(get("EventHandlerService")));
                if unlisted {
                    self.notes.push(format!(
                        "event left out: {} -> {}.{} (the widget catalogue lists no such service)",
                        get("EventTriggerEvent"),
                        get("EventHandlerId"),
                        get("EventHandlerService")
                    ));
                    continue;
                }
            }
            let target = match get("EventHandlerArea") {
                "Mashup" => Some(format!("mashup.{}", get("EventHandlerService"))),
                "Data" => self
                    .calls
                    .iter()
                    .find(|c| {
                        c.section == get("EventHandlerId") && c.name == get("EventHandlerService")
                    })
                    .map(|c| c.alias.clone()),
                "UI" => Some(format!(
                    "@@{}@@.{}",
                    get("EventHandlerId"),
                    get("EventHandlerService")
                )),
                _ => None,
            };
            let Some(target) = target else {
                self.dropped += 1;
                continue;
            };
            match trigger {
                None if self.calls.iter().any(|c| c.alias == target) => {
                    loaded.insert(target);
                }
                None => events.push(json!({"on": "load", "invoke": target})),
                Some(on) => events.push(json!({"on": on, "invoke": target})),
            }
        }
        for call in &self.calls {
            let mut entry = Map::new();
            entry.insert("service".into(), json!(call.service));
            if loaded.contains(&call.alias) {
                entry.insert("on".into(), json!("load"));
                if call.refresh > 0 {
                    entry.insert("refresh".into(), json!(call.refresh));
                }
            } else if call.refresh > 0 {
                entry.insert("on".into(), json!(format!("every {}s", call.refresh)));
            }
            if call.dynamic {
                match self.entity_link(&call.section) {
                    Some((i, from)) => {
                        entry.insert("entity".into(), json!({"bind": from}));
                        self.links[i].consumed = true;
                    }
                    None if call.section.starts_with("DynamicThingTemplates") => {
                        self.notes.push(format!(
                            "{}: a dynamic template source with no entity binding cannot run; it is written as a plain service",
                            call.section
                        ));
                    }
                    None => {}
                }
            }
            let mut params = call.params.clone();
            let mut fixed = Map::new();
            for i in 0..self.links.len() {
                let link = &self.links[i];
                if link.target.0 == "Data"
                    && link.target.1 == call.name
                    && link.target.2 == call.section
                    && !link.consumed
                    && !self
                        .links
                        .iter()
                        .any(|l| l.consumed && l.target == link.target)
                {
                    if let Some(source) = self.source(link) {
                        if let Some(value) = params.get(&link.target.3) {
                            fixed.insert(link.target.3.clone(), value.clone());
                        }
                        params.insert(link.target.3.clone(), json!({"bind": source}));
                        self.links[i].consumed = true;
                    }
                }
            }
            if !params.is_empty() {
                entry.insert("params".into(), Value::Object(params));
            }
            if !fixed.is_empty() {
                entry.insert("fixed".into(), Value::Object(fixed));
            }
            data.insert(call.alias.clone(), Value::Object(entry));
        }
        (data, events)
    }

    /// What a dynamic source's entity slot is bound from, as the text of its address.
    fn entity_link(&self, section: &str) -> Option<(usize, String)> {
        let found = self.links.iter().position(|l| {
            !l.consumed
                && l.target.0 == "Data"
                && l.target.2 == section
                && matches!(l.target.1.as_str(), "EntityName" | "Entity")
        })?;
        Some((found, self.source(&self.links[found])?))
    }

    fn remaining_bindings(&mut self) -> Vec<Value> {
        let mut bindings = vec![];
        for i in 0..self.links.len() {
            let link = &self.links[i];
            if link.consumed {
                continue;
            }
            let Some(from) = self.source(link) else {
                continue;
            };
            let (area, id, section, property) = &link.target;
            let root = self.content["UI"]["Properties"]["Id"]
                .as_str()
                .unwrap_or("");
            let to = match area.as_str() {
                "UI" if self.ids.contains_key(id) => format!("@@{id}@@.{property}"),
                // The mashup root is no node; `page.<property>` names its properties.
                "UI" if id == root && root == "mashup-root" => {
                    format!("page.{property}")
                }
                "Data" => {
                    let Some(call) = self
                        .calls
                        .iter()
                        .find(|c| c.name == *id && c.section == *section)
                    else {
                        continue;
                    };
                    format!("{}.{property}", call.alias)
                }
                "Mashup" if self.parameters.contains(property) => format!("param.{property}"),
                _ => continue,
            };
            bindings.push(json!({"from": from, "to": to}));
            self.links[i].consumed = true;
        }
        bindings
    }

    /// Replace `@@id@@` with the spec id of the widget; drop what refers to a widget that is not
    /// a node of its own.
    fn resolve(&mut self, spec: &mut Value) {
        let mut dropped = self.dropped;
        let mut ids = self.ids.clone();
        // A navigation function is folded into `navigate:` actions: nothing can refer to it.
        for folded in self.navigations.keys() {
            ids.remove(folded);
        }
        fn fix(text: &str, ids: &HashMap<String, String>) -> Option<String> {
            let mut out = String::new();
            let mut rest = text;
            while let Some(start) = rest.find("@@") {
                out.push_str(&rest[..start]);
                let after = &rest[start + 2..];
                let end = after.find("@@")?;
                out.push_str(ids.get(&after[..end])?);
                rest = &after[end + 2..];
            }
            out.push_str(rest);
            Some(out)
        }
        fn walk(value: &mut Value, ids: &HashMap<String, String>, dropped: &mut usize) {
            match value {
                Value::Object(map) => {
                    let keys: Vec<String> = map.keys().cloned().collect();
                    for key in keys {
                        let remove = match map.get_mut(&key) {
                            Some(Value::String(s)) if s.contains("@@") => match fix(s, ids) {
                                Some(fixed) => {
                                    *s = fixed;
                                    false
                                }
                                None => true,
                            },
                            Some(Value::Object(inner))
                                if inner.len() == 1
                                    && inner
                                        .get("bind")
                                        .and_then(Value::as_str)
                                        .is_some_and(|b| b.contains("@@")) =>
                            {
                                match fix(inner["bind"].as_str().expect("bind"), ids) {
                                    Some(fixed) => {
                                        inner.insert("bind".into(), json!(fixed));
                                        false
                                    }
                                    None => true,
                                }
                            }
                            // A route is positional: one that names a folded widget is an
                            // unbound input, not a missing entry.
                            Some(Value::Array(routes)) if key == "routes" => {
                                for route in routes.iter_mut() {
                                    let fixed = route
                                        .get("bind")
                                        .and_then(Value::as_str)
                                        .map(|b| fix(b, ids));
                                    match fixed {
                                        Some(Some(fixed)) => *route = json!({"bind": fixed}),
                                        Some(None) => {
                                            *route = Value::Null;
                                            *dropped += 1;
                                        }
                                        None => {}
                                    }
                                }
                                false
                            }
                            Some(other) => {
                                walk(other, ids, dropped);
                                false
                            }
                            None => false,
                        };
                        if remove {
                            map.remove(&key);
                            *dropped += 1;
                        }
                    }
                }
                Value::Array(items) => {
                    for item in items.iter_mut() {
                        walk(item, ids, dropped);
                    }
                    items.retain(|item| {
                        if item.is_null() {
                            return false;
                        }
                        let unresolved = item.as_object().is_some_and(|m| {
                            m.contains_key("on")
                                != (m.contains_key("invoke")
                                    || m.contains_key("navigate")
                                    || m.contains_key("show"))
                                || m.contains_key("from") != m.contains_key("to")
                        });
                        if unresolved {
                            *dropped += 1;
                        }
                        !unresolved
                    });
                }
                _ => {}
            }
        }
        walk(spec, &ids, &mut dropped);
        for (widget, property) in std::mem::take(&mut self.unknown) {
            self.notes.push(format!(
                "property left out: {widget} {property} (not in the widget catalogue)"
            ));
        }
        let unused_links: Vec<_> = self.links.iter().filter(|link| !link.consumed).collect();
        for link in &unused_links {
            self.notes.push(format!(
                "unsupported wiring left out: {}:{}:{} -> {}:{}:{}:{}",
                link.source.0,
                link.source.1,
                link.source.2.replace('\u{1}', "."),
                link.target.0,
                link.target.1,
                link.target.2,
                link.target.3
            ));
        }
        let unused = unused_links.len();
        if dropped > 0 || unused > 0 {
            self.notes.push(format!(
                "{} bindings or events could not be written as elements and were left out",
                dropped.max(unused)
            ));
        }
        self.dropped = dropped;
    }
}

fn plain(text: &str) -> bool {
    let reserved = matches!(
        text.to_ascii_lowercase().as_str(),
        "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
    );
    plain_text(text, reserved)
}

fn plain_text(text: &str, reserved: bool) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && !reserved
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " _-.".contains(c))
        && !text.ends_with(' ')
        && !text.contains("  ")
}

fn key(text: &str) -> String {
    if plain_text(text, false) {
        text.to_string()
    } else {
        serde_json::to_string(text).expect("string")
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) if plain(s) => s.clone(),
        Value::String(s) => serde_json::to_string(s).expect("string"),
        other => other.to_string(),
    }
}

fn flow(value: &Value) -> String {
    match value {
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", key(k), flow(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(flow).collect::<Vec<_>>().join(", ")
        ),
        other => scalar(other),
    }
}

fn simple(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.len() <= 6
                && map
                    .values()
                    .all(|v| !matches!(v, Value::Array(a) if a.iter().any(|i| i.is_object())))
        }
        Value::Array(items) => items.iter().all(|i| !i.is_object() || simple(i)),
        _ => true,
    }
}

fn node_like(map: &Map<String, Value>) -> bool {
    map.len() == 1 && map.values().all(Value::is_object)
}

/// Block-style YAML, with short leaves in flow style so a node stays on one line.
fn block(value: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                out.push_str(&pad);
                out.push_str(&self::key(key));
                out.push(':');
                inline_or_block(value, indent, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                let mut inner = String::new();
                match item {
                    Value::Object(map) if node_like(map) && !flowable(item) => {
                        block(item, indent + 2, &mut inner)
                    }
                    Value::Object(_) if !flowable(item) => block(item, indent + 2, &mut inner),
                    _ => {
                        inner.push_str(&" ".repeat(indent + 2));
                        inner.push_str(&flow(item));
                        inner.push('\n');
                    }
                }
                out.push_str(&pad);
                out.push_str("- ");
                out.push_str(&inner[indent + 2..]);
            }
        }
        other => {
            out.push_str(&pad);
            out.push_str(&scalar(other));
            out.push('\n');
        }
    }
}

fn flowable(value: &Value) -> bool {
    simple(value) && flow(value).len() <= 96
}

fn inline_or_block(value: &Value, indent: usize, out: &mut String) {
    match value {
        Value::Object(map) if map.is_empty() => out.push_str(" {}\n"),
        Value::Array(items) if items.is_empty() => out.push_str(" []\n"),
        Value::Object(_) | Value::Array(_) if flowable(value) => {
            out.push(' ');
            out.push_str(&flow(value));
            out.push('\n');
        }
        Value::Object(_) => {
            out.push('\n');
            block(value, indent + 2, out);
        }
        Value::Array(_) => {
            out.push('\n');
            block(value, indent + 2, out);
        }
        other => {
            out.push(' ');
            out.push_str(&scalar(other));
            out.push('\n');
        }
    }
}
