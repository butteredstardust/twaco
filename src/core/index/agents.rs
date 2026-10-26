//! References from AI agents and MCP namespaces.

use super::build::Builder;
use super::EdgeKind;
use crate::core::agents::{self, collection};

impl Builder<'_> {
    pub(super) fn add_agent_edges(&mut self) {
        let entities: Vec<_> = self
            .entities
            .iter()
            .map(|(key, at)| (key.clone(), *at))
            .collect();
        for (key, from) in entities {
            let Some(file) = self.graph[from].file.clone() else {
                continue;
            };
            let Ok(bytes) = std::fs::read(self.solution.root.join(&file)) else {
                continue;
            };
            match key.collection() {
                "MCPNamespaces" => match agents::namespace(&bytes) {
                    Ok(namespace) => {
                        for reference in namespace.tools.into_iter().chain(
                            namespace
                                .resources
                                .iter()
                                .filter_map(|resource| resource.service.clone()),
                        ) {
                            if let Some(collection) = collection(&reference.provider_type) {
                                self.link(
                                    from,
                                    collection,
                                    &reference.provider,
                                    EdgeKind::McpTool,
                                    Some(&reference.name),
                                    Some(&reference.service),
                                );
                            }
                        }
                        for resource in namespace.resources {
                            if let Some(file_reference) = resource.file {
                                self.link(
                                    from,
                                    "Things",
                                    &file_reference.repository,
                                    EdgeKind::McpFile,
                                    Some(&resource.name),
                                    None,
                                );
                            }
                        }
                    }
                    Err(error) => self.note_unreadable(&format!("{}: {error}", file.display())),
                },
                "AIAgents" => match agents::agent(&bytes) {
                    Ok(agent) => {
                        for namespace in agent.namespaces {
                            self.link(
                                from,
                                "MCPNamespaces",
                                &namespace.name,
                                EdgeKind::AgentNamespace,
                                None,
                                None,
                            );
                        }
                        for resolver in agent.resolvers {
                            self.link(
                                from,
                                "Things",
                                &resolver,
                                EdgeKind::AgentConfig,
                                Some("resolver"),
                                None,
                            );
                        }
                        for (kind, name) in agent.taxonomy {
                            if let Some(collection) = collection(&kind) {
                                self.link(
                                    from,
                                    collection,
                                    &name,
                                    EdgeKind::AgentConfig,
                                    Some("taxonomy"),
                                    None,
                                );
                            }
                        }
                        for skill in agent.skills {
                            self.link(
                                from,
                                "Things",
                                &skill.repository,
                                EdgeKind::AgentConfig,
                                Some("skill"),
                                None,
                            );
                        }
                    }
                    Err(error) => self.note_unreadable(&format!("{}: {error}", file.display())),
                },
                _ => {}
            }
        }
    }
}
