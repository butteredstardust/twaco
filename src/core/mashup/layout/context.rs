//! Layout contexts: what the rule table is asked about, for an operation or a document node.
//!
//! An authoring operation builds a [`Node`] for the container or widget it is about to write and
//! names the operation (`op`). `mashup why` and validation describe a node of an existing document
//! with [`describe`]: its template and slot when twaco made it from one, and the facts the rules
//! may ask (authored, a row of panels, a mixed label/control row, a narrow column, ...).

use super::{Context, Element};
use crate::core::mashup::edit::{row_facts, type_of, MashupDocument};
use serde_json::Value;

/// A column this narrow cannot show a long date: its pickers default to a short format.
pub(crate) const NARROW_COLUMN: u32 = 320;

/// An owned [`Context`].
#[derive(Clone, Debug)]
pub(crate) struct Node {
    template: String,
    element: Element,
    slot: Option<String>,
    parent: Option<String>,
    widget: Option<String>,
    op: Option<&'static str>,
    label: Option<String>,
    facts: Vec<&'static str>,
}

impl Node {
    fn new(element: Element) -> Self {
        Self {
            template: String::new(),
            element,
            slot: None,
            parent: None,
            widget: None,
            op: None,
            label: None,
            facts: Vec::new(),
        }
    }

    /// A node an operation decides about.
    pub(crate) fn op(op: &'static str, element: Element) -> Self {
        Self {
            op: Some(op),
            ..Self::new(element)
        }
    }

    /// The role the node plays in its operation, such as a toolbar's `right` group.
    pub(crate) fn slot(mut self, role: &str) -> Self {
        self.slot = Some(role.to_string());
        self
    }

    pub(crate) fn widget(mut self, type_id: &str) -> Self {
        self.widget = Some(type_id.to_string());
        self
    }

    pub(crate) fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_string());
        self
    }

    /// Add the facts that hold.
    pub(crate) fn facts(mut self, facts: &[&'static str]) -> Self {
        for fact in facts {
            if !self.facts.contains(fact) {
                self.facts.push(fact);
            }
        }
        self
    }

    /// Add a fact when `holds`.
    pub(crate) fn fact(self, fact: &'static str, holds: bool) -> Self {
        if holds {
            self.facts(&[fact])
        } else {
            self
        }
    }

    pub(crate) fn context(&self) -> Context<'_> {
        Context {
            template: &self.template,
            element: self.element,
            slot: self.slot.as_deref(),
            parent: self.parent.as_deref(),
            widget: self.widget.as_deref(),
            op: self.op,
            label: self.label.as_deref(),
            facts: &self.facts,
        }
    }
}

/// What a document node is, as far as the layout rules know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The root container of a template.
    TemplatePage { template: String },
    /// A named slot of a template.
    TemplateSlot { template: String, slot: String },
    /// A widget a template slot holds.
    TemplateWidget { template: String, slot: String },
    /// A container twaco generated.
    Generated,
    /// A container someone added as a card or panel.
    Panel,
    /// Anything else: twaco did not lay it out.
    Other,
}

/// A document node with the contexts to evaluate for it.
#[derive(Clone, Debug)]
pub(crate) struct Described {
    pub(crate) origin: Origin,
    pub(crate) nodes: Vec<Node>,
}

/// Describe the node at `path`: the rule contexts that apply to it.
///
/// `natural` also builds the context that decides how a row of a column grows, which reads the
/// whole column; validation does not need it.
pub(crate) fn describe(document: &MashupDocument, path: &[usize], natural: bool) -> Described {
    let node = document.at(path);
    let own: Vec<&Value> = (0..=path.len()).map(|d| document.at(&path[..d])).collect();
    let template = own
        .iter()
        .rev()
        .find_map(|widget| widget["twacoTemplateName"].as_str())
        .map(str::to_string);
    let name = |widget: &Value| {
        widget["Properties"]["DisplayName"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let facts = row_facts(node);
    let make = |element: Element| {
        let mut made = Node::new(element).facts(&facts);
        made.template = template.clone().unwrap_or_default();
        made
    };
    if let Some(name_of_template) = node["twacoTemplateName"].as_str() {
        return Described {
            origin: Origin::TemplatePage {
                template: name_of_template.to_string(),
            },
            nodes: vec![make(Element::Page)],
        };
    }
    let slots: Vec<String> = own
        .iter()
        .filter(|widget| widget["twacoTemplateSlot"] == true)
        .map(|widget| name(widget))
        .collect();
    if node["twacoTemplateSlot"] == true {
        let slot = name(node);
        let divider = slot.starts_with("divider-");
        let mut made = make(if divider {
            Element::Divider
        } else {
            Element::Slot
        });
        made.slot = Some(slot.clone());
        made.parent = slots.iter().rev().nth(1).cloned();
        return Described {
            origin: Origin::TemplateSlot {
                template: template.unwrap_or_default(),
                slot,
            },
            nodes: vec![made],
        };
    }
    if let (Some(template), Some(slot), Some((_, parent))) =
        (&template, slots.last(), path.split_last())
    {
        if document.at(parent)["twacoTemplateSlot"] == true
            && type_of(node) != Some("flexcontainer")
        {
            let mut made = make(Element::Widget);
            made.slot = Some(slot.clone());
            made.parent = slots.iter().rev().nth(1).cloned();
            made.widget = type_of(node).map(str::to_string);
            return Described {
                origin: Origin::TemplateWidget {
                    template: template.clone(),
                    slot: slot.clone(),
                },
                nodes: vec![made],
            };
        }
    }
    if type_of(node) == Some("flexcontainer") {
        let in_column = path.split_last().is_some_and(|(_, parent)| {
            document.at(parent)["Properties"]["flex-direction"] == "column"
        });
        let mut nodes = vec![
            make(Element::Row).with_op("equal-siblings"),
            make(Element::Row).with_op("control-row"),
        ];
        if in_column && natural {
            let (index, parent) = path.split_last().map(|(i, p)| (*i, p)).expect("a parent");
            let mut natural = make(Element::Row).with_op("natural-height");
            natural.facts = document.natural_height_facts(parent, index);
            if facts.contains(&"authored") {
                natural.facts.push("authored");
            }
            nodes.push(natural);
        }
        let panel = node["Properties"]["DisplayName"]
            .as_str()
            .is_some_and(|name| name.starts_with("panel-"));
        return Described {
            origin: if panel {
                Origin::Panel
            } else if node["Properties"]["DisplayName"]
                .as_str()
                .is_some_and(|name| name.starts_with("container-"))
            {
                Origin::Generated
            } else {
                Origin::Other
            },
            nodes,
        };
    }
    Described {
        origin: Origin::Other,
        nodes: Vec::new(),
    }
}

impl Node {
    fn with_op(mut self, op: &'static str) -> Self {
        self.op = Some(op);
        self
    }
}

/// Whether a widget sits in a column too narrow for long dates.
pub(crate) fn narrow_column(document: &MashupDocument, path: &[usize]) -> bool {
    (1..path.len()).any(|depth| {
        let p = &document.at(&path[..depth])["Properties"];
        let px = |key: &str| {
            p[key]
                .as_str()
                .and_then(|width| width.trim_end_matches("px").parse::<u32>().ok())
        };
        matches!(p["DisplayName"].as_str(), Some("filters" | "filter-column"))
            || px("flex-max-width").is_some_and(|width| width <= NARROW_COLUMN)
            || (p["flex-size"] == "fixed"
                && px("flex-basis").is_some_and(|width| width <= NARROW_COLUMN))
            || (p["Type"] == "flexcontainer"
                && p["ResponsiveLayout"] == false
                && p["Width"]
                    .as_u64()
                    .is_some_and(|width| width <= u64::from(NARROW_COLUMN)))
    })
}

/// Whether a widget is inside the filter panel's footer.
pub(crate) fn in_filter_footer(document: &MashupDocument, path: &[usize]) -> bool {
    (1..path.len())
        .any(|depth| document.at(&path[..depth])["Properties"]["DisplayName"] == "filter-footer")
}
