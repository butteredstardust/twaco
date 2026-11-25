//! The layout rule table: the layout decisions of the embedded templates, as data.
//!
//! A rule set is an ordered list of rules, each with a stable `id`, a typed `when` predicate and
//! `decisions`. A decision is looked up on its own: the first rule that matches the context and
//! makes that decision wins, so a rule that sets a size does not hide a later rule that sets the
//! gutter. There is no expression language: every predicate field and decision is a typed value,
//! and an unknown key is refused.
//!
//! A template file (`templates/layouts/*.json`) is the skeleton: slots, nesting, widgets, the
//! direction of each container. A property the rules decide stands in it as `"@rule"`, which keeps
//! its position, so the resolved template serialises exactly as one that wrote the value itself.
//! Unversioned mashups follow `legacy`; ordinary new mashups pin `v1` by default.
//! Edits keep the document's stored rule set.
//!
//! ```json
//! {"version": "legacy", "rules": [
//!   {"id": "size.toolbar-50", "when": {"template": "list-page", "slot": "toolbar"},
//!    "decisions": {"size": {"fixed": "50px"}}}
//! ]}
//! ```

use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;

pub(crate) mod context;
pub(crate) mod resolve;

const LEGACY: &str = include_str!("legacy.json");
const V1: &str = include_str!("v1.json");
const PLACEHOLDER: &str = "@rule";

/// Which embedded rule set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Version {
    Legacy,
    /// Page specs and ordinary new command mashups by default.
    V1,
}

impl Version {
    const fn name(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::V1 => "v1",
        }
    }
}

/// What a rule applies to.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Element {
    /// The template's root container.
    Page,
    /// A named slot container.
    Slot,
    /// A one-pixel rule between slots.
    Divider,
    /// The widget a slot holds, or a widget in a document.
    Widget,
    /// A container in a document that no template names.
    Container,
    /// A row of a column twaco lays out.
    Row,
    /// A child of a row whose siblings are decided together.
    Member,
    /// The tab container a contained mashup sits in.
    Tab,
}

/// The closed vocabulary of `facts`: what a document node can be asked about. A rule naming
/// another fact is refused.
pub(crate) const FACTS: &[&str] = &[
    "authored",
    "measured",
    "row-of-panels",
    "mixed-label-controls",
    "in-tab",
    "in-filter-footer",
    "narrow-column",
    "size-fixed",
    "style-card",
    "fit-content",
    "compact",
    "fills",
    "column-fills",
];

/// One name or a list of names.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
enum Names {
    One(String),
    Many(Vec<String>),
}

impl Names {
    fn contains(&self, name: &str) -> bool {
        match self {
            Self::One(one) => one == name,
            Self::Many(many) => many.iter().any(|candidate| candidate == name),
        }
    }

    fn is_blank(&self) -> bool {
        match self {
            Self::One(one) => one.is_empty(),
            Self::Many(many) => many.is_empty() || many.iter().any(String::is_empty),
        }
    }
}

/// The predicate of a rule: every field given must match.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct When {
    template: Option<Names>,
    element: Option<Element>,
    slot: Option<Names>,
    /// The enclosing slot's name.
    parent: Option<Names>,
    /// The widget type the slot holds.
    widget: Option<Names>,
    /// The operation deciding: a rule with an `op` applies only to it, one without only outside.
    op: Option<Names>,
    /// The label the widget is given.
    label: Option<Names>,
    /// Facts that must all hold.
    facts: Option<Vec<String>>,
    /// Facts that must all be absent.
    without: Option<Vec<String>>,
}

/// A container's own look: the colour it draws.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Surface {
    background: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum SizeWord {
    Fill,
    Content,
}

/// How a container is sized along its parent's main axis.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
enum Size {
    /// `"fill"` grows like `{"fill": 1}`; `"content"` does not grow.
    Word(SizeWord),
    /// Grows with this weight.
    Fill { fill: u32 },
    /// A fixed length such as `50px`.
    Fixed { fixed: String },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Siblings {
    /// Siblings share their parent's main axis equally.
    Equal,
    /// Each sibling takes its content's size.
    Content,
}

/// The decisions a rule makes. Each is optional; a rule makes at least one.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Decisions {
    surface: Option<Surface>,
    size: Option<Size>,
    /// The container's `Margin`; the empty string is none.
    gutter: Option<String>,
    /// `align-items`.
    align: Option<String>,
    /// `justify-content`.
    justify: Option<String>,
    /// Composer properties no typed decision covers, each decided on its own. Keys are listed in
    /// write order: the last matching rule that names a key sets its place, the first its value.
    props: Option<Map<String, Value>>,
    /// Whether the children of a row are equal or content-sized.
    siblings: Option<Siblings>,
    /// The properties each child of a row, or each control of a control row, takes.
    member_props: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Rule {
    id: String,
    #[serde(default)]
    when: When,
    decisions: Decisions,
}

/// Where a decision is being made.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Context<'a> {
    pub(crate) template: &'a str,
    pub(crate) element: Element,
    pub(crate) slot: Option<&'a str>,
    pub(crate) parent: Option<&'a str>,
    pub(crate) widget: Option<&'a str>,
    /// The operation deciding, or none for a template skeleton and `why`.
    pub(crate) op: Option<&'a str>,
    pub(crate) label: Option<&'a str>,
    pub(crate) facts: &'a [&'static str],
}

impl Context<'_> {
    fn describe(&self) -> String {
        format!(
            "{} {:?} {}",
            self.template,
            self.element,
            self.slot.unwrap_or("-")
        )
    }
}

impl When {
    fn matches(&self, context: &Context) -> bool {
        fn names(wanted: &Option<Names>, value: Option<&str>) -> bool {
            wanted
                .as_ref()
                .is_none_or(|names| value.is_some_and(|value| names.contains(value)))
        }
        names(&self.template, Some(context.template))
            && self
                .element
                .is_none_or(|element| element == context.element)
            && names(&self.slot, context.slot)
            && names(&self.parent, context.parent)
            && names(&self.widget, context.widget)
            && self.op.is_some() == context.op.is_some()
            && names(&self.op, context.op)
            && names(&self.label, context.label)
            && self
                .facts
                .iter()
                .flatten()
                .all(|fact| context.facts.contains(&fact.as_str()))
            && !self
                .without
                .iter()
                .flatten()
                .any(|fact| context.facts.contains(&fact.as_str()))
    }
}

/// A validated, ordered rule set.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RuleSet {
    version: String,
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    version: String,
    rules: Vec<Rule>,
}

impl RuleSet {
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    /// Parse and validate a rule file: unknown keys, an empty or repeated id, a rule that decides
    /// nothing, and an empty name list are refused.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let file: RuleFile =
            serde_json::from_str(text).map_err(|error| format!("layout rules: {error}"))?;
        if !matches!(file.version.as_str(), "legacy" | "v1") {
            return Err(format!(
                "layout rules: version `{}` is not legacy or v1",
                file.version
            ));
        }
        let mut ids = BTreeSet::new();
        for rule in &file.rules {
            let id = &rule.id;
            if id.is_empty() {
                return Err("layout rules: a rule has an empty id".into());
            }
            if !ids.insert(id.as_str()) {
                return Err(format!("layout rules: rule id `{id}` appears twice"));
            }
            let decisions = &rule.decisions;
            if decisions.surface.is_none()
                && decisions.size.is_none()
                && decisions.gutter.is_none()
                && decisions.align.is_none()
                && decisions.justify.is_none()
                && decisions.siblings.is_none()
                && decisions.props.as_ref().is_none_or(Map::is_empty)
                && decisions.member_props.as_ref().is_none_or(Map::is_empty)
            {
                return Err(format!("layout rules: rule `{id}` makes no decision"));
            }
            let when = &rule.when;
            if [&when.template, &when.slot, &when.parent, &when.widget]
                .into_iter()
                .flatten()
                .any(Names::is_blank)
            {
                return Err(format!("layout rules: rule `{id}` has an empty name"));
            }
            for fact in when.facts.iter().chain(&when.without).flatten() {
                if !FACTS.contains(&fact.as_str()) {
                    return Err(format!(
                        "layout rules: rule `{id}` names unknown fact `{fact}`"
                    ));
                }
            }
            if let Some(Size::Fixed { fixed }) = &decisions.size {
                if fixed.is_empty() {
                    return Err(format!("layout rules: rule `{id}` has an empty fixed size"));
                }
            }
        }
        Ok(Self {
            version: file.version,
            rules: file.rules,
        })
    }

    /// The embedded rule set for a version.
    pub(crate) fn embedded(version: Version) -> &'static Self {
        static LEGACY_SET: OnceLock<RuleSet> = OnceLock::new();
        static V1_SET: OnceLock<RuleSet> = OnceLock::new();
        let (cell, text) = match version {
            Version::Legacy => (&LEGACY_SET, LEGACY),
            Version::V1 => (&V1_SET, V1),
        };
        cell.get_or_init(|| {
            let set =
                Self::parse(text).expect("the embedded layout rules are valid; a test checks them");
            assert_eq!(set.version, version.name());
            set
        })
    }

    /// The rule ids in order.
    #[cfg(test)]
    fn ids(&self) -> Vec<&str> {
        self.rules.iter().map(|rule| rule.id.as_str()).collect()
    }

    /// The first rule that matches and makes a decision, with that decision.
    fn decide<'a, T>(
        &'a self,
        context: &Context,
        pick: impl Fn(&'a Decisions) -> Option<T>,
    ) -> Option<(&'a str, T)> {
        self.rules
            .iter()
            .filter(|rule| rule.when.matches(context))
            .find_map(|rule| pick(&rule.decisions).map(|value| (rule.id.as_str(), value)))
    }

    /// Fill every `"@rule"` of a template skeleton from the rules, keeping each key where it is.
    pub(crate) fn resolve(&self, template: &mut Value) -> Result<(), String> {
        let name = template["name"].as_str().unwrap_or_default().to_string();
        if let Some(page) = template.get_mut("page").and_then(Value::as_object_mut) {
            let context = Context {
                template: &name,
                element: Element::Page,
                slot: None,
                parent: None,
                widget: None,
                op: None,
                label: None,
                facts: &[],
            };
            self.resolve_properties(page, &context)?;
        }
        if let Some(children) = template.get_mut("children").and_then(Value::as_array_mut) {
            for child in children {
                self.resolve_node(child, &name, None)?;
            }
        }
        Ok(())
    }

    fn resolve_node(
        &self,
        node: &mut Value,
        template: &str,
        parent: Option<&str>,
    ) -> Result<(), String> {
        let object = node
            .as_object_mut()
            .ok_or_else(|| format!("layout template {template}: a node is not an object"))?;
        let divider = object.get("divider").and_then(Value::as_bool) == Some(true);
        let slot = object
            .get("slot")
            .and_then(Value::as_str)
            .map(str::to_string);
        let widget = object
            .get("widget")
            .and_then(Value::as_str)
            .map(str::to_string);
        let context = Context {
            template,
            element: if divider {
                Element::Divider
            } else {
                Element::Slot
            },
            slot: slot.as_deref(),
            parent,
            widget: None,
            op: None,
            label: None,
            facts: &[],
        };
        let held = Context {
            element: Element::Widget,
            widget: widget.as_deref(),
            ..context
        };
        self.resolve_properties_in(object, &context, &held)?;
        if let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) {
            for child in children {
                self.resolve_node(child, template, slot.as_deref())?;
            }
        }
        Ok(())
    }

    fn resolve_properties(
        &self,
        object: &mut Map<String, Value>,
        context: &Context,
    ) -> Result<(), String> {
        self.resolve_properties_in(object, context, context)
    }

    fn resolve_properties_in(
        &self,
        object: &mut Map<String, Value>,
        context: &Context,
        held: &Context,
    ) -> Result<(), String> {
        let keys: Vec<String> = object
            .iter()
            .filter(|(_, value)| value.as_str() == Some(PLACEHOLDER))
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            let value = self.value_for(&key, context, held)?;
            object.insert(key, value);
        }
        Ok(())
    }

    fn value_for(&self, key: &str, context: &Context, held: &Context) -> Result<Value, String> {
        let missing = || {
            format!(
                "layout rules {}: no decision for `{key}` on {}",
                self.version,
                context.describe()
            )
        };
        let fixed = || match self.decide(context, |d| d.size.as_ref()) {
            Some((_, Size::Fixed { fixed })) => Ok(json!(fixed)),
            Some(_) => Err(format!(
                "layout rules {}: `{key}` on {} needs a fixed size",
                self.version,
                context.describe()
            )),
            None => Err(missing()),
        };
        match key {
            "fixed" | "thickness" => fixed(),
            "style" => self
                .decide(context, |d| d.surface.as_ref())
                .map(|(_, surface)| json!({ "background": surface.background }))
                .ok_or_else(missing),
            "widget_gutter" => self
                .decide(held, |d| d.gutter.as_ref())
                .map(|(_, gutter)| json!(gutter))
                .ok_or_else(missing),
            "align-items" => self
                .decide(context, |d| d.align.as_ref())
                .map(|(_, align)| json!(align))
                .ok_or_else(missing),
            "justify-content" => self
                .decide(context, |d| d.justify.as_ref())
                .map(|(_, justify)| json!(justify))
                .ok_or_else(missing),
            "Margin" => self
                .decide(context, |d| d.gutter.as_ref())
                .map(|(_, gutter)| json!(gutter))
                .ok_or_else(missing),
            "flex-grow" => match self.decide(context, |d| d.size.as_ref()) {
                Some((_, Size::Word(SizeWord::Fill))) => Ok(json!(1)),
                Some((_, Size::Word(SizeWord::Content))) => Ok(json!(0)),
                Some((_, Size::Fill { fill })) => Ok(json!(fill)),
                Some((_, Size::Fixed { .. })) => Err(format!(
                    "layout rules {}: `flex-grow` on {} is decided as fixed",
                    self.version,
                    context.describe()
                )),
                None => Err(missing()),
            },
            _ => self
                .decide(context, |d| {
                    d.props.as_ref().and_then(|props| props.get(key))
                })
                .map(|(_, value)| value.clone())
                .ok_or_else(missing),
        }
    }
}

pub(crate) fn height_floor(widget: &serde_json::Value) -> u32 {
    let props = &widget["Properties"];
    let minimum = props["flex-min-height"]
        .as_str()
        .and_then(|s| s.strip_suffix("px"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let heights = widget["Widgets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(height_floor);
    let content = if props["flex-direction"] == "row" || props["Type"] == "ptcstabset" {
        heights.max().unwrap_or(0)
    } else {
        heights.fold(0u32, u32::saturating_add)
    };
    minimum.max(content)
}

pub(crate) fn patch(document: &mut super::edit::MashupDocument, path: &[usize], op: &'static str) {
    let node = context::Node::op(op, Element::Container);
    for (key, value) in document.rules().evaluate(&node.context()).patch() {
        document.set_property(path, key, value);
    }
}

pub(crate) fn clear_page_fillers(document: &mut super::edit::MashupDocument) {
    fn clear(node: &mut Value) {
        if let Some(children) = node["Widgets"].as_array_mut() {
            children.retain(|child| child["twacoLayoutFiller"] != true);
            for child in children {
                clear(child);
            }
        }
    }
    if document.value()["twacoPageRules"] == "v1" {
        let ids: Vec<_> = document
            .widgets()
            .into_iter()
            .filter(|w| document.at(&w.path)["twacoLayoutFiller"] == true)
            .map(|w| w.id)
            .collect();
        if let Some(styles) = document
            .root_properties_mut()
            .get_mut("StyleProperties")
            .and_then(|s| s["data"].as_object_mut())
        {
            for id in ids {
                styles.remove(&id);
            }
        }
        clear(&mut document.value_mut()["UI"]);
    }
}

fn side_columns(document: &mut super::edit::MashupDocument) {
    let shells: Vec<_> = document
        .widgets()
        .into_iter()
        .filter(|w| {
            let node = document.at(&w.path);
            node["Properties"]["flex-direction"] == "row"
                && super::edit::children(node).first().is_some_and(|first| {
                    first["twacoSidePanel"] == true
                        || first["twacoTemplateName"] == "filter-panel"
                        || first["Properties"]["DisplayName"] == "filter-column"
                })
                && !(1..w.path.len())
                    .any(|depth| document.at(&w.path[..depth])["twacoSidePanel"] == true)
        })
        .collect();
    for shell in shells {
        for depth in 1..=shell.path.len() {
            let path = &shell.path[..depth];
            let node = document.at(path);
            if node["Properties"]["Margin"] == "24" {
                document.set_property(path, "Margin", json!(""));
            }
        }
        let children = super::edit::children(document.at(&shell.path)).len();
        for index in 0..children {
            let path = [shell.path.as_slice(), &[index]].concat();
            let node = document.at(&path);
            let side = node["twacoSidePanel"] == true
                || node["twacoTemplateName"] == "filter-panel"
                || node["Properties"]["DisplayName"] == "filter-column";
            if side {
                document.set_property(&path, "Margin", json!(""));
                document.set_property(&path, "align-self", json!("stretch"));
            } else if !node["Properties"]["DisplayName"]
                .as_str()
                .unwrap_or("")
                .starts_with("divider-")
                && node["Properties"]["Margin"]
                    .as_str()
                    .is_none_or(str::is_empty)
            {
                document.set_property(&path, "Margin", json!("24"));
            }
        }
    }
}

pub(crate) fn finish_page(document: &mut super::edit::MashupDocument) {
    if document.value()["twacoPageRules"] != "v1" {
        return;
    }
    clear_page_fillers(document);
    side_columns(document);

    let fields: Vec<_> = document
        .widgets()
        .into_iter()
        .filter(|w| w.type_id == "ptcstextfield" && context::narrow_column(document, &w.path))
        .collect();
    for field in fields {
        let width = (1..field.path.len()).rev().find_map(|depth| {
            document.at(&field.path[..depth])["Properties"]["flex-max-width"]
                .as_str()?
                .strip_suffix("px")?
                .parse::<u32>()
                .ok()
        });
        if let Some(width) = width {
            let props = &document.at(&field.path)["Properties"];
            let margins: Vec<u32> = props["Margin"]
                .as_str()
                .unwrap_or("")
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            let horizontal = match margins.as_slice() {
                [all] | [_, all] | [_, all, _] => all.saturating_mul(2),
                [_, right, _, left] => right.saturating_add(*left),
                _ => 0,
            };
            let available = width.saturating_sub(horizontal).min(240);
            let authored = props["Width"].as_u64().unwrap_or(u64::from(available));
            document.set_property(
                &field.path,
                "Width",
                json!(authored.min(u64::from(available))),
            );
        }
    }
    let mut containers: Vec<_> = document
        .widgets()
        .into_iter()
        .filter(|w| w.type_id == "flexcontainer")
        .collect();
    containers.sort_by_key(|w| std::cmp::Reverse(w.path.len()));
    for container in &containers {
        let floor = height_floor(document.at(&container.path));
        let fixed = document.at(&container.path)["Properties"]["flex-size"] == "fixed";
        patch(document, &container.path, "page-overflow");
        if !fixed && floor > 0 {
            document.set_property(
                &container.path,
                "flex-min-height",
                json!(format!("{floor}px")),
            );
        }
        match container.display_name.as_str() {
            "filters" => patch(document, &container.path, "page-filter-fields"),
            "filter-footer-area" => {
                document.set_property(&container.path, "flex-min-height", json!(""));
                document.set_property(&container.path, "flex-max-height", json!(""));
            }
            "filter-footer" => patch(document, &container.path, "page-filter-footer"),
            "filter-column" => {
                // Runtime parses flex dimensions as numbers, so viewport bounds need CSS.
                let gutter: f64 = (1..container.path.len())
                    .map(|depth| {
                        let props = &document.at(&container.path[..depth])["Properties"];
                        let values: Vec<f64> = props["Margin"]
                            .as_str()
                            .unwrap_or("")
                            .split_whitespace()
                            .filter_map(|v| v.parse().ok())
                            .collect();
                        match values.as_slice() {
                            [all] | [all, _] => all * 2.0,
                            [top, _, bottom, ..] => top + bottom,
                            _ => 0.0,
                        }
                    })
                    .sum();
                let limit = if gutter > 0.0 {
                    format!("calc(100vh - {gutter}px)")
                } else {
                    "100vh".into()
                };
                let styles = document
                    .root_properties_mut()
                    .entry("StyleProperties")
                    .or_insert_with(
                        || json!({"meta":{"type":"WidgetStyler","version":"1.0"},"data":{}}),
                    );
                let entry = &mut styles["data"][&container.id];
                if entry.is_null() {
                    *entry = json!({"widget":"flex-container","parts":{"$":{"$":{}}}});
                }
                entry["parts"]["$"]["$"]["max-height"] = json!(limit);
                document.set_property(&container.path, "flex-min-height", json!("0px"));
            }
            _ => {}
        }
    }
    // The page container's floor must cover what its rows really hold: a row's own minimum
    // leaves out its margins, padding and the measured height of its widgets, and rows that
    // end up short make the page container scroll inside the mashup root's own scroller.
    let styles = document
        .root_properties()
        .get("StyleProperties")
        .map_or(Value::Null, |styles| styles["data"].clone());
    let page_container = page_container_id(document);
    for container in &containers {
        let node = document.at(&container.path);
        if page_container.as_ref().map(|(id, _)| id) != Some(&container.id)
            || node["Properties"]["flex-size"] == "fixed"
            || node["twacoLayoutAuthored"] == true
        {
            continue;
        }
        let content = super::edit::content_floor(node, &styles);
        let current = height_floor(node);
        let id = node["Properties"]["Id"].as_str().map(str::to_string);
        if content > f64::from(current) {
            document.set_property(
                &container.path,
                "flex-min-height",
                json!(format!("{}px", content.ceil())),
            );
        }
        // Runtime gives every flex container `overflow: auto`, so a page container whose floor
        // falls short of what its rows render scrolls inside the mashup root's scroller (two
        // scrollbars at short viewports). The page scrolls once, in the mashup root.
        // A master's pages render in its page host, which scrolls on its own.
        // A tile or card template's main slot (a cell) stays clipped to its cell.
        let master = document.value()["mashupType"] == "targetmashup";
        let page = page_container.as_ref().is_some_and(|(_, page)| *page);
        if let Some(id) = id.filter(|_| !master && page) {
            document.set_style(&id, "flex-container", "overflow", json!("visible"));
        }
    }
    // AdminPage01 grid/content and Compare tab content use LastContainer for the remainder.
    for container in containers {
        let node = document.at(&container.path);
        let p = &node["Properties"];
        if node["twacoLayoutAuthored"] == true {
            continue;
        }
        let floored = p["flex-min-height"]
            .as_str()
            .is_some_and(|min| min.ends_with("px") && min != "0px");
        let holds_fill = super::edit::children(node).iter().any(|child| {
            matches!(
                child["Properties"]["Type"].as_str(),
                Some("ptcsgrid" | "ptcscollection" | "ptcstabset")
            )
        });
        let tab_content = container.path.len() > 1
            && document.at(&container.path[..container.path.len() - 1])["Properties"]["Type"]
                == "ptcstabset";
        if p["flex-grow"] == 1
            && p["flex-size"] != "fixed"
            && (holds_fill
                || tab_content
                || matches!(
                    container.display_name.as_str(),
                    "filter-column" | "filter-stack" | "content" | "center" | "value"
                ))
        {
            document.set_property(&container.path, "LastContainer", json!(true));
            document.set_property(&container.path, "flex-size", json!("default"));
            let free = matches!(
                container.display_name.as_str(),
                "filter-column" | "filter-stack"
            );
            if holds_fill || tab_content || free {
                // A fill row keeps an authored or content floor (tall grids, charts) so the page
                // scrolls above it; only the filter columns drop theirs.
                if free || !floored {
                    document.set_property(&container.path, "flex-min-height", json!(""));
                }
                document.set_property(&container.path, "flex-max-height", json!(""));
            }
        }
    }
}

/// The template's content slot keeps its role when its display name changes. The blank
/// template uses its first visible root container; other templates declare their master slot.
/// Also says whether it is a page's own content area (the blank template's root or a `center`
/// slot) rather than the main slot of a tile or card template that cells are built from.
fn page_container_id(document: &super::edit::MashupDocument) -> Option<(String, bool)> {
    static SLOTS: OnceLock<std::collections::BTreeMap<String, (String, bool)>> = OnceLock::new();
    let root = document
        .widgets()
        .into_iter()
        .find(|w| w.path.len() == 1 && w.type_id == "flexcontainer")?;
    let node = document.at(&root.path);
    let Some(template) = node["twacoTemplateName"].as_str() else {
        return Some((root.id, true));
    };
    let slots = SLOTS.get_or_init(|| {
        super::templates::layouts()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|template| {
                let page = template.master_slot.as_deref() == Some("center");
                super::authoring::template_page_container(&template)
                    .map(|id| (template.name, (id, page)))
            })
            .collect()
    });
    let (slot, page) = slots.get(template)?;
    document.find_exact(slot).ok().map(|w| (w.id, *page))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str) -> String {
        format!(r#"{{"id":"{id}","decisions":{{"align":"center"}}}}"#)
    }

    fn file(version: &str, rules: &[String]) -> String {
        format!(r#"{{"version":"{version}","rules":[{}]}}"#, rules.join(","))
    }

    #[test]
    fn v1_changes_only_reviewed_page_rules() {
        let legacy = RuleSet::embedded(Version::Legacy);
        let v1 = RuleSet::embedded(Version::V1);
        for old in &legacy.rules {
            let new = v1.rules.iter().find(|rule| rule.id == old.id).unwrap();
            if ![
                "gutter.page-padded",
                "gutter.filter-footer",
                "size.filter-column",
                "props.filters-width",
                "props.filter-footer-width",
                "props.filter-rule-width",
                "props.toolbar-band",
                "props.card-recipe",
                "props.filter-footer-button",
            ]
            .contains(&old.id.as_str())
            {
                assert_eq!(old, new, "{}", old.id);
            } else {
                assert_ne!(old, new, "{}", old.id);
            }
        }
        let additions: Vec<_> = v1
            .rules
            .iter()
            .filter(|rule| !legacy.rules.iter().any(|old| old.id == rule.id))
            .collect();
        let native: Vec<_> = additions
            .iter()
            .filter(|r| r.id.starts_with("props.native-"))
            .collect();
        assert_eq!(native.len(), 9);
        for rule in native {
            assert!(matches!(&rule.when.op, Some(Names::One(op)) if op == "add"));
            let Some(Names::One(widget)) = &rule.when.widget else {
                panic!("native widget rule");
            };
            let defaults = &crate::core::widgets::catalog()
                .by_type(widget)
                .unwrap()
                .defaults;
            for (property, value) in rule.decisions.props.as_ref().unwrap() {
                assert_eq!(
                    defaults.get(property),
                    Some(value),
                    "{} {property}",
                    rule.id
                );
            }
        }
        let additions: Vec<_> = additions
            .into_iter()
            .filter(|r| !r.id.starts_with("props.native-") && !r.id.starts_with("props.chart-"))
            .collect();
        assert_eq!(additions.len(), 22);
        assert_eq!(additions[0].id, "gutter.filter-actions");
        for (rule, op) in additions[1..].iter().zip([
            "page-header",
            "page-kpi-row",
            "page-card",
            "page-card-caption",
            "page-control-row",
            "page-control-member",
            "page-date-range",
            "page-date-range-narrow",
            "page-chart-region",
            "page-content-height",
            "page-panel-width",
            "page-slider-wrapper",
            "page-fill",
            "page-chart-minimum",
            "page-chart-legend",
            "page-filter-fields",
            "page-filter-footer",
            "page-overflow",
            "page-filter-only",
            "sparkline-height",
            "page-spacer",
        ]) {
            assert!(matches!(&rule.when.op,Some(Names::One(name)) if name == op));
        }
        assert_eq!(legacy.version, "legacy");
        assert_eq!(v1.version, "v1");
    }

    #[test]
    fn embedded_rule_ids_are_stable_dotted_names() {
        for id in RuleSet::embedded(Version::Legacy).ids() {
            assert!(
                id.split('.').count() == 2
                    && id.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || b".-".contains(&byte)),
                "{id}"
            );
        }
    }

    #[test]
    fn a_duplicate_id_is_refused() {
        let text = file("legacy", &[rule("a.one"), rule("a.one")]);
        let error = RuleSet::parse(&text).unwrap_err();
        assert!(error.contains("`a.one` appears twice"), "{error}");
    }

    #[test]
    fn unknown_keys_are_refused_everywhere() {
        let bad = [
            r#"{"version":"legacy","rules":[],"extra":1}"#.to_string(),
            r#"{"version":"legacy","rules":[{"id":"a.b","decisions":{"align":"center"},"note":1}]}"#
                .to_string(),
            r#"{"version":"legacy","rules":[{"id":"a.b","when":{"colour":"red"},"decisions":{"align":"center"}}]}"#
                .to_string(),
            r#"{"version":"legacy","rules":[{"id":"a.b","decisions":{"align":"center","shade":"x"}}]}"#
                .to_string(),
            r##"{"version":"legacy","rules":[{"id":"a.b","decisions":{"surface":{"background":"#fff","border":"x"}}}]}"##
                .to_string(),
            r#"{"version":"legacy","rules":[{"id":"a.b","decisions":{"size":{"fixed":"1px","extra":1}}}]}"#
                .to_string(),
        ];
        for text in bad {
            assert!(RuleSet::parse(&text).is_err(), "{text}");
        }
    }

    #[test]
    fn empty_ids_names_decisions_and_unknown_versions_are_refused() {
        let cases = [
            (file("legacy", &[rule("")]), "empty id"),
            (file("v9", &[rule("a.b")]), "not legacy or v1"),
            (
                file("legacy", &[r#"{"id":"a.b","decisions":{}}"#.to_string()]),
                "makes no decision",
            ),
            (
                file(
                    "legacy",
                    &[
                        r#"{"id":"a.b","when":{"slot":[]},"decisions":{"align":"center"}}"#
                            .to_string(),
                    ],
                ),
                "empty name",
            ),
            (
                file(
                    "legacy",
                    &[r#"{"id":"a.b","decisions":{"size":{"fixed":""}}}"#.to_string()],
                ),
                "empty fixed size",
            ),
        ];
        for (text, wanted) in cases {
            let error = RuleSet::parse(&text).unwrap_err();
            assert!(error.contains(wanted), "{wanted}: {error}");
        }
    }

    #[test]
    fn each_decision_takes_its_first_matching_rule_on_its_own() {
        let text = file(
            "legacy",
            &[
                r#"{"id":"a.size","when":{"slot":"bar"},"decisions":{"size":{"fixed":"10px"}}}"#.into(),
                r#"{"id":"a.align","when":{"element":"slot"},"decisions":{"align":"center"}}"#.into(),
                r#"{"id":"a.size-later","when":{"element":"slot"},"decisions":{"size":"fill","align":"stretch"}}"#.into(),
            ],
        );
        let set = RuleSet::parse(&text).unwrap();
        let context = Context {
            template: "t",
            element: Element::Slot,
            slot: Some("bar"),
            parent: None,
            widget: None,
            op: None,
            label: None,
            facts: &[],
        };
        assert_eq!(
            set.decide(&context, |d| d.size.as_ref()).map(|(id, _)| id),
            Some("a.size")
        );
        assert_eq!(
            set.decide(&context, |d| d.align.as_ref()).map(|(id, _)| id),
            Some("a.align")
        );
        let other = Context {
            slot: Some("body"),
            ..context
        };
        assert_eq!(
            set.decide(&other, |d| d.size.as_ref()).map(|(id, _)| id),
            Some("a.size-later")
        );
    }

    #[test]
    fn resolving_fills_placeholders_in_place_and_names_a_missing_decision() {
        let set = RuleSet::parse(&file(
            "legacy",
            &[r#"{"id":"a.bar","when":{"template":"t","slot":"bar"},"decisions":{"size":{"fixed":"9px"},"align":"center"}}"#.into()],
        ))
        .unwrap();
        let mut template: Value = serde_json::from_str(
            r#"{"name":"t","page":{"flex-direction":"row"},"children":[
                {"slot":"bar","align-items":"@rule","fixed":"@rule","flex-shrink":0}]}"#,
        )
        .unwrap();
        set.resolve(&mut template).unwrap();
        assert_eq!(
            template["children"][0],
            json!({"slot":"bar","align-items":"center","fixed":"9px","flex-shrink":0})
        );
        let mut missing: Value = serde_json::from_str(
            r#"{"name":"t","page":{},"children":[{"slot":"other","align-items":"@rule"}]}"#,
        )
        .unwrap();
        let error = set.resolve(&mut missing).unwrap_err();
        assert!(error.contains("no decision for `align-items`"), "{error}");
    }
}
