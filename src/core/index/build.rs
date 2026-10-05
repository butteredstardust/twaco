//! Reading a solution into an [`Index`].

use super::{Edge, EdgeKind, Index, Node, NodeKind, Unreadable};
use crate::core::config::Solution;
use crate::core::datashape::Aspect;
use crate::core::entity_key::EntityKey;
use crate::core::types::{self, Member, TypedValue};
use crate::core::{scan, sidecar, workspace};
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

impl Index {
    /// Read every entity document and the configuration of `solution` once.
    ///
    /// A document that cannot be read is listed in [`Index::unreadable`] and left out; it never
    /// stops the build, so the answer is as complete as the repository allows and says where it
    /// is not.
    pub fn build(solution: &Solution) -> Index {
        let (model, skipped) = types::load_model(solution);
        let mut builder = Builder {
            solution,
            model,
            skipped,
            graph: DiGraph::new(),
            entities: BTreeMap::new(),
            projects: BTreeMap::new(),
            unreadable: Vec::new(),
            unparsed: Vec::new(),
        };
        builder.add_projects();
        builder.add_entities();
        builder.add_structural_edges();
        builder.add_script_edges();
        builder.add_mashup_edges();
        builder.add_deploy_edges();
        builder.finish()
    }
}

pub(super) struct Builder<'a> {
    pub(super) solution: &'a Solution,
    /// Read once: the entity model and what it could not read.
    pub(super) model: types::Model,
    pub(super) skipped: Vec<String>,
    pub(super) graph: DiGraph<Node, Edge>,
    pub(super) entities: BTreeMap<EntityKey, NodeIndex>,
    pub(super) projects: BTreeMap<String, NodeIndex>,
    pub(super) unreadable: Vec<Unreadable>,
    pub(super) unparsed: Vec<String>,
}

/// What the entity model says about one entity, for the node that stands for it.
#[derive(Default)]
struct Declared {
    template: Option<String>,
    shapes: Vec<String>,
    services: BTreeSet<String>,
}

impl Builder<'_> {
    pub(super) fn note_unreadable(&mut self, message: &str) {
        // Discovery and the model report `path: reason`; keep them apart when they say so.
        let (what, why) = message.split_once(": ").unwrap_or((message, ""));
        let entry = Unreadable {
            what: what.to_string(),
            why: why.to_string(),
        };
        if !self.unreadable.contains(&entry) {
            self.unreadable.push(entry);
        }
    }

    fn add_projects(&mut self) {
        for project in &self.solution.projects {
            let at = self.graph.add_node(Node {
                kind: NodeKind::Project(project.name.clone()),
                project: project.name.clone(),
                file: None,
                template: None,
                shapes: Vec::new(),
                services: BTreeSet::new(),
                subscribes: false,
            });
            self.projects.insert(project.name.clone(), at);
        }
        for project in &self.solution.projects {
            let from = self.projects[&project.name];
            for dependency in &project.depends_on {
                if let Some(&to) = self.projects.get(dependency) {
                    if to != from {
                        self.graph.add_edge(
                            from,
                            to,
                            Edge {
                                kind: EdgeKind::ProjectDependency,
                                from_member: None,
                                to_member: None,
                                at: Some(crate::core::config::CONFIG_FILE.to_string()),
                            },
                        );
                    }
                }
            }
        }
    }

    /// Every entity whose name is qualified (contains a dot), by name. Short names such as `Run`
    /// would match any string, so a string is taken for an entity's name only when it is
    /// qualified.
    pub(super) fn qualified_names(&self) -> BTreeMap<String, Vec<EntityKey>> {
        let mut names: BTreeMap<String, Vec<EntityKey>> = BTreeMap::new();
        for key in self.entities.keys() {
            if crate::core::refs::is_qualified(key.name()) {
                names
                    .entry(key.name().to_string())
                    .or_default()
                    .push(key.clone());
            }
        }
        names
    }

    /// Whether any reference already goes from `from` (from `member`) to `to`.
    pub(super) fn referenced(&self, from: NodeIndex, to: NodeIndex, member: Option<&str>) -> bool {
        self.graph
            .edges_connecting(from, to)
            .any(|edge| edge.weight().from_member.as_deref() == member)
    }

    /// A review-level edge to every entity whose qualified name `text` equals, unless a reference
    /// from the same place to the same entity is already there: a string that merely names an
    /// entity says less than a binding or a `Things[...]` access that does.
    pub(super) fn mention(
        &mut self,
        from: NodeIndex,
        member: Option<&str>,
        text: &str,
        kind: EdgeKind,
        names: &BTreeMap<String, Vec<EntityKey>>,
    ) {
        let Some(keys) = names.get(text) else {
            return;
        };
        for key in keys {
            let Some(&to) = self.entities.get(key) else {
                continue;
            };
            if to == from || self.referenced(from, to, member) {
                continue;
            }
            self.link(from, key.collection(), key.name(), kind, member, None);
        }
    }

    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.solution.root)
            .unwrap_or(path)
            .to_path_buf()
    }

    fn add_entities(&mut self) {
        let discovered = workspace::discover(self.solution);
        for message in &discovered.unreadable {
            self.note_unreadable(message);
        }
        for message in std::mem::take(&mut self.skipped) {
            self.note_unreadable(&message);
        }
        let mut declared: BTreeMap<(String, String), Declared> = BTreeMap::new();
        for entity in &self.model.entities {
            let services = entity
                .members
                .iter()
                .filter_map(|member| match member {
                    Member::Service(service) => Some(service.name.clone()),
                    Member::Property(_) => None,
                })
                .collect();
            declared.insert(
                (entity.collection.clone(), entity.name.clone()),
                Declared {
                    template: entity.template.clone(),
                    shapes: entity.shapes.clone(),
                    services,
                },
            );
        }
        for file in discovered.entities {
            let key = match EntityKey::new(&file.info.collection, &file.info.name) {
                Ok(key) => key,
                Err(error) => {
                    self.unreadable.push(Unreadable {
                        what: self.relative(&file.path).display().to_string(),
                        why: format!("not an addressable entity: {error}"),
                    });
                    continue;
                }
            };
            if let Some(&existing) = self.entities.get(&key) {
                let first = self.graph[existing]
                    .file
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
                self.unreadable.push(Unreadable {
                    what: self.relative(&file.path).display().to_string(),
                    why: format!("{key} is also defined in {first}; the first is used"),
                });
                continue;
            }
            let own = declared
                .remove(&(file.info.collection.clone(), file.info.name.clone()))
                .unwrap_or_default();
            let at = self.graph.add_node(Node {
                kind: NodeKind::Entity(key.clone()),
                project: file.found_under.clone(),
                file: Some(self.relative(&file.path)),
                template: own.template,
                shapes: own.shapes,
                services: own.services,
                subscribes: false,
            });
            self.entities.insert(key, at);
        }
    }

    /// An edge from `from` to the entity `collection/name`, if the repository holds it. A name
    /// that is not here (a platform template, an extension's shape) names something the index
    /// cannot see, so there is nothing to point at.
    pub(super) fn link(
        &mut self,
        from: NodeIndex,
        collection: &str,
        name: &str,
        kind: EdgeKind,
        from_member: Option<&str>,
        to_member: Option<&str>,
    ) {
        let Ok(key) = EntityKey::new(collection, name) else {
            return;
        };
        let Some(&to) = self.entities.get(&key) else {
            return;
        };
        if to == from {
            return;
        }
        // A project has no file of its own: what it names is written in `twaco.toml`.
        let at = match &self.graph[from].file {
            Some(path) => Some(path.to_string_lossy().replace('\\', "/")),
            None => Some(crate::core::config::CONFIG_FILE.to_string()),
        };
        let edge = Edge {
            kind,
            from_member: from_member.map(str::to_string),
            to_member: to_member.map(str::to_string),
            at,
        };
        let present = self
            .graph
            .edges_connecting(from, to)
            .any(|existing| existing.weight() == &edge);
        if !present {
            self.graph.add_edge(from, to, edge);
        }
    }

    fn add_structural_edges(&mut self) {
        let model = std::mem::take(&mut self.model);
        let typed = |builder: &mut Self, from: NodeIndex, member: &str, value: &TypedValue| {
            if let Some(shape) = &value.data_shape {
                builder.link(
                    from,
                    "DataShapes",
                    shape,
                    EdgeKind::DataShape,
                    Some(member),
                    None,
                );
            }
        };
        for entity in &model.entities {
            let Ok(key) = EntityKey::new(&entity.collection, &entity.name) else {
                continue;
            };
            let Some(&from) = self.entities.get(&key) else {
                continue;
            };
            if let Some(template) = &entity.template {
                self.link(
                    from,
                    "ThingTemplates",
                    template,
                    EdgeKind::Template,
                    None,
                    None,
                );
            }
            for shape in &entity.shapes {
                self.link(
                    from,
                    "ThingShapes",
                    shape,
                    EdgeKind::ImplementedShape,
                    None,
                    None,
                );
            }
            for member in &entity.members {
                match member {
                    Member::Service(service) => {
                        typed(self, from, &service.name, &service.result);
                        for parameter in &service.parameters {
                            typed(self, from, &service.name, &parameter.value);
                        }
                    }
                    Member::Property(property) => {
                        typed(self, from, &property.name, &property.value)
                    }
                }
            }
        }
        for shape in &model.data_shapes {
            let Ok(key) = EntityKey::new("DataShapes", &shape.name) else {
                continue;
            };
            let Some(&from) = self.entities.get(&key) else {
                continue;
            };
            for field in &shape.fields {
                if let Some(Aspect::Text(target)) = field.aspects.get("dataShape") {
                    self.link(
                        from,
                        "DataShapes",
                        target,
                        EdgeKind::DataShape,
                        Some(&field.name),
                        None,
                    );
                }
            }
            if let Some(base) = self.base_data_shape(from) {
                self.link(
                    from,
                    "DataShapes",
                    &base,
                    EdgeKind::BaseDataShape,
                    None,
                    None,
                );
            }
        }
    }

    /// The `baseDataShape` a DataShape document declares; the entity model does not keep it.
    fn base_data_shape(&mut self, at: NodeIndex) -> Option<String> {
        let relative = self.graph[at].file.clone()?;
        let path = self.solution.root.join(&relative);
        let bytes = std::fs::read(&path).ok()?;
        let tokens = scan::tokenize(&bytes).ok()?;
        let entity = sidecar::entity_element(&tokens, &bytes)?;
        let span = scan::attribute(&bytes, &tokens[entity], "baseDataShape")
            .ok()
            .flatten()?;
        let value = scan::decode_entities(&String::from_utf8_lossy(span.of(&bytes)));
        (!value.is_empty()).then_some(value)
    }

    fn finish(mut self) -> Index {
        let deploy_order = match self.solution.deploy_order() {
            Ok(order) => order.iter().map(|project| project.name.clone()).collect(),
            Err(error) => {
                self.unreadable.push(Unreadable {
                    what: crate::core::config::CONFIG_FILE.to_string(),
                    why: error.to_string(),
                });
                Vec::new()
            }
        };
        self.unreadable
            .sort_by(|a, b| (&a.what, &a.why).cmp(&(&b.what, &b.why)));
        self.unparsed.sort();
        Index {
            graph: self.graph,
            entities: self.entities,
            projects: self.projects,
            deploy_order,
            unreadable: self.unreadable,
            unparsed: self.unparsed,
        }
    }
}
