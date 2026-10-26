//! Structural references held in entity XML outside the entity model: a value stream, the
//! DataShape of a data table or stream, a network's Things, a menu's mashups, and who belongs to
//! a group, an organization or a ThingGroup.

use super::build::Builder;
use super::EdgeKind;
use crate::core::agents::child_text;
use crate::core::scan::{self, Token};

fn attribute(tokens: &[Token], src: &[u8], at: usize, name: &str) -> Option<String> {
    scan::attribute(src, &tokens[at], name)
        .ok()
        .flatten()
        .map(|value| scan::decode_entities(&String::from_utf8_lossy(value.of(src))))
        .filter(|value| !value.is_empty())
}

/// Every `tag` element below `parent`, at any depth.
fn all(tokens: &[Token], src: &[u8], tag: &str, parent: usize) -> Vec<usize> {
    scan::tags_within(tokens, src, tag, parent)
}

/// The collection a membership's `type` names: a group or organization holds users and
/// groups, a ThingGroup Things and ThingGroups.
fn member_collection(kind: &str) -> Option<&'static str> {
    match kind {
        "User" => Some("Users"),
        "Group" => Some("Groups"),
        "Thing" => Some("Things"),
        "ThingGroup" => Some("ThingGroups"),
        _ => None,
    }
}

impl Builder<'_> {
    pub(super) fn add_link_edges(&mut self) {
        let entities: Vec<_> = self
            .entities
            .iter()
            .filter(|(key, _)| {
                matches!(
                    key.collection(),
                    "Things"
                        | "ThingTemplates"
                        | "Networks"
                        | "Menus"
                        | "Groups"
                        | "Organizations"
                        | "ThingGroups"
                )
            })
            .map(|(key, at)| (key.collection().to_string(), *at))
            .collect();
        for (collection, from) in entities {
            let Some(file) = self.graph[from].file.clone() else {
                continue;
            };
            let Ok(src) = std::fs::read(self.solution.root.join(&file)) else {
                continue;
            };
            let Ok(tokens) = scan::tokenize(&src) else {
                continue;
            };
            let Some(entity) = crate::core::sidecar::entity_element(&tokens, &src) else {
                continue;
            };
            match collection.as_str() {
                "Things" | "ThingTemplates" => {
                    if let Some(stream) = attribute(&tokens, &src, entity, "valueStream") {
                        self.link(from, "Things", &stream, EdgeKind::ValueStream, None, None);
                    }
                    // A data table's or stream's DataShape, in its Settings table.
                    let tables = scan::child_tags(&tokens, &src, "ConfigurationTables", entity);
                    for table in tables
                        .into_iter()
                        .flat_map(|list| {
                            scan::child_tags(&tokens, &src, "ConfigurationTable", list)
                        })
                        .filter(|&at| {
                            attribute(&tokens, &src, at, "name").as_deref() == Some("Settings")
                        })
                    {
                        for row in scan::child_tags(&tokens, &src, "Rows", table)
                            .into_iter()
                            .flat_map(|rows| scan::child_tags(&tokens, &src, "Row", rows))
                        {
                            if let Some(shape) = child_text(&tokens, &src, "dataShape", row)
                                .filter(|shape| !shape.is_empty())
                            {
                                self.link(
                                    from,
                                    "DataShapes",
                                    &shape,
                                    EdgeKind::DataShape,
                                    Some("dataShape"),
                                    None,
                                );
                            }
                        }
                    }
                }
                "Networks" => {
                    for connection in all(&tokens, &src, "Connection", entity) {
                        for end in ["from", "to"] {
                            if let Some(thing) = attribute(&tokens, &src, connection, end) {
                                self.link(
                                    from,
                                    "Things",
                                    &thing,
                                    EdgeKind::NetworkNode,
                                    None,
                                    None,
                                );
                            }
                        }
                    }
                }
                "Menus" => {
                    for item in all(&tokens, &src, "MenuItem", entity) {
                        if attribute(&tokens, &src, item, "linkType").as_deref() != Some("Mashup") {
                            continue;
                        }
                        if let Some(mashup) = attribute(&tokens, &src, item, "linkDestination") {
                            let title = attribute(&tokens, &src, item, "title");
                            self.link(
                                from,
                                "Mashups",
                                &mashup,
                                EdgeKind::MenuItem,
                                title.as_deref(),
                                None,
                            );
                        }
                    }
                }
                _ => {
                    // A group's, an organization unit's or a ThingGroup's members.
                    for member in all(&tokens, &src, "Member", entity) {
                        let kind = attribute(&tokens, &src, member, "type");
                        let (Some(collection), Some(name)) = (
                            kind.as_deref().and_then(member_collection),
                            attribute(&tokens, &src, member, "name"),
                        ) else {
                            continue;
                        };
                        self.link(from, collection, &name, EdgeKind::Member, None, None);
                    }
                }
            }
        }
    }
}
