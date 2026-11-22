//! Derive a readable page spec from mashup content, offline.
//!
//! Widgets become elements where the structure is recognised and `widget:` nodes otherwise,
//! carrying only the properties that differ from the widget catalogue. Styles and custom CSS are
//! not carried: a spec describes structure and wiring, and the build supplies the look.

use super::Error;
use crate::core::widgets;
use serde_json::{json, Map, Value};
use serde_saphyr::Location;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug)]
pub(crate) struct Derived {
    pub(crate) spec: String,
    pub(crate) elements: usize,
    pub(crate) raw: usize,
    pub(crate) raw_types: BTreeMap<String, usize>,
    pub(crate) widgets: usize,
    pub(crate) notes: Vec<String>,
}

pub(crate) fn derive(name: &str, content: &str) -> Result<Derived, Error> {
    let content: Value =
        serde_json::from_str(content).map_err(|e| Error::at(Location::UNKNOWN, e))?;
    let root = content
        .get("UI")
        .ok_or_else(|| Error::at(Location::UNKNOWN, "mashup needs UI"))?;
    let mut run = Run::new(&content);
    let parameters = run.parameters(&root["Properties"]);
    let slot: Vec<Value> = widgets_of(root)
        .iter()
        .map(|w| run.node(w, "column"))
        .collect();
    let mut page = Map::new();
    page.insert("name".into(), json!(name));
    page.insert("template".into(), json!("blank"));
    let props = &root["Properties"];
    if let Some(theme) = props["StyleTheme"]
        .as_str()
        .filter(|t| !t.is_empty() && *t != "PTC Convergence Theme")
    {
        page.insert("theme".into(), json!(theme));
    }
    if !parameters.is_empty() {
        page.insert("parameters".into(), Value::Object(parameters));
    }
    let (data, events) = run.wiring();
    if !data.is_empty() {
        page.insert("data".into(), Value::Object(data));
    }
    page.insert("slots".into(), json!({"center": slot}));
    if !events.is_empty() {
        page.insert("events".into(), Value::Array(events));
    }
    let mut spec = json!({"version": 1, "rules": "v1", "page": page});
    run.resolve(&mut spec);
    run.notes.push(
        "styles, custom CSS and unsupported wiring are not carried; the build supplies the look"
            .into(),
    );
    let mut text = String::new();
    block(&spec, 0, &mut text);
    Ok(Derived {
        spec: text,
        elements: run.elements,
        raw: run.raw,
        raw_types: run.raw_types,
        widgets: run.widgets,
        notes: run.notes,
    })
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
fn known_property(widget: &Value, property: &str) -> bool {
    let type_id = kind(widget);
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
}

struct Link {
    source: (String, String, String),
    target: (String, String, String, String),
    consumed: bool,
}

struct Run<'a> {
    content: &'a Value,
    calls: Vec<Call>,
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
    unknown: BTreeSet<String>,
}

impl<'a> Run<'a> {
    fn new(content: &'a Value) -> Self {
        let mut run = Self {
            content,
            calls: vec![],
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
            let entity = entry["EntityName"].as_str().unwrap_or("");
            if entry["EntityType"] != "Things" || entity.is_empty() {
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
                out.insert(name.into(), json!(base));
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
            "Data" => {
                let call = self
                    .calls
                    .iter()
                    .find(|c| c.section == section && c.name == *id)?;
                Some(if field.is_empty() {
                    call.alias.clone()
                } else {
                    format!("{}.{field}", call.alias)
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
                && l.source.2.ends_with('\u{1}')
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
            if noise(key)
                || skip.contains(&key.as_str())
                || skip
                    .iter()
                    .any(|s| s.ends_with('*') && key.starts_with(&s[..s.len() - 1]))
            {
                continue;
            }
            if base.get(key).is_some_and(|d| same(d, value)) {
                continue;
            }
            if !known_property(widget, key) {
                self.unknown.insert(key.clone());
                continue;
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
            if !known_property(widget, &self.links[i].target.3) {
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
        match type_id.as_str() {
            "flexcontainer" => self.container(widget, parent),
            "ptcslabel" => self.label(widget),
            "ptcsbutton" => self.button(widget),
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
            "ptcsdropdown" | "ptcslist" => self.picker(widget),
            "ptcsgrid" => self.grid(widget),
            "ptcschartline" | "ptcschartbar" | "ptcschartpie" | "ptcschartcombo" => {
                self.chart(widget)
            }
            "ptcscollection" => self.collection(widget),
            "ptcstabset" => self.tabs(widget),
            _ => self.other(widget),
        }
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
        if !widgets_of(widget).is_empty() {
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
            props.insert("LabelText".into(), bound);
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

    fn picker(&mut self, widget: &Value) -> Value {
        let is_list = kind(widget) == "ptcslist";
        let display = text(widget, "DisplayField");
        let fed = display.as_ref().and_then(|_| self.feed(widget));
        let (Some(display), Some(data)) = (display, fed) else {
            return self.other(widget);
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
        let Some(data) = self.feed(widget) else {
            return self.other(widget);
        };
        let mut body = Map::new();
        body.insert("data".into(), json!(data));
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
        let props = self.props(widget, &["ColumnFormat", "Label"]);
        self.finish("grid", widget, body, props)
    }

    fn chart(&mut self, widget: &Value) -> Value {
        let x = text(widget, "XAxisField");
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
            let Some(value) = text(widget, "DataField") else {
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
        let cell = text(widget, "Mashup");
        let map = prop(widget, "MashupPropertyBinding")
            .as_object()
            .filter(|m| !m.is_empty())
            .cloned();
        let fed = cell
            .as_ref()
            .and(map.as_ref())
            .and_then(|_| self.feed(widget));
        let (Some(cell), Some(map), Some(data)) = (cell, map, fed) else {
            return self.other(widget);
        };
        let mut body = Map::new();
        body.insert("data".into(), json!(data));
        body.insert("mashup".into(), json!(cell));
        body.insert("map".into(), Value::Object(map));
        if let Some(empty) =
            text(widget, "NoDataText").filter(|t| t != "[[noCollectionDataToDisplay]]")
        {
            body.insert("empty".into(), json!(empty));
        }
        self.notes_once("collection layout properties are regenerated by the collection preset");
        let id = self.id(widget);
        self.make("collection", id, body)
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
            .map(|n| text(widget, &format!("Tab{n}Name")).unwrap_or_default())
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
                let content: Vec<Value> = widgets_of(tab)
                    .iter()
                    .map(|w| self.node(w, "column"))
                    .collect();
                json!({"label": label, "content": content})
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
        let found = self
            .side_panel(widget)
            .or_else(|| self.filter_panel(widget))
            .or_else(|| self.card(widget))
            .or_else(|| self.kpi_row(widget))
            .or_else(|| self.page_header(widget, direction))
            .or_else(|| self.toolbar(widget, direction));
        if let Some(found) = found {
            return found;
        }
        if is_spacer(widget) && parent == "row" {
            let id = self.id(widget);
            return self.make("spacer", id, Map::new());
        }
        let mut body = Map::new();
        let mut skip = vec!["flex-direction"];
        let grow = number(prop(widget, "flex-grow")).unwrap_or(1.0);
        if grow == 0.0 {
            let dimension = if parent == "row" { "width" } else { "height" };
            let min = pixels(prop(widget, &format!("flex-min-{dimension}")));
            let max = pixels(prop(widget, &format!("flex-max-{dimension}")));
            if min.is_some() && min == max {
                body.insert("size".into(), json!({"fixed": min}));
                skip.extend([
                    "flex-min-width",
                    "flex-min-height",
                    "flex-max-width",
                    "flex-max-height",
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
        let mut data = Map::new();
        let mut events = vec![];
        let mut loaded: BTreeSet<String> = BTreeSet::new();
        for event in self.content["Events"].as_array().into_iter().flatten() {
            let get = |k: &str| event[k].as_str().unwrap_or("");
            let trigger =
                if get("EventTriggerArea") == "Mashup" && get("EventTriggerEvent") == "Loaded" {
                    None
                } else if get("EventTriggerArea") == "UI" {
                    Some(format!(
                        "@@{}@@.{}",
                        get("EventTriggerId"),
                        get("EventTriggerEvent")
                    ))
                } else {
                    self.dropped += 1;
                    continue;
                };
            let target = match get("EventHandlerArea") {
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
            } else if call.refresh > 0 {
                entry.insert("on".into(), json!(format!("every {}s", call.refresh)));
            }
            let mut params = call.params.clone();
            for link in &self.links {
                if link.target.0 == "Data"
                    && link.target.1 == call.name
                    && link.target.2 == call.section
                {
                    if let Some(source) = self.source(link) {
                        params.insert(link.target.3.clone(), json!({"bind": source}));
                    }
                }
            }
            if !params.is_empty() {
                entry.insert("params".into(), Value::Object(params));
            }
            data.insert(call.alias.clone(), Value::Object(entry));
        }
        (data, events)
    }

    /// Replace `@@id@@` with the spec id of the widget; drop what refers to a widget that is not
    /// a node of its own.
    fn resolve(&mut self, spec: &mut Value) {
        let mut dropped = self.dropped;
        let ids = self.ids.clone();
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
                        let unresolved = item
                            .as_object()
                            .is_some_and(|m| m.contains_key("on") != m.contains_key("invoke"));
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
        if !self.unknown.is_empty() {
            let names: Vec<_> = self.unknown.iter().cloned().collect();
            self.notes.push(format!(
                "properties the widget catalogue does not know were left out: {}",
                names.join(", ")
            ));
        }
        let unused = self.links.iter().filter(|l| !l.consumed).count();
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
