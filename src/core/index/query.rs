//! Questions asked of an [`Index`].

use super::inherit::{implements_shape, inheritance_chain, Inherits};
use super::{Confidence, Edge, EdgeKind, Index};
use crate::core::entity_key::EntityKey;
use crate::core::workspace::WorkspaceError;
use petgraph::graph::NodeIndex;
use petgraph::visit::{EdgeFiltered, EdgeRef};
use petgraph::Direction;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// What to ask [`Index::dependents`].
#[derive(Clone, Debug)]
pub struct DependentOptions {
    /// Restrict the question to one service, property or field of the entity: who calls `Run`,
    /// not who uses the entity at all.
    pub member: Option<String>,
    /// The weakest edge to follow. `Review` follows everything.
    pub min: Confidence,
    /// How many references away to look; none is no limit.
    pub max_depth: Option<usize>,
}

impl Default for DependentOptions {
    fn default() -> Self {
        DependentOptions {
            member: None,
            min: Confidence::Review,
            max_depth: None,
        }
    }
}

/// One reference on the way from a dependent to what it depends on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Step {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub from_member: Option<String>,
    pub to_member: Option<String>,
    pub at: Option<String>,
}

/// Something that depends, directly or through others, on the thing asked about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Dependent {
    /// `Collection/Name`, or `project <name>`.
    pub label: String,
    /// The project it belongs to.
    pub project: String,
    /// How many references away it is.
    pub depth: usize,
    /// The strongest chain of references that reaches it is only as sure as its weakest link.
    pub confidence: Confidence,
    /// The services, properties or fields of it that are involved.
    pub members: Vec<String>,
    /// The shortest chain that reaches it, starting at the thing asked about.
    pub path: Vec<Step>,
}

/// The member a reference carries onward. Inheritance and typing carry the member that was
/// asked about; any other reference carries the member that holds it.
fn carried(edge: &Edge, current: &Option<String>) -> Option<String> {
    if edge.kind.is_inheritance() {
        current.clone()
    } else {
        edge.from_member.clone()
    }
}

/// Whether following `edge` backwards answers a question about `member` of its target. A
/// reference that names no member (inheritance, a typed field, a mention, a deploy call) concerns
/// every member; one that names a member concerns only that one.
fn concerns(edge: &Edge, member: &Option<String>) -> bool {
    match (member, &edge.to_member) {
        (None, _) => true,
        (Some(_), None) => true,
        (Some(asked), Some(named)) => asked == named,
    }
}

impl Index {
    fn at(&self, key: &EntityKey) -> Option<NodeIndex> {
        self.entities.get(key).copied()
    }

    fn step(&self, edge: petgraph::graph::EdgeReference<'_, Edge>) -> Step {
        let weight = edge.weight();
        Step {
            from: self.graph[edge.source()].label(),
            to: self.graph[edge.target()].label(),
            kind: weight.kind,
            from_member: weight.from_member.clone(),
            to_member: weight.to_member.clone(),
            at: weight.at.clone(),
        }
    }

    /// Everything that depends on `key`, directly or through others, nearest first.
    ///
    /// Each dependent carries the strongest confidence at which it is reached: it is found once
    /// at the strongest tier that reaches it, with the shortest chain at that tier. A dependent
    /// reached only through a review-level reference is reported as review.
    pub fn dependents(&self, key: &EntityKey, options: &DependentOptions) -> Vec<Dependent> {
        let Some(start) = self.at(key) else {
            return Vec::new();
        };
        let tiers: Vec<Confidence> = [
            Confidence::Structural,
            Confidence::Resolved,
            Confidence::Review,
        ]
        .into_iter()
        .filter(|tier| *tier >= options.min)
        .collect();
        let mut found: BTreeMap<NodeIndex, Dependent> = BTreeMap::new();
        for tier in tiers {
            let mut seen: BTreeSet<(NodeIndex, Option<String>)> = BTreeSet::new();
            let mut queue: VecDeque<(NodeIndex, Option<String>, usize, Vec<Step>)> =
                VecDeque::new();
            seen.insert((start, options.member.clone()));
            queue.push_back((start, options.member.clone(), 0, Vec::new()));
            while let Some((node, member, depth, path)) = queue.pop_front() {
                if options.max_depth.is_some_and(|limit| depth >= limit) {
                    continue;
                }
                for edge in self.graph.edges_directed(node, Direction::Incoming) {
                    let weight = edge.weight();
                    if weight.confidence() < tier || !concerns(weight, &member) {
                        continue;
                    }
                    let from = edge.source();
                    if from == start {
                        continue;
                    }
                    let next_member = carried(weight, &member);
                    let mut next_path = path.clone();
                    next_path.push(self.step(edge));
                    if seen.insert((from, next_member.clone())) {
                        queue.push_back((from, next_member.clone(), depth + 1, next_path.clone()));
                    }
                    let entry = found.entry(from).or_insert_with(|| Dependent {
                        label: self.graph[from].label(),
                        project: self.graph[from].project.clone(),
                        depth: depth + 1,
                        confidence: tier,
                        members: Vec::new(),
                        path: next_path.clone(),
                    });
                    // A later, stronger tier does not run (tiers go strongest first), so the
                    // first sighting fixes confidence and path; only the members accumulate.
                    if entry.confidence == tier {
                        if let Some(name) = &next_member {
                            if !entry.members.contains(name) {
                                entry.members.push(name.clone());
                            }
                        }
                    }
                }
            }
        }
        let mut out: Vec<Dependent> = found.into_values().collect();
        for dependent in &mut out {
            dependent.members.sort();
        }
        out.sort_by(|a, b| {
            (a.depth, std::cmp::Reverse(a.confidence), &a.label).cmp(&(
                b.depth,
                std::cmp::Reverse(b.confidence),
                &b.label,
            ))
        });
        out
    }

    /// What refers to `key` directly, as the references themselves, strongest first.
    pub fn references_to(&self, key: &EntityKey) -> Vec<Step> {
        let Some(to) = self.at(key) else {
            return Vec::new();
        };
        let mut out: Vec<(Confidence, Step)> = self
            .graph
            .edges_directed(to, Direction::Incoming)
            .map(|edge| (edge.weight().confidence(), self.step(edge)))
            .collect();
        out.sort_by(|a, b| {
            (std::cmp::Reverse(a.0), &a.1.from, a.1.kind, &a.1.to_member).cmp(&(
                std::cmp::Reverse(b.0),
                &b.1.from,
                b.1.kind,
                &b.1.to_member,
            ))
        });
        out.into_iter().map(|(_, step)| step).collect()
    }

    /// What `key` refers to directly, as the references themselves, strongest first.
    pub fn references_from(&self, key: &EntityKey) -> Vec<Step> {
        let Some(from) = self.at(key) else {
            return Vec::new();
        };
        let mut out: Vec<(Confidence, Step)> = self
            .graph
            .edges_directed(from, Direction::Outgoing)
            .map(|edge| (edge.weight().confidence(), self.step(edge)))
            .collect();
        out.sort_by(|a, b| {
            (std::cmp::Reverse(a.0), &a.1.to, a.1.kind, &a.1.from_member).cmp(&(
                std::cmp::Reverse(b.0),
                &b.1.to,
                b.1.kind,
                &b.1.from_member,
            ))
        });
        out.into_iter().map(|(_, step)| step).collect()
    }

    /// The entity a person means by `given`: `Collection/Name`, a full name, or its last dotted
    /// segment (case-insensitively, as typed on Windows). A name that fits two entities is an
    /// error listing them, never a first-match win.
    pub fn resolve(&self, given: &str) -> Result<EntityKey, WorkspaceError> {
        let unknown = || WorkspaceError::UnknownEntity {
            name: given.to_string(),
        };
        if given.contains('/') {
            return EntityKey::parse(given)
                .ok()
                .filter(|key| self.contains(key))
                .ok_or_else(unknown);
        }
        let labels = |keys: &[&EntityKey]| keys.iter().map(|key| key.to_string()).collect();
        let exact: Vec<&EntityKey> = self
            .entities
            .keys()
            .filter(|key| key.name() == given)
            .collect();
        match exact.len() {
            1 => return Ok(exact[0].clone()),
            0 => {}
            _ => {
                return Err(WorkspaceError::Ambiguous {
                    name: given.to_string(),
                    found: labels(&exact),
                })
            }
        }
        let suffix: Vec<&EntityKey> = self
            .entities
            .keys()
            .filter(|key| {
                key.name()
                    .rsplit('.')
                    .next()
                    .is_some_and(|last| last.eq_ignore_ascii_case(given))
            })
            .collect();
        match suffix.len() {
            0 => Err(unknown()),
            1 => Ok(suffix[0].clone()),
            _ => Err(WorkspaceError::Ambiguous {
                name: given.to_string(),
                found: labels(&suffix),
            }),
        }
    }

    fn inherits_of<'a>(&'a self, key: &'a EntityKey) -> Option<Inherits<'a>> {
        let node = self.node(key)?;
        Some(Inherits {
            collection: key.collection(),
            template: node.template.as_deref(),
            shapes: &node.shapes,
        })
    }

    fn template_named(&self, name: &str) -> Option<Inherits<'_>> {
        let node = self
            .entities
            .get_key_value(&EntityKey::new("ThingTemplates", name).ok()?)?;
        Some(Inherits {
            collection: node.0.collection(),
            template: self.graph[*node.1].template.as_deref(),
            shapes: &self.graph[*node.1].shapes,
        })
    }

    /// The templates and shapes `key` inherits, nearest first: see
    /// [`inheritance_chain`], the one walk the service catalog shares.
    pub fn inheritance_names(&self, key: &EntityKey) -> Vec<String> {
        self.inherits_of(key)
            .map(|start| inheritance_chain(start, |name| self.template_named(name)))
            .unwrap_or_default()
    }

    /// The Things and templates that implement `shape`, directly or through a template chain, by
    /// name, sorted. `project` limits the answer to one project's entities.
    pub fn implementers(&self, shape: &EntityKey, project: Option<&str>) -> Vec<String> {
        let mut out: Vec<String> = self
            .entities()
            .filter(|(key, node)| {
                key.collection() != "ThingShapes"
                    && project.is_none_or(|project| node.project == project)
            })
            .filter(|(key, _)| {
                self.inherits_of(key).is_some_and(|start| {
                    implements_shape(start, shape.name(), |name| self.template_named(name))
                })
            })
            .map(|(key, _)| key.name().to_string())
            .collect();
        out.sort();
        out
    }

    /// Groups of entities that inherit from each other in a circle (a template that is its own
    /// ancestor, a DataShape whose base leads back to itself). An empty answer means inheritance
    /// is a tree.
    pub fn inheritance_cycles(&self) -> Vec<Vec<String>> {
        let inheritance =
            EdgeFiltered::from_fn(&self.graph, |edge| edge.weight().kind.is_inheritance());
        let mut cycles: Vec<Vec<String>> = petgraph::algo::tarjan_scc(&inheritance)
            .into_iter()
            .filter(|group| {
                group.len() > 1
                    || group.first().is_some_and(|node| {
                        self.graph
                            .edges_connecting(*node, *node)
                            .any(|edge| edge.weight().kind.is_inheritance())
                    })
            })
            .map(|group| {
                let mut names: Vec<String> =
                    group.iter().map(|node| self.graph[*node].label()).collect();
                names.sort();
                names
            })
            .collect();
        cycles.sort();
        cycles
    }

    /// Everything reachable by following references forward from `roots`, the roots included.
    pub fn reachable_from(&self, roots: &[EntityKey], min: Confidence) -> BTreeSet<EntityKey> {
        let mut seen: BTreeSet<NodeIndex> = BTreeSet::new();
        let mut queue: VecDeque<NodeIndex> = VecDeque::new();
        for root in roots {
            if let Some(at) = self.at(root) {
                if seen.insert(at) {
                    queue.push_back(at);
                }
            }
        }
        while let Some(node) = queue.pop_front() {
            for edge in self.graph.edges_directed(node, Direction::Outgoing) {
                if edge.weight().confidence() >= min && seen.insert(edge.target()) {
                    queue.push_back(edge.target());
                }
            }
        }
        seen.into_iter()
            .filter_map(|node| self.graph[node].key().cloned())
            .collect()
    }

    /// The projects that hold any of `labels` (entity labels as returned in [`Dependent::label`]
    /// or `Collection/Name`), with each project's place in the deploy order, in that order.
    pub fn projects_of(&self, labels: &[String]) -> Vec<(usize, String)> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        for label in labels {
            if let Some(name) = label.strip_prefix("project ") {
                names.insert(name.to_string());
                continue;
            }
            if let Some(node) = label
                .split_once('/')
                .and_then(|(collection, name)| EntityKey::new(collection, name).ok())
                .and_then(|key| self.node(&key))
            {
                names.insert(node.project.clone());
            }
        }
        let mut out: Vec<(usize, String)> = names
            .into_iter()
            .map(|name| {
                let place = self
                    .deploy_order
                    .iter()
                    .position(|candidate| candidate == &name)
                    .unwrap_or(usize::MAX);
                (place, name)
            })
            .collect();
        out.sort();
        out
    }

    /// The project nodes of the graph, by name.
    pub fn project_names(&self) -> Vec<&str> {
        self.projects.keys().map(String::as_str).collect()
    }

    /// The labels of everything that refers to `key` at all, entities and projects, sorted. A
    /// dependent that is itself unreferenced is a dead end, and shows up here for the entity it
    /// still names.
    pub fn referrers(&self, key: &EntityKey) -> Vec<String> {
        let Some(node) = self.at(key) else {
            return Vec::new();
        };
        let mut out: Vec<String> = self
            .graph
            .edges_directed(node, Direction::Incoming)
            .map(|edge| self.graph[edge.source()].label())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The entities `twaco.toml` names as a project's entry point or a post-import call.
    pub fn deploy_targets(&self) -> Vec<EntityKey> {
        self.entities
            .iter()
            .filter(|(_, node)| {
                self.graph
                    .edges_directed(**node, Direction::Incoming)
                    .any(|edge| edge.weight().kind == EdgeKind::Deploy)
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Whether `key` runs without being called: it declares subscriptions, it inherits a template
    /// or shape that does, or it is a Timer or Scheduler.
    pub fn runs_on_events(&self, key: &EntityKey) -> bool {
        let Some(node) = self.node(key) else {
            return false;
        };
        let runs = |name: &str| matches!(name, "Timer" | "Scheduler");
        if node.subscribes || node.template.as_deref().is_some_and(runs) {
            return true;
        }
        self.inheritance_names(key).iter().any(|name| {
            runs(name)
                || ["ThingTemplates", "ThingShapes"].iter().any(|collection| {
                    EntityKey::new(*collection, name)
                        .ok()
                        .and_then(|inherited| self.node(&inherited))
                        .is_some_and(|node| {
                            node.subscribes || node.template.as_deref().is_some_and(runs)
                        })
                })
        })
    }

    /// Whether the node standing for `key` is a configured project's entry point or deploy call
    /// target, as `twaco.toml` names it.
    pub fn is_deployed(&self, key: &EntityKey) -> bool {
        self.at(key).is_some_and(|node| {
            self.graph
                .edges_directed(node, Direction::Incoming)
                .any(|edge| edge.weight().kind == EdgeKind::Deploy)
        })
    }

    /// The kind of node `label` names, for rendering.
    pub fn is_project(&self, label: &str) -> bool {
        label
            .strip_prefix("project ")
            .is_some_and(|name| self.projects.contains_key(name))
    }

    /// Entities no reference points at, sorted. Not a verdict that they are unused: see
    /// `unused` for that.
    pub fn unreferenced(&self) -> Vec<EntityKey> {
        self.entities
            .iter()
            .filter(|(_, node)| {
                self.graph
                    .edges_directed(**node, Direction::Incoming)
                    .next()
                    .is_none()
            })
            .map(|(key, _)| key.clone())
            .collect()
    }
}
