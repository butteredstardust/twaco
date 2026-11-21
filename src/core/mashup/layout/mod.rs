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
//! `legacy` is what twaco has always produced and is what every existing mashup follows; `v1` starts
//! equal to it and is the set new authoring may improve.
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

const LEGACY: &str = include_str!("legacy.json");
const V1: &str = include_str!("v1.json");
const PLACEHOLDER: &str = "@rule";

/// Which embedded rule set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Version {
    Legacy,
    /// Not yet chosen by any command; authoring selects it once new mashups pin a rule set.
    #[allow(dead_code)]
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
    /// The widget a slot holds.
    Widget,
}

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
    /// Composer properties no typed decision covers, each decided on its own.
    props: Option<Map<String, Value>>,
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
                && decisions.props.as_ref().is_none_or(Map::is_empty)
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
    fn the_embedded_rule_sets_load_and_legacy_equals_v1_for_now() {
        let legacy = RuleSet::embedded(Version::Legacy);
        let v1 = RuleSet::embedded(Version::V1);
        assert_eq!(legacy.rules, v1.rules);
        assert_eq!(legacy.version, "legacy");
        assert_eq!(v1.version, "v1");
        assert!(!legacy.rules.is_empty());
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
