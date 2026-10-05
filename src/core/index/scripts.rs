//! References from service scripts.

use super::build::Builder;
use super::EdgeKind;
use crate::core::entity_key::EntityKey;
use crate::core::{script, sidecar};
use petgraph::graph::NodeIndex;
use std::collections::{BTreeMap, BTreeSet};

impl Builder<'_> {
    /// An edge for every static reference a service script makes to another Thing, and a review
    /// edge for every string in it that equals an entity's qualified name.
    ///
    /// A script the parser refuses is listed as unparsed and searched for names as plain words,
    /// so its references are known only as review-level mentions.
    pub(super) fn add_script_edges(&mut self) {
        let names = self.qualified_names();
        let owners: Vec<(EntityKey, NodeIndex)> = self
            .entities
            .iter()
            .filter(|(key, _)| {
                matches!(
                    key.collection(),
                    "Things" | "ThingTemplates" | "ThingShapes"
                )
            })
            .map(|(key, at)| (key.clone(), *at))
            .collect();
        for (key, from) in owners {
            let Some(file) = self.graph[from].file.clone() else {
                continue;
            };
            let Ok(bytes) = std::fs::read(self.solution.root.join(&file)) else {
                continue;
            };
            // A document the entity model could not read is already listed as unreadable.
            let Ok(scripts) = sidecar::script_services(&bytes) else {
                continue;
            };
            for service in scripts {
                self.scan_script(&key, from, &service.name, &service.script, &names);
            }
        }
    }

    fn scan_script(
        &mut self,
        owner: &EntityKey,
        from: NodeIndex,
        service: &str,
        text: &str,
        names: &BTreeMap<String, Vec<EntityKey>>,
    ) {
        match script::parse(text.as_bytes()) {
            Ok(facts) => {
                let bytes = text.as_bytes();
                // `Things["X"].Run()` and `var t = Things["X"]; t.Run()` reach a member of X.
                let mut reached: BTreeSet<String> = BTreeSet::new();
                for member in &facts.members {
                    let Some(entity) = facts.thing_of(&member.receiver) else {
                        continue;
                    };
                    let to_member = String::from_utf8_lossy(member.property.of(bytes)).into_owned();
                    reached.insert(entity.to_string());
                    self.link(
                        from,
                        "Things",
                        entity,
                        EdgeKind::ScriptReference,
                        Some(service),
                        Some(&to_member),
                    );
                }
                // `Things["X"]` itself, when no member of X is reached from it in this script: the
                // Thing is passed along or stored, so the whole entity is what is referred to.
                for member in &facts.members {
                    if matches!(&member.receiver, script::Receiver::Variable(name) if name == "Things")
                    {
                        let entity =
                            String::from_utf8_lossy(member.property.of(bytes)).into_owned();
                        if !reached.contains(&entity) {
                            self.link(
                                from,
                                "Things",
                                &entity,
                                EdgeKind::ScriptReference,
                                Some(service),
                                None,
                            );
                        }
                    }
                }
                for string in &facts.strings {
                    self.mention(
                        from,
                        Some(service),
                        &string.value,
                        EdgeKind::ScriptMention,
                        names,
                    );
                }
            }
            Err(_) => {
                self.unparsed.push(format!("{owner}/{service}"));
                for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')) {
                    let word = word.trim_matches('.');
                    self.mention(from, Some(service), word, EdgeKind::ScriptMention, names);
                }
            }
        }
    }
}
