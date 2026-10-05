//! The solution as one immutable graph of what depends on what.
//!
//! Every entity document the repository holds is a node, and so is every configured project. An
//! edge points from the thing that refers to another to the thing it refers to, and says how it
//! knows: a template or shape named in the XML is **structural**; a static name in a script or a
//! mashup binding that the repository proves is **resolved**; a string that merely looks like an
//! entity's name, or a script the parser could not read, is **review** only. Nothing here claims a
//! complete picture: names built at run time and anything outside the repository are invisible, and
//! [`Index::unreadable`] lists the inputs that could not be read at all, so a command that acts on
//! the answer can say what it left out.
//!
//! The index reads the entity XML, which `twaco check` holds in step with the sidecars. It is built
//! once per command and never changes.

mod build;
mod deploy;
pub(crate) mod inherit;
mod mashups;
mod query;
mod scripts;

#[cfg(test)]
mod tests;

pub use query::{Dependent, DependentOptions, Step};

use super::entity_key::EntityKey;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// How sure the index is of an edge. Ordered: `Review < Resolved < Structural`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// A string that looks like an entity's name, or a reference in a script that could not be
    /// parsed. A person has to decide.
    Review,
    /// A static name in a script or a mashup binding, matched to an entity the repository holds.
    Resolved,
    /// Declared in the entity's own XML or in `twaco.toml`.
    Structural,
}

impl Confidence {
    pub fn word(self) -> &'static str {
        match self {
            Confidence::Review => "review",
            Confidence::Resolved => "resolved",
            Confidence::Structural => "structural",
        }
    }

    pub fn parse(word: &str) -> Option<Confidence> {
        match word {
            "review" => Some(Confidence::Review),
            "resolved" => Some(Confidence::Resolved),
            "structural" => Some(Confidence::Structural),
            _ => None,
        }
    }
}

/// What one reference is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// A Thing's `thingTemplate`, or a template's `baseThingTemplate`.
    Template,
    /// A Thing or template implementing a ThingShape.
    ImplementedShape,
    /// A DataShape's `baseDataShape`.
    BaseDataShape,
    /// A service parameter, a service result, a property or a field typed by a DataShape.
    DataShape,
    /// A script reading `Things["X"]` or `Things.X`, and then a member of it.
    ScriptReference,
    /// A string in a script that equals an entity's name.
    ScriptMention,
    /// A mashup widget bound to an entity's service or property.
    MashupBinding,
    /// A string in a mashup that equals an entity's name.
    MashupMention,
    /// `twaco.toml` naming an entity: a project's entry point or a post-import call.
    Deploy,
    /// A project that must be imported before another.
    ProjectDependency,
}

impl EdgeKind {
    pub fn confidence(self) -> Confidence {
        match self {
            EdgeKind::Template
            | EdgeKind::ImplementedShape
            | EdgeKind::BaseDataShape
            | EdgeKind::DataShape
            | EdgeKind::Deploy
            | EdgeKind::ProjectDependency => Confidence::Structural,
            EdgeKind::ScriptReference | EdgeKind::MashupBinding => Confidence::Resolved,
            EdgeKind::ScriptMention | EdgeKind::MashupMention => Confidence::Review,
        }
    }

    /// A reference that makes the referrer inherit the members of what it names.
    pub fn is_inheritance(self) -> bool {
        matches!(
            self,
            EdgeKind::Template | EdgeKind::ImplementedShape | EdgeKind::BaseDataShape
        )
    }

    pub fn word(self) -> &'static str {
        match self {
            EdgeKind::Template => "template",
            EdgeKind::ImplementedShape => "implemented_shape",
            EdgeKind::BaseDataShape => "base_data_shape",
            EdgeKind::DataShape => "data_shape",
            EdgeKind::ScriptReference => "script_reference",
            EdgeKind::ScriptMention => "script_mention",
            EdgeKind::MashupBinding => "mashup_binding",
            EdgeKind::MashupMention => "mashup_mention",
            EdgeKind::Deploy => "deploy",
            EdgeKind::ProjectDependency => "project_dependency",
        }
    }
}

/// One reference: from the node it sits on to the node it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub kind: EdgeKind,
    /// The service, property or field of the referrer that holds the reference, if it is in one.
    pub from_member: Option<String>,
    /// The member of the target that is named (`Things["X"].Run()` names `Run`), if one is.
    pub to_member: Option<String>,
    /// The file, relative to the solution root, where the reference is written.
    pub at: Option<String>,
}

impl Edge {
    pub fn confidence(&self) -> Confidence {
        self.kind.confidence()
    }
}

/// What a node is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Entity(EntityKey),
    Project(String),
}

/// One node: an entity document or a configured project.
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    /// The project whose folders hold it, or the project's own name.
    pub project: String,
    /// The entity's file, relative to the solution root; none for a project.
    pub file: Option<PathBuf>,
    /// A Thing's `thingTemplate` or a template's `baseThingTemplate`, as written.
    pub template: Option<String>,
    /// The ThingShapes it implements, as written and in order.
    pub shapes: Vec<String>,
    /// The services it declares itself.
    pub services: BTreeSet<String>,
}

impl Node {
    /// `Collection/Name`, or `project <name>`.
    pub fn label(&self) -> String {
        match &self.kind {
            NodeKind::Entity(key) => key.to_string(),
            NodeKind::Project(name) => format!("project {name}"),
        }
    }

    pub fn key(&self) -> Option<&EntityKey> {
        match &self.kind {
            NodeKind::Entity(key) => Some(key),
            NodeKind::Project(_) => None,
        }
    }
}

/// An input the index could not read. What depends on it is missing from every answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Unreadable {
    /// A file, or `entity/service` for a script.
    pub what: String,
    pub why: String,
}

/// The whole picture. Built once by [`Index::build`]; never changed.
#[derive(Debug)]
pub struct Index {
    graph: DiGraph<Node, Edge>,
    entities: BTreeMap<EntityKey, NodeIndex>,
    projects: BTreeMap<String, NodeIndex>,
    /// Project names in the order they must be imported.
    deploy_order: Vec<String>,
    unreadable: Vec<Unreadable>,
    /// `entity/service` of every script the parser refused: its references are known only as
    /// review-level mentions.
    unparsed: Vec<String>,
}

impl Index {
    /// Every input that could not be read. An answer from an index with entries here is partial,
    /// and a command that acts on it says so or refuses.
    pub fn unreadable(&self) -> &[Unreadable] {
        &self.unreadable
    }

    /// Scripts the parser refused, as `entity/service`. Their references are review-level only.
    pub fn unparsed_scripts(&self) -> &[String] {
        &self.unparsed
    }

    /// Whether every input was read.
    pub fn is_complete(&self) -> bool {
        self.unreadable.is_empty()
    }

    /// The project names in the order they must be imported.
    pub fn deploy_order(&self) -> &[String] {
        &self.deploy_order
    }

    pub fn node(&self, key: &EntityKey) -> Option<&Node> {
        self.entities.get(key).map(|at| &self.graph[*at])
    }

    pub fn contains(&self, key: &EntityKey) -> bool {
        self.entities.contains_key(key)
    }

    /// Every entity, in `Collection/Name` order.
    pub fn entities(&self) -> impl Iterator<Item = (&EntityKey, &Node)> {
        self.entities
            .iter()
            .map(|(key, at)| (key, &self.graph[*at]))
    }

    /// Every edge, as the labels of its two ends and the edge itself, in a stable order.
    pub fn edges(&self) -> Vec<(String, String, &Edge)> {
        use petgraph::visit::EdgeRef;
        let mut out: Vec<(String, String, &Edge)> = self
            .graph
            .edge_references()
            .map(|edge| {
                (
                    self.graph[edge.source()].label(),
                    self.graph[edge.target()].label(),
                    edge.weight(),
                )
            })
            .collect();
        out.sort_by(|a, b| {
            (&a.0, &a.1, a.2.kind, &a.2.from_member, &a.2.to_member).cmp(&(
                &b.0,
                &b.1,
                b.2.kind,
                &b.2.from_member,
                &b.2.to_member,
            ))
        });
        out
    }
}
