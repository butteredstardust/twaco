//! References from service scripts.

use super::build::Builder;
use super::EdgeKind;
use crate::core::entity_key::EntityKey;
use crate::core::{scan, script, sidecar};
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
            // A Thing or template that declares subscriptions runs when an event reaches it.
            if let Ok(tokens) = scan::tokenize(&bytes) {
                self.graph[from].subscribes = tokens.iter().any(|token| {
                    matches!(token.kind, scan::Kind::Start | scan::Kind::Empty)
                        && token.name.of(&bytes) == b"Subscription"
                });
            }
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
                for member in &facts.members {
                    let Some(entity) = facts.thing_of(&member.receiver) else {
                        continue;
                    };
                    let to_member = String::from_utf8_lossy(member.property.of(bytes)).into_owned();
                    self.link(
                        from,
                        "Things",
                        entity,
                        EdgeKind::ScriptReference,
                        Some(service),
                        Some(&to_member),
                    );
                }
                // `Things["X"]` itself, wherever no member of X is reached from that occurrence:
                // the Thing is passed along or stored, so the whole entity is what is referred to.
                // Each occurrence is judged on its own: `Things["X"].Run()` consumes one, a
                // variable that holds one and has a member reached through it consumes one, and
                // an occurrence left over is a reference to every member.
                let mut occurrences: BTreeMap<String, usize> = BTreeMap::new();
                let mut consumed: BTreeMap<String, usize> = BTreeMap::new();
                let mut variables_used: BTreeSet<&str> = BTreeSet::new();
                for member in &facts.members {
                    match &member.receiver {
                        script::Receiver::Variable(name) if name == "Things" => {
                            let entity =
                                String::from_utf8_lossy(member.property.of(bytes)).into_owned();
                            *occurrences.entry(entity).or_default() += 1;
                        }
                        script::Receiver::Thing(entity) => {
                            *consumed.entry(entity.clone()).or_default() += 1;
                        }
                        script::Receiver::Variable(name) => {
                            if let Some(entity) = facts.thing_variables.get(name) {
                                if variables_used.insert(name.as_str()) {
                                    *consumed.entry(entity.clone()).or_default() += 1;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                for (entity, count) in &occurrences {
                    if *count > consumed.get(entity).copied().unwrap_or(0) {
                        self.link(
                            from,
                            "Things",
                            entity,
                            EdgeKind::ScriptReference,
                            Some(service),
                            None,
                        );
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
