//! One deploy that removes an MCP item an agent allows.
//!
//! The server refuses a namespace that drops an item an agent on it allows, and an agent that
//! allows an item its namespace lacks; a bundle imports namespaces before agents. So, before the
//! first import, each deployed agent whose server copy allows an item a deployed namespace drops
//! is imported as that copy without those allowances. That copy is valid against the namespace
//! on the server now and against the one the bundle brings, and the bundle then writes the agent
//! as the repository has it.

use super::super::agents::{self, Items};
use super::super::entity::edit;
use super::super::entity::key::EntityKey;
use super::{DeployError, ProjectBundle, Remote};
use std::collections::BTreeMap;

/// An agent imported before the bundles, without the allowances its namespaces drop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentNarrowing {
    pub name: String,
    /// The project whose bundle writes the agent as the repository has it.
    pub project: String,
    /// Per namespace, the items the server's agent allows and the deploy drops.
    pub dropped: Vec<(String, Vec<String>)>,
}

/// An [`AgentNarrowing`] with the documents it imports: the narrowed copy, and the server's own
/// to put back when the bundle that writes the agent does not import.
pub(crate) struct Interim {
    pub(crate) narrowing: AgentNarrowing,
    pub(crate) narrowed: Vec<u8>,
    pub(crate) original: Vec<u8>,
}

impl Interim {
    pub(crate) fn file_name(&self) -> String {
        format!("{}.xml", self.narrowing.name)
    }
}

/// The agents to narrow before the first import. Reads from the server; changes nothing.
pub(crate) fn plan(
    remote: &dyn Remote,
    projects: &[ProjectBundle],
) -> Result<Vec<Interim>, DeployError> {
    let entities = || {
        projects
            .iter()
            .flat_map(|project| project.entities.iter().map(move |entity| (project, entity)))
    };
    let mut dropped: BTreeMap<String, Items> = BTreeMap::new();
    for (_, entity) in entities().filter(|(_, entity)| entity.collection == "MCPNamespaces") {
        let server_error = |why: String| DeployError::Server {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            why,
        };
        let local = agents::namespace(&entity.bytes)
            .map_err(|why| DeployError::Working {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                why,
            })?
            .items();
        let key = EntityKey::address(&entity.collection, &entity.name)
            .map_err(|error| server_error(error.to_string()))?;
        // The Exporter's copy: the REST read is a bare element, which the reader refuses.
        let Some(server) = remote
            .export_owned(&key)
            .map_err(|error| server_error(error.to_string()))?
        else {
            continue;
        };
        let gone = agents::namespace(&server)
            .map_err(server_error)?
            .items()
            .without(&local);
        if !gone.is_empty() {
            dropped.insert(entity.name.clone(), gone);
        }
    }
    if dropped.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for (project, entity) in entities().filter(|(_, entity)| entity.collection == "AIAgents") {
        let server_error = |why: String| DeployError::Server {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            why,
        };
        let key = EntityKey::address(&entity.collection, &entity.name)
            .map_err(|error| server_error(error.to_string()))?;
        // The Exporter's copy, which imports as it is.
        let Some(original) = remote
            .export_owned(&key)
            .map_err(|error| server_error(error.to_string()))?
        else {
            continue;
        };
        let allowed = agents::agent(&original).map_err(server_error)?;
        let mut names = Vec::new();
        for reference in &allowed.namespaces {
            let Some(gone) = dropped.get(&reference.name) else {
                continue;
            };
            let allows = reference.items();
            let both: Vec<String> = allows
                .without(&allows.without(gone))
                .names()
                .cloned()
                .collect();
            if !both.is_empty() {
                names.push((reference.name.clone(), both));
            }
        }
        if names.is_empty() {
            continue;
        }
        let narrowed = edit::agent_without_allowances(&entity.name, original.clone(), &dropped)
            .map_err(|error| server_error(error.to_string()))?
            .ok_or_else(|| server_error("its allowances could not be narrowed".to_string()))?;
        out.push(Interim {
            narrowing: AgentNarrowing {
                name: entity.name.clone(),
                project: project.project.clone(),
                dropped: names,
            },
            narrowed,
            original,
        });
    }
    Ok(out)
}

/// Put back the server's own copy of each agent in `narrowed`: what to say about each.
pub(crate) fn put_back(remote: &dyn Remote, narrowed: &[&Interim]) -> Box<[String]> {
    narrowed
        .iter()
        .map(|interim| {
            let name = &interim.narrowing.name;
            match remote.import(&interim.file_name(), &interim.original) {
                Ok(()) => format!("AIAgents/{name} was put back as the server had it"),
                Err(error) => format!(
                    "AIAgents/{name} still lacks the allowances deploy took out first, and putting it back failed: {error}"
                ),
            }
        })
        .collect()
}
