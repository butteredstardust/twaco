//! `twaco mashup why <mashup> <widget>`: what the layout rules decide for a widget, which rule
//! decided it, and how to override it.
//!
//! The decisions come from the evaluator authoring and validation use ([`super::layout`]), so what
//! is printed is what the next edit would do. A node twaco made from a template names its template
//! slot; a container with no such marker is explained as `inferred`: the rules' answer now, not a
//! record of how it was made. A setting someone made on the node wins and is reported as authored.

use super::edit::{children, row_facts, takes_equal_width, EditError, MashupDocument};
use super::layout::context::{describe, Origin};
use super::layout::resolve::{Decision, Evaluation, Source};
use super::layout::Siblings;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::mashup::describe::DescribeError;
use crate::core::{mashup, workspace};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum WhyError {
    #[error(transparent)]
    Mashup(#[from] DescribeError),
    #[error(transparent)]
    Widget(#[from] EditError),
}

impl Coded for WhyError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Mashup(error) => error.code(),
            Self::Widget(error) => error.code(),
        }
    }
}

/// One decision for a widget.
#[derive(Debug, Serialize, PartialEq)]
pub struct WhyDecision {
    pub name: String,
    pub value: String,
    pub rule: Option<String>,
    pub source: Source,
    /// What the document holds now, when that can be said in one line.
    pub current: Option<String>,
    /// Whether `current` agrees with the decision; absent when there is nothing to compare.
    pub agrees: Option<bool>,
    pub override_with: String,
}

/// The decisions about one container.
#[derive(Debug, Serialize, PartialEq)]
pub struct Layer {
    pub widget: String,
    pub origin: String,
    pub decisions: Vec<WhyDecision>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Why {
    pub mashup: String,
    pub widget: String,
    pub widget_type: String,
    pub display_name: String,
    pub origin: String,
    pub inferred: bool,
    pub rules: String,
    pub decisions: Vec<WhyDecision>,
    /// The container this widget sits in, when the widget decides nothing itself.
    pub enclosing: Option<Layer>,
    pub notes: Vec<String>,
}

/// Explain one widget of a repository mashup.
pub fn why(solution: &Solution, mashup_name: &str, widget: &str) -> Result<Why, WhyError> {
    let content = read_content(solution, mashup_name)?;
    let document = mashup::edit::parse(&content)?;
    explain(&document, mashup_name, widget)
}

fn read_content(solution: &Solution, name: &str) -> Result<String, DescribeError> {
    let discovered = workspace::discover(solution);
    let entity = discovered
        .entities
        .iter()
        .find(|e| e.info.collection == "Mashups" && e.info.name == name)
        .ok_or_else(|| DescribeError::Unknown(name.to_string()))?;
    let unreadable = DescribeError::Unreadable;
    let dir = workspace::mashup_dir(solution, entity);
    match workspace::read_mashup(&dir).map_err(|error| unreadable(error.to_string()))? {
        Some(assets) => Ok(assets.content),
        None => {
            let bytes = std::fs::read(&entity.path)
                .map_err(|error| unreadable(format!("{}: {error}", entity.path.display())))?;
            Ok(mashup::extract(&bytes)
                .map_err(|error| unreadable(error.to_string()))?
                .content)
        }
    }
}

pub(crate) fn explain(
    document: &MashupDocument,
    mashup_name: &str,
    widget: &str,
) -> Result<Why, WhyError> {
    let found = document.find(widget)?;
    let own = layer(document, mashup_name, &found.path, &found.id, true);
    let enclosing = match found.path.split_last() {
        Some((_, parent)) if own.decisions.is_empty() && !parent.is_empty() => {
            let id = document.at(parent)["Properties"]["Id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let upper = layer(document, mashup_name, parent, &id, true);
            (!upper.decisions.is_empty()).then_some(upper)
        }
        _ => None,
    };
    let mut notes = Vec::new();
    let all = own
        .decisions
        .iter()
        .chain(enclosing.iter().flat_map(|upper| &upper.decisions));
    let inferred = all.clone().any(|d| d.source == Source::Inferred);
    if own.decisions.is_empty() && enclosing.is_none() {
        notes.push(format!(
            "no layout rule applies to {}; twaco leaves it as it is",
            found.type_id
        ));
    }
    if inferred {
        notes.push("twaco did not record how this container was made, so these are the rules' answers now (inferred), not provenance".into());
    }
    if own.decisions.iter().any(|d| d.name == "size")
        && document.at(&found.path).get("twacoLayoutHeight").is_some()
    {
        notes.push("twaco fitted this row's height to its content (twacoLayoutHeight), so the size from the rule table is where it started".into());
    }
    if all.clone().any(|d| d.source == Source::Authored) {
        notes.push("a setting someone made on this container (twacoLayoutAuthored) wins over the rule table; the rules do not change it".into());
    }
    Ok(Why {
        mashup: mashup_name.to_string(),
        widget: found.id,
        widget_type: found.type_id,
        display_name: found.display_name,
        origin: own.origin,
        inferred,
        rules: document.rules().version().into(),
        decisions: own.decisions,
        enclosing,
        notes,
    })
}

/// Decisions the mashup contradicts, for validation: a template slot's fixed size and a row of
/// cards that is not equal. Authored settings and inferred sizes never count.
pub(crate) fn contradictions(
    document: &MashupDocument,
    path: &[usize],
    id: &str,
) -> Vec<WhyDecision> {
    layer(document, "", path, id, false)
        .decisions
        .into_iter()
        .filter(|decision| {
            decision.agrees == Some(false)
                && match decision.name.as_str() {
                    "siblings" => decision.source != Source::Authored,
                    "size" => {
                        decision.source == Source::RuleTable && decision.value.starts_with("fixed ")
                    }
                    _ => false,
                }
        })
        .collect()
}

fn layer(
    document: &MashupDocument,
    mashup_name: &str,
    path: &[usize],
    id: &str,
    natural: bool,
) -> Layer {
    let described = describe(document, path, natural);
    let (origin, from_template) = match &described.origin {
        Origin::TemplatePage { template } => (format!("page of template {template}"), true),
        Origin::TemplateSlot { template, slot } => {
            (format!("slot \"{slot}\" of template {template}"), true)
        }
        Origin::TemplateWidget { template, slot } => (
            format!("widget held by slot \"{slot}\" of template {template}"),
            true,
        ),
        Origin::Generated => ("generated container".to_string(), false),
        Origin::Panel => ("card or panel container".to_string(), false),
        Origin::Other => ("not laid out by a twaco rule".to_string(), false),
    };
    let node = document.at(path);
    let facts = row_facts(node);
    let mut decisions: Vec<WhyDecision> = Vec::new();
    for context in &described.nodes {
        let decided = document.rules().evaluate(&context.context());
        for decision in decided.decisions() {
            if decision.name == "siblings" && children(node).len() < 2 {
                continue;
            }
            decisions.push(explained(
                document,
                mashup_name,
                (id, path),
                &decided,
                decision,
                from_template,
            ));
        }
        if decided.authored {
            for name in ["size", "siblings"] {
                if name == "siblings" && !facts.contains(&"row-of-panels") {
                    continue;
                }
                if !decisions.iter().any(|d| d.name == name) {
                    let (current, _) = current_of(document, path, name, "", &decided);
                    decisions.push(WhyDecision {
                        name: name.into(),
                        value: "as set".into(),
                        rule: None,
                        source: Source::Authored,
                        current,
                        agrees: None,
                        override_with: override_for(mashup_name, id, name),
                    });
                }
            }
        }
    }
    // Several contexts can decide one name; the first stands.
    let mut seen: Vec<String> = Vec::new();
    decisions.retain(|decision| {
        let fresh = !seen.contains(&decision.name);
        seen.push(decision.name.clone());
        fresh
    });
    Layer {
        widget: id.to_string(),
        origin,
        decisions,
    }
}

fn explained(
    document: &MashupDocument,
    mashup_name: &str,
    (id, path): (&str, &[usize]),
    decided: &Evaluation,
    decision: Decision,
    from_template: bool,
) -> WhyDecision {
    let (current, mut agrees) =
        current_of(document, path, &decision.name, &decision.value, decided);
    if decided.measured && decision.name == "size" {
        agrees = None;
    }
    WhyDecision {
        override_with: override_for(mashup_name, id, &decision.name),
        source: if decided.authored && matches!(decision.name.as_str(), "size" | "siblings") {
            Source::Authored
        } else if from_template {
            decision.source
        } else {
            Source::Inferred
        },
        name: decision.name,
        value: decision.value,
        rule: decision.rule,
        current,
        agrees,
    }
}

fn override_for(mashup_name: &str, id: &str, name: &str) -> String {
    let set = format!("twaco mashup set {mashup_name} {id}");
    match name {
        "size" => format!("{set} --prop flex-grow=<n> (any flex-*, Width or Height setting marks the size as authored)"),
        "gutter" => format!("{set} --prop Margin=\"<top right bottom left>\""),
        "align" => format!("{set} --prop align-items=<flex-start|center|flex-end|stretch>"),
        "justify" => format!("{set} --prop justify-content=<flex-start|center|flex-end|space-between>"),
        "surface" => format!("{set} --style background=<colour>"),
        "siblings" => format!("twaco mashup set {mashup_name} <member> --prop flex-grow=<n> (a flex-* setting marks that member as authored, so the row stops equalising it)"),
        other => format!(
            "{set} --prop {}=<value>",
            other.strip_prefix("prop ").unwrap_or(other)
        ),
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => "unset".into(),
        Value::String(text) if text.is_empty() => "none".into(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// What the document holds for a decision, in one line, and whether it agrees.
fn current_of(
    document: &MashupDocument,
    path: &[usize],
    name: &str,
    value: &str,
    decided: &Evaluation,
) -> (Option<String>, Option<bool>) {
    let properties = &document.at(path)["Properties"];
    let property = |key: &str| &properties[key];
    let same = |key: &str, wanted: &str| {
        let held = text(property(key));
        (Some(format!("{key}={held}")), Some(held == wanted))
    };
    match name {
        "size" => {
            if let Some(fixed) = value.strip_prefix("fixed ") {
                let dimension = match path.split_last() {
                    Some((_, parent))
                        if document.at(parent)["Properties"]["flex-direction"] == "row" =>
                    {
                        "width"
                    }
                    _ => "height",
                };
                let min = format!("flex-min-{dimension}");
                let max = format!("flex-max-{dimension}");
                let agrees = property("flex-size") == "fixed"
                    && property(&min) == fixed
                    && property(&max) == fixed;
                (
                    Some(format!(
                        "flex-size={}, {min}={}, {max}={}",
                        text(property("flex-size")),
                        text(property(&min)),
                        text(property(&max))
                    )),
                    Some(agrees),
                )
            } else {
                let held = property("flex-grow");
                (
                    Some(format!("flex-grow={}", text(held))),
                    decided
                        .grow()
                        .map(|grow| held.as_f64() == Some(f64::from(grow))),
                )
            }
        }
        "gutter" => {
            let held = match text(property("Margin")).as_str() {
                "unset" => "none".to_string(),
                other => other.to_string(),
            };
            (Some(format!("Margin={held}")), Some(held == value))
        }
        "align" => same("align-items", value),
        "justify" => same("justify-content", value),
        "surface" => {
            let id = properties["Id"].as_str().unwrap_or_default();
            let background = &document.value()["UI"]["Properties"]["StyleProperties"]["data"][id]
                ["parts"]["$"]["$"]["background"];
            let held = text(background);
            (Some(format!("background={held}")), Some(held == value))
        }
        "siblings" => {
            if decided.siblings.as_ref().map(|hit| hit.value) != Some(Siblings::Equal) {
                return (None, None);
            }
            let members: Vec<&Value> = children(document.at(path))
                .iter()
                .filter(|member| takes_equal_width(member))
                .collect();
            let equal = members
                .iter()
                .filter(|member| {
                    member["Properties"]["flex-grow"] == 1
                        && member["Properties"]["flex-basis"] == "0px"
                })
                .count();
            (
                Some(format!(
                    "{equal} of {} unsized members have flex-grow=1 and flex-basis=0px",
                    members.len()
                )),
                Some(equal == members.len()),
            )
        }
        other => {
            let key = other.strip_prefix("prop ").unwrap_or(other);
            let held = property(key);
            (
                Some(format!("{key}={}", text(held))),
                decided.prop(key).map(|wanted| wanted == held),
            )
        }
    }
}

fn lines(out: &mut String, decisions: &[WhyDecision]) {
    for decision in decisions {
        let source = match decision.source {
            Source::RuleTable => "rule table",
            Source::Authored => "explicit authored setting",
            Source::Inferred => "inferred",
        };
        out.push_str(&format!(
            "  {:<22}{:<20}{} ({source})
",
            decision.name,
            decision.value,
            decision
                .rule
                .as_deref()
                .map_or("no rule".to_string(), |rule| format!("rule {rule}")),
        ));
        if let Some(current) = &decision.current {
            let verdict = match decision.agrees {
                Some(true) => " [agrees]",
                Some(false) => " [differs]",
                None => "",
            };
            out.push_str(&format!(
                "    now: {current}{verdict}
"
            ));
        }
    }
}

/// The text `mashup why` prints.
pub fn render(why: &Why) -> String {
    let mut out = format!(
        "{} {} ({}, {})
  {}; rules: {}{}
",
        why.mashup,
        why.widget,
        why.widget_type,
        if why.display_name.is_empty() {
            "unnamed"
        } else {
            &why.display_name
        },
        why.origin,
        why.rules,
        if why.inferred { " (inferred)" } else { "" }
    );
    lines(&mut out, &why.decisions);
    if let Some(upper) = &why.enclosing {
        out.push_str(&format!(
            "inside {} ({})
",
            upper.widget, upper.origin
        ));
        lines(&mut out, &upper.decisions);
    }
    for note in &why.notes {
        out.push_str(&format!(
            "note: {note}
"
        ));
    }
    let mut overrides: Vec<&str> = Vec::new();
    for decision in why
        .decisions
        .iter()
        .chain(why.enclosing.iter().flat_map(|upper| &upper.decisions))
    {
        if !overrides.contains(&decision.override_with.as_str()) {
            overrides.push(&decision.override_with);
        }
    }
    if !overrides.is_empty() {
        out.push_str(
            "override:
",
        );
        for line in overrides {
            out.push_str(&format!(
                "  {line}
"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mashup::edit::{new_widget, parse};
    use crate::core::mashup::templates::BLANK;
    use crate::core::widgets;
    use serde_json::json;

    /// A row of two cards inside the page container, with the cards' flex settings given.
    fn cards(settings: [(u64, &str); 2]) -> MashupDocument {
        let mut document = parse(BLANK).unwrap();
        let center = document.find("center").unwrap().path;
        let container = widgets::catalog().by_type("flexcontainer").unwrap();
        let make = |document: &mut MashupDocument, parent: &[usize], name: &str| {
            let number = document.allocate();
            document.push_child(parent, new_widget(container, number, true));
            let path = document
                .find_exact(&format!("flexcontainer-{number}"))
                .unwrap()
                .path;
            document.set_property(&path, "DisplayName", json!(name));
            path
        };
        let row = make(&mut document, &center, "container-1");
        document.set_property(&row, "flex-direction", json!("row"));
        for (index, (grow, basis)) in settings.into_iter().enumerate() {
            let card = make(&mut document, &row, &format!("panel-{index}"));
            document.set_property(&card, "flex-grow", json!(grow));
            document.set_property(&card, "flex-basis", json!(basis));
        }
        document
    }

    #[test]
    fn a_row_of_unequal_cards_is_explained_as_inferred_and_contradicted() {
        let document = cards([(1, "0px"), (2, "auto")]);
        let found = explain(&document, "Acme.Case_MU", "container-1").unwrap();
        assert_eq!(found.origin, "generated container");
        assert!(found.inferred);
        let siblings = found
            .decisions
            .iter()
            .find(|d| d.name == "siblings")
            .unwrap();
        assert_eq!(
            (
                siblings.value.as_str(),
                siblings.rule.as_deref(),
                siblings.source,
                siblings.agrees
            ),
            (
                "equal",
                Some("siblings.panel-row"),
                Source::Inferred,
                Some(false)
            )
        );
        assert!(siblings.override_with.contains("--prop flex-grow"));
        let row = document.find("container-1").unwrap();
        assert_eq!(contradictions(&document, &row.path, &row.id,).len(), 1);
        let text = render(&found);
        assert!(
            text.contains("rule siblings.panel-row (inferred)"),
            "{text}"
        );
        assert!(text.contains("[differs]"), "{text}");
    }

    #[test]
    fn equal_cards_agree_and_nothing_is_contradicted() {
        let document = cards([(1, "0px"), (1, "0px")]);
        let row = document.find("container-1").unwrap();
        let found = explain(&document, "Acme.Case_MU", &row.id).unwrap();
        assert!(found.decisions.iter().any(|d| d.agrees == Some(true)));
        assert!(contradictions(&document, &row.path, &row.id).is_empty());
    }

    #[test]
    fn an_authored_row_is_reported_as_authored_and_never_contradicted() {
        let mut document = cards([(1, "0px"), (2, "auto")]);
        let row = document.find("container-1").unwrap();
        document.at_mut(&row.path)["twacoLayoutAuthored"] = json!(true);
        let found = explain(&document, "Acme.Case_MU", &row.id).unwrap();
        let authored: Vec<_> = found
            .decisions
            .iter()
            .filter(|d| d.source == Source::Authored)
            .map(|d| (d.name.as_str(), d.rule.as_deref()))
            .collect();
        assert_eq!(authored, [("size", None), ("siblings", None)]);
        assert!(found
            .notes
            .iter()
            .any(|n| n.contains("twacoLayoutAuthored")));
        assert!(contradictions(&document, &row.path, &row.id).is_empty());
    }

    #[test]
    fn a_widget_no_rule_covers_says_so_and_an_unknown_widget_is_refused() {
        let document = cards([(1, "0px"), (1, "0px")]);
        let found = explain(&document, "Acme.Case_MU", "panel-0").unwrap();
        assert!(found.decisions.is_empty());
        let upper = found.enclosing.expect("the row it sits in decides for it");
        assert!(upper.decisions.iter().any(|d| d.name == "siblings"));
        let alone = explain(&document, "Acme.Case_MU", "mashup-root").unwrap();
        assert!(alone.notes[0].contains("no layout rule applies"));
        let error = explain(&document, "Acme.Case_MU", "nothing-here").unwrap_err();
        assert!(matches!(error, WhyError::Widget(EditError::NoWidget(_))));
        assert_eq!(error.code(), ErrorCode::UnknownEntity);
    }

    #[test]
    fn a_fixed_slot_size_is_judged_unless_twaco_fitted_it_or_someone_set_it() {
        let mut document = parse(BLANK).unwrap();
        let center = document.find("center").unwrap().path;
        document.at_mut(&center)["twacoTemplateName"] = json!("list-page");
        let container = widgets::catalog().by_type("flexcontainer").unwrap();
        let number = document.allocate();
        document.push_child(&center, new_widget(container, number, true));
        let slot = document
            .find_exact(&format!("flexcontainer-{number}"))
            .unwrap();
        document.set_property(&center, "flex-direction", json!("column"));
        document.set_property(&slot.path, "DisplayName", json!("toolbar"));
        document.at_mut(&slot.path)["twacoTemplateSlot"] = json!(true);
        for (key, value) in [
            ("flex-size", json!("fixed")),
            ("flex-min-height", json!("64px")),
            ("flex-max-height", json!("64px")),
        ] {
            document.set_property(&slot.path, key, value);
        }
        let found = contradictions(&document, &slot.path, &slot.id);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule.as_deref(), Some("size.toolbar-50"));
        assert_eq!(found[0].source, Source::RuleTable);
        document.at_mut(&slot.path)["twacoLayoutHeight"] = json!("64px");
        assert!(contradictions(&document, &slot.path, &slot.id).is_empty());
        let shown = explain(&document, "Acme.Case_MU", &slot.id).unwrap();
        assert!(shown.notes.iter().any(|n| n.contains("fitted")));
        document
            .at_mut(&slot.path)
            .as_object_mut()
            .unwrap()
            .remove("twacoLayoutHeight");
        document.at_mut(&slot.path)["twacoLayoutAuthored"] = json!(true);
        assert!(contradictions(&document, &slot.path, &slot.id).is_empty());
    }
}
