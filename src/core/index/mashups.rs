//! References from mashups.

use super::build::Builder;
use super::EdgeKind;
use crate::core::entity_key::EntityKey;
use crate::core::mashup;
use petgraph::graph::NodeIndex;
use serde_json::Value;
use std::collections::BTreeMap;

/// A mashup's collection names as the exporter writes them in an `EntityType`, singular or not.
fn collection_of(entity_type: &str) -> String {
    match entity_type {
        "Thing" => "Things",
        "ThingTemplate" => "ThingTemplates",
        "ThingShape" => "ThingShapes",
        "DataShape" => "DataShapes",
        other => other,
    }
    .to_string()
}

/// Every object in `value` that names an entity: `{ "EntityName": ..., "EntityType": ..., "Service": ... }`.
fn bindings(value: &Value, out: &mut Vec<(String, String, Option<String>)>) {
    match value {
        Value::Object(object) => {
            if let Some(name) = object.get("EntityName").and_then(Value::as_str) {
                let kind = object
                    .get("EntityType")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let service = object
                    .get("Service")
                    .and_then(Value::as_str)
                    .filter(|service| !service.is_empty());
                out.push((
                    kind.to_string(),
                    name.to_string(),
                    service.map(str::to_string),
                ));
            }
            for child in object.values() {
                bindings(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| bindings(item, out)),
        _ => {}
    }
}

/// Every string value (not key) in `value`.
fn strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Object(object) => object.values().for_each(|child| strings(child, out)),
        Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
        _ => {}
    }
}

impl Builder<'_> {
    /// An edge for every service or property a mashup binds to, and a review edge for every other
    /// string in it that equals an entity's qualified name.
    pub(super) fn add_mashup_edges(&mut self) {
        let names = self.qualified_names();
        let mashups: Vec<(EntityKey, NodeIndex)> = self
            .entities
            .iter()
            .filter(|(key, _)| key.collection() == "Mashups")
            .map(|(key, at)| (key.clone(), *at))
            .collect();
        for (_, from) in mashups {
            let Some(file) = self.graph[from].file.clone() else {
                continue;
            };
            let Ok(bytes) = std::fs::read(self.solution.root.join(&file)) else {
                continue;
            };
            let content = match mashup::extract(&bytes) {
                Ok(assets) => assets.content,
                Err(error) => {
                    self.note_unreadable(&format!("{}: {error}", file.display()));
                    continue;
                }
            };
            let value: Value = match serde_json::from_str(&content) {
                Ok(value) => value,
                Err(error) => {
                    self.note_unreadable(&format!(
                        "{}: mashup content is not JSON: {error}",
                        file.display()
                    ));
                    continue;
                }
            };
            let mut bound = Vec::new();
            bindings(&value, &mut bound);
            for (entity_type, name, service) in bound {
                self.bind(from, &entity_type, &name, service.as_deref(), &names);
            }
            let mut texts = Vec::new();
            strings(&value, &mut texts);
            for text in texts {
                self.mention(from, None, text, EdgeKind::MashupMention, &names);
            }
        }
    }

    /// The binding to `name`: in the collection the mashup says, else among the Things, else
    /// wherever a qualified name matches.
    fn bind(
        &mut self,
        from: NodeIndex,
        entity_type: &str,
        name: &str,
        service: Option<&str>,
        names: &BTreeMap<String, Vec<EntityKey>>,
    ) {
        let mut candidates: Vec<EntityKey> = Vec::new();
        let given = collection_of(entity_type);
        for collection in [given.as_str(), "Things"] {
            if let Ok(key) = EntityKey::new(collection, name) {
                if self.entities.contains_key(&key) {
                    candidates.push(key);
                    break;
                }
            }
        }
        if candidates.is_empty() {
            candidates = names.get(name).cloned().unwrap_or_default();
        }
        for key in candidates {
            self.link(
                from,
                key.collection(),
                key.name(),
                EdgeKind::MashupBinding,
                None,
                service,
            );
        }
    }
}
