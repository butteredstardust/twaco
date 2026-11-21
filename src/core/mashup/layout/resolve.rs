//! Evaluating the rule table for one node: every decision with the rule that made it.
//!
//! Template instantiation, authoring, validation and `mashup why` all ask here, so they cannot
//! disagree about what a node should be. An explicit authored setting (the `authored` fact, from
//! `twacoLayoutAuthored`) wins over the table: the geometry decisions (`size`, `siblings`, `props`)
//! are then not taken from a rule, and `why` reports them as authored.

use super::{Context, RuleSet, Siblings, Size, SizeWord};
use serde::Serialize;
use serde_json::Value;

/// A value and the rule that decided it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit<T> {
    pub(crate) rule: String,
    pub(crate) value: T,
}

/// Where a decision came from.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A rule of the embedded table.
    RuleTable,
    /// A setting someone made on the node, which the table does not override.
    Authored,
    /// The node was not laid out from a template: this is what the table would decide now.
    Inferred,
}

/// One decision, shaped for display.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub(crate) struct Decision {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) rule: Option<String>,
    pub(crate) source: Source,
}

/// What the table decides for one context.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Evaluation {
    pub(crate) authored: bool,
    /// twaco fitted the node's height to its content (`twacoLayoutHeight`): a fixed size from the
    /// table is where it started, not what it must stay.
    pub(crate) measured: bool,
    pub(crate) surface: Option<Hit<String>>,
    size: Option<Hit<Size>>,
    pub(crate) gutter: Option<Hit<String>>,
    pub(crate) align: Option<Hit<String>>,
    pub(crate) justify: Option<Hit<String>>,
    pub(crate) siblings: Option<Hit<Siblings>>,
    /// In write order.
    pub(crate) props: Vec<Hit<(String, Value)>>,
    pub(crate) member_props: Vec<Hit<(String, Value)>>,
}

impl Evaluation {
    /// `flex-grow` the size decision means, or `None` when it is fixed or undecided.
    pub(crate) fn grow(&self) -> Option<u32> {
        match &self.size.as_ref()?.value {
            Size::Word(SizeWord::Fill) => Some(1),
            Size::Word(SizeWord::Content) => Some(0),
            Size::Fill { fill } => Some(*fill),
            Size::Fixed { .. } => None,
        }
    }

    /// The decided property value, for a key.
    pub(crate) fn prop(&self, key: &str) -> Option<&Value> {
        self.props
            .iter()
            .find(|hit| hit.value.0 == key)
            .map(|hit| &hit.value.1)
    }

    /// The decided properties as a write list.
    pub(crate) fn patch(&self) -> Vec<(&str, Value)> {
        self.props
            .iter()
            .map(|hit| (hit.value.0.as_str(), hit.value.1.clone()))
            .collect()
    }

    /// The properties for each member, as a write list.
    pub(crate) fn member_patch(&self) -> Vec<(&str, Value)> {
        self.member_props
            .iter()
            .map(|hit| (hit.value.0.as_str(), hit.value.1.clone()))
            .collect()
    }

    /// Every decision made, in a fixed order, for display.
    pub(crate) fn decisions(&self) -> Vec<Decision> {
        let rule = |hit: &str| Decision {
            name: String::new(),
            value: String::new(),
            rule: Some(hit.to_string()),
            source: Source::RuleTable,
        };
        let mut out = Vec::new();
        let mut text = |name: &str, hit: Option<(&str, String)>| {
            if let Some((id, value)) = hit {
                out.push(Decision {
                    name: name.into(),
                    value,
                    ..rule(id)
                });
            }
        };
        text(
            "surface",
            self.surface
                .as_ref()
                .map(|hit| (hit.rule.as_str(), hit.value.clone())),
        );
        text(
            "size",
            self.size
                .as_ref()
                .map(|hit| (hit.rule.as_str(), size_text(&hit.value))),
        );
        text(
            "gutter",
            self.gutter.as_ref().map(|hit| {
                (
                    hit.rule.as_str(),
                    if hit.value.is_empty() {
                        "none".into()
                    } else {
                        hit.value.clone()
                    },
                )
            }),
        );
        text(
            "align",
            self.align
                .as_ref()
                .map(|hit| (hit.rule.as_str(), hit.value.clone())),
        );
        text(
            "justify",
            self.justify
                .as_ref()
                .map(|hit| (hit.rule.as_str(), hit.value.clone())),
        );
        text(
            "siblings",
            self.siblings.as_ref().map(|hit| {
                (
                    hit.rule.as_str(),
                    match hit.value {
                        Siblings::Equal => "equal",
                        Siblings::Content => "content",
                    }
                    .to_string(),
                )
            }),
        );
        for hit in &self.props {
            text(
                &format!("prop {}", hit.value.0),
                Some((
                    hit.rule.as_str(),
                    match &hit.value.1 {
                        Value::String(text) if text.is_empty() => "\"\"".to_string(),
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    },
                )),
            );
        }
        out
    }
}

fn size_text(size: &Size) -> String {
    match size {
        Size::Word(SizeWord::Fill) => "fill".into(),
        Size::Word(SizeWord::Content) => "content".into(),
        Size::Fill { fill } => format!("fill {fill}"),
        Size::Fixed { fixed } => format!("fixed {fixed}"),
    }
}

impl RuleSet {
    /// Every decision the table makes for a context. Each decision takes its first matching
    /// rule; an authored node takes no geometry from the table.
    pub(crate) fn evaluate(&self, context: &Context) -> Evaluation {
        let authored = context.facts.contains(&"authored");
        let hit = |found: Option<(&str, String)>| {
            found.map(|(rule, value)| Hit {
                rule: rule.into(),
                value,
            })
        };
        Evaluation {
            authored,
            measured: context.facts.contains(&"measured"),
            surface: hit(self
                .decide(context, |d| d.surface.as_ref())
                .map(|(rule, surface)| (rule, surface.background.clone()))),
            size: if authored {
                None
            } else {
                self.decide(context, |d| d.size.as_ref())
                    .map(|(rule, size)| Hit {
                        rule: rule.into(),
                        value: size.clone(),
                    })
            },
            gutter: hit(self
                .decide(context, |d| d.gutter.as_ref())
                .map(|(rule, value)| (rule, value.clone()))),
            align: hit(self
                .decide(context, |d| d.align.as_ref())
                .map(|(rule, value)| (rule, value.clone()))),
            justify: hit(self
                .decide(context, |d| d.justify.as_ref())
                .map(|(rule, value)| (rule, value.clone()))),
            siblings: if authored {
                None
            } else {
                self.decide(context, |d| d.siblings)
                    .map(|(rule, value)| Hit {
                        rule: rule.into(),
                        value,
                    })
            },
            props: if authored {
                Vec::new()
            } else {
                self.patch(context, |d| d.props.as_ref())
            },
            member_props: self.patch(context, |d| d.member_props.as_ref()),
        }
    }

    /// A property map decided key by key: a key's value comes from the first matching rule that
    /// names it, its place from the last, so a general rule lists the write order and a specific
    /// one in front of it only changes values.
    fn patch<'a>(
        &'a self,
        context: &Context,
        pick: impl Fn(&'a super::Decisions) -> Option<&'a serde_json::Map<String, Value>>,
    ) -> Vec<Hit<(String, Value)>> {
        let matching: Vec<&super::Rule> = self
            .rules
            .iter()
            .filter(|rule| rule.when.matches(context))
            .collect();
        let mut order: Vec<&str> = Vec::new();
        for rule in matching.iter().rev() {
            for key in pick(&rule.decisions).into_iter().flat_map(|map| map.keys()) {
                if !order.contains(&key.as_str()) {
                    order.push(key);
                }
            }
        }
        order
            .into_iter()
            .filter_map(|key| {
                matching.iter().find_map(|rule| {
                    pick(&rule.decisions)
                        .and_then(|map| map.get(key))
                        .map(|value| Hit {
                            rule: rule.id.clone(),
                            value: (key.to_string(), value.clone()),
                        })
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Element, RuleSet, Version};
    use super::*;
    use crate::core::mashup::layout::context::Node;
    use serde_json::json;

    fn legacy() -> &'static RuleSet {
        RuleSet::embedded(Version::Legacy)
    }

    fn row(op: &'static str, facts: &[&'static str]) -> Evaluation {
        legacy().evaluate(&Node::op(op, Element::Row).facts(facts).context())
    }

    #[test]
    fn natural_height_decides_every_combination_of_facts() {
        let cases: [(&[&str], u32, &str); 4] = [
            (&["fills", "compact"], 0, "size.row-compact"),
            (&["fills"], 1, "size.row-filler"),
            (&["column-fills"], 0, "size.row-beside-filler"),
            (&[], 1, "size.row-natural"),
        ];
        for (facts, grow, rule) in cases {
            let decided = row("natural-height", facts);
            assert_eq!(decided.grow(), Some(grow), "{facts:?}");
            assert_eq!(
                decided.size.as_ref().map(|hit| hit.rule.as_str()),
                Some(rule)
            );
            // The shrink comes from a different rule than the growth: each decision on its own.
            assert_eq!(decided.prop("flex-shrink"), Some(&json!(0)), "{facts:?}");
            assert_eq!(decided.props[0].rule, "size.row-natural");
        }
        let both = row("natural-height", &["fills", "column-fills"]);
        assert_eq!(
            both.grow(),
            Some(1),
            "the filler outranks a filling sibling"
        );
    }

    #[test]
    fn equal_siblings_name_their_rule_and_the_member_patch() {
        let equal = row("equal-siblings", &["row-of-panels"]);
        let hit = equal.siblings.as_ref().unwrap();
        assert_eq!(
            (hit.rule.as_str(), hit.value),
            ("siblings.panel-row", Siblings::Equal)
        );
        assert_eq!(
            equal.member_patch(),
            vec![
                ("flex-grow", json!(1)),
                ("flex-shrink", json!(1)),
                ("flex-basis", json!("0px"))
            ]
        );
        let content = row("equal-siblings", &[]);
        let hit = content.siblings.as_ref().unwrap();
        assert_eq!(
            (hit.rule.as_str(), hit.value),
            ("siblings.row-content", Siblings::Content)
        );
        assert!(content.member_props.is_empty());
    }

    #[test]
    fn an_explicit_authored_setting_wins_over_the_table() {
        let authored = row("equal-siblings", &["row-of-panels", "authored"]);
        assert!(authored.authored);
        assert!(authored.siblings.is_none());
        let natural = row("natural-height", &["authored"]);
        assert!(natural.grow().is_none() && natural.props.is_empty());
        let band = legacy().evaluate(
            &Node::op("toolbar-band", Element::Container)
                .fact("authored", true)
                .context(),
        );
        assert!(band.props.is_empty());
        // Decisions that are not geometry still come from the table.
        let template = legacy().evaluate(&Context {
            template: "list-page",
            element: Element::Slot,
            slot: Some("toolbar"),
            parent: None,
            widget: None,
            op: None,
            label: None,
            facts: &["authored"],
        });
        assert!(template.size.is_none());
        assert_eq!(template.align.unwrap().rule, "align.bar");
        assert_eq!(template.surface.unwrap().rule, "surface.toolbar-band");
    }

    #[test]
    fn an_operation_and_a_plain_context_never_share_rules() {
        let plain = legacy().evaluate(&Context {
            template: "",
            element: Element::Row,
            slot: None,
            parent: None,
            widget: None,
            op: None,
            label: None,
            facts: &["row-of-panels"],
        });
        assert!(plain.siblings.is_none() && plain.props.is_empty());
        let on_op = legacy().evaluate(&Node::op("add", Element::Slot).slot("toolbar").context());
        assert!(on_op.size.is_none() && on_op.align.is_none());
    }

    #[test]
    fn a_property_takes_its_value_from_the_first_rule_and_its_place_from_the_last() {
        let set = RuleSet::parse(
            r#"{"version":"legacy","rules":[
              {"id":"a.special","when":{"op":"x","facts":["style-card"]},"decisions":{"props":{"Margin":"","extra":1}}},
              {"id":"a.base","when":{"op":"x"},"decisions":{"props":{"Style":"s","Margin":"24","flex-grow":1}}}
            ]}"#,
        )
        .unwrap();
        let plain = set.evaluate(&Node::op("x", Element::Container).context());
        assert_eq!(
            plain.patch(),
            vec![
                ("Style", json!("s")),
                ("Margin", json!("24")),
                ("flex-grow", json!(1))
            ]
        );
        let card = set.evaluate(
            &Node::op("x", Element::Container)
                .fact("style-card", true)
                .context(),
        );
        assert_eq!(
            card.patch(),
            vec![
                ("Style", json!("s")),
                ("Margin", json!("")),
                ("flex-grow", json!(1)),
                ("extra", json!(1))
            ]
        );
        let rules: Vec<_> = card.props.iter().map(|hit| hit.rule.as_str()).collect();
        assert_eq!(rules, ["a.base", "a.special", "a.base", "a.special"]);
    }

    #[test]
    fn display_decisions_carry_the_rule_and_the_source() {
        let slot = legacy().evaluate(&Context {
            template: "list-page",
            element: Element::Slot,
            slot: Some("toolbar"),
            parent: None,
            widget: None,
            op: None,
            label: None,
            facts: &[],
        });
        let shown: Vec<_> = slot
            .decisions()
            .into_iter()
            .map(|d| (d.name, d.value, d.rule.unwrap(), d.source))
            .collect();
        assert!(shown.contains(&(
            "size".into(),
            "fixed 50px".into(),
            "size.toolbar-50".into(),
            Source::RuleTable
        )));
        assert!(shown.iter().any(|(name, value, rule, _)| {
            name == "surface" && value == "#F7F7F7" && rule == "surface.toolbar-band"
        }));
    }

    #[test]
    fn a_rule_naming_an_unknown_fact_is_refused() {
        let error = RuleSet::parse(
            r#"{"version":"legacy","rules":[{"id":"a.b","when":{"facts":["tall"]},"decisions":{"align":"center"}}]}"#,
        )
        .unwrap_err();
        assert!(error.contains("unknown fact `tall`"), "{error}");
    }
}
