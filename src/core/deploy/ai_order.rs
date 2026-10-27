//! One deploy that removes an MCP item an agent allows.
//!
//! The server refuses a namespace that drops an item an agent on it allows, and an agent that
//! allows an item its namespace lacks; a bundle imports namespaces before agents. So, before the
//! first import, each deployed agent whose server copy allows an item a deployed namespace drops
//! is imported as that copy without those allowances. That copy is valid against the namespace
//! on the server now and against the one the bundle brings, and the bundle then writes the agent
//! as the repository has it. An agent on the server outside the deploy that allows a dropped
//! item refuses the deploy: changing it is not this deploy's to do.

use super::super::agents::{self, Items};
use super::super::baseline::Baseline;
use super::super::entity::edit;
use super::super::entity::key::EntityKey;
use super::super::normalise;
use super::super::server_owned::Part;
use super::{DeployError, ProjectBundle, Remote};
use std::collections::{BTreeMap, BTreeSet};

/// The items of one namespace that an agent allows and a deploy drops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedItems {
    pub namespace: String,
    pub tools: Vec<String>,
    pub resources: Vec<String>,
    pub prompts: Vec<String>,
}

impl DroppedItems {
    fn new(namespace: &str, items: &Items) -> Self {
        Self {
            namespace: namespace.to_string(),
            tools: items.tools.iter().cloned().collect(),
            resources: items.resources.iter().cloned().collect(),
            prompts: items.prompts.iter().cloned().collect(),
        }
    }
}

impl std::fmt::Display for DroppedItems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        for (kind, names) in [
            ("tool", &self.tools),
            ("resource", &self.resources),
            ("prompt", &self.prompts),
        ] {
            if !names.is_empty() {
                parts.push(format!("{kind} {}", names.join(", ")));
            }
        }
        write!(f, "{} from {}", parts.join("; "), self.namespace)
    }
}

/// An agent imported before the bundles, without the allowances its namespaces drop; or, in
/// [`DeployError::AgentsOutside`], one outside the deploy that allows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentNarrowing {
    pub name: String,
    /// The project whose bundle writes the agent as the repository has it; empty for an agent
    /// outside the deploy.
    pub project: String,
    pub dropped: Vec<DroppedItems>,
}

/// An [`AgentNarrowing`] with the documents it imports: the narrowed copy, and the server's own
/// to put back when the deploy fails before the agent or its namespaces are written.
pub(crate) struct Interim {
    pub(crate) narrowing: AgentNarrowing,
    pub(crate) narrowed: Vec<u8>,
    pub(crate) original: Vec<u8>,
    /// The projects holding the namespaces that drop its items.
    pub(crate) namespace_projects: BTreeSet<String>,
    pub(crate) server_owned: Vec<Part>,
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
    // Namespace -> (what it drops, the project that writes it).
    let mut dropped: BTreeMap<String, (Items, String)> = BTreeMap::new();
    for (project, entity) in entities().filter(|(_, entity)| entity.collection == "MCPNamespaces") {
        let server_error = server_error(&entity.collection, &entity.name);
        let local = agents::namespace(&entity.bytes)
            .map_err(|why| DeployError::Working {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                why,
            })?
            .items();
        // The Exporter's copy: the REST read is a bare element, which the reader refuses.
        let Some(server) = export(remote, &entity.collection, &entity.name)? else {
            continue;
        };
        let gone = agents::namespace(&server)
            .map_err(server_error)?
            .items()
            .without(&local);
        if !gone.is_empty() {
            dropped.insert(entity.name.clone(), (gone, project.project.clone()));
        }
    }
    if dropped.is_empty() {
        return Ok(Vec::new());
    }

    let deployed: BTreeMap<&str, (&str, &super::Entity)> = entities()
        .filter(|(_, entity)| entity.collection == "AIAgents")
        .map(|(project, entity)| (entity.name.as_str(), (project.project.as_str(), entity)))
        .collect();
    // Every agent on the server, since one outside the deploy blocks the namespace as well.
    let mut names: BTreeSet<String> = remote
        .list_entity_names("AIAgents")
        .map_err(|error| DeployError::Server {
            collection: "AIAgents".to_string(),
            name: "(listing)".to_string(),
            why: error.to_string(),
        })?
        .into_iter()
        .collect();
    names.extend(deployed.keys().map(|name| name.to_string()));

    let mut out = Vec::new();
    let mut outside = Vec::new();
    for name in names {
        let label = name.clone();
        let server_error = server_error("AIAgents", &label);
        // The Exporter's copy, which imports as it is.
        let Some(original) = export(remote, "AIAgents", &name)? else {
            continue;
        };
        let agent = agents::agent(&original).map_err(&server_error)?;
        let mut hits = Vec::new();
        let mut namespace_projects = BTreeSet::new();
        for reference in &agent.namespaces {
            let Some((gone, project)) = dropped.get(&reference.name) else {
                continue;
            };
            let both = reference.items().both(gone);
            if !both.is_empty() {
                hits.push(DroppedItems::new(&reference.name, &both));
                namespace_projects.insert(project.clone());
            }
        }
        if hits.is_empty() {
            continue;
        }
        let Some((project, entity)) = deployed.get(name.as_str()) else {
            outside.push(AgentNarrowing {
                name,
                project: String::new(),
                dropped: hits,
            });
            continue;
        };
        let wanted: BTreeMap<String, Items> = dropped
            .iter()
            .map(|(namespace, (items, _))| (namespace.clone(), items.clone()))
            .collect();
        let narrowed = edit::agent_without_allowances(&name, original.clone(), &wanted)
            .map_err(|error| server_error(error.to_string()))?
            .ok_or_else(|| server_error("its allowances could not be narrowed".to_string()))?;
        out.push(Interim {
            narrowing: AgentNarrowing {
                name,
                project: project.to_string(),
                dropped: hits,
            },
            narrowed,
            original,
            namespace_projects,
            server_owned: entity.server_owned.clone(),
        });
    }
    if !outside.is_empty() {
        return Err(DeployError::AgentsOutside(outside));
    }
    Ok(out)
}

fn server_error<'a>(collection: &'a str, name: &'a str) -> impl Fn(String) -> DeployError + 'a {
    move |why| DeployError::Server {
        collection: collection.to_string(),
        name: name.to_string(),
        why,
    }
}

fn export(
    remote: &dyn Remote,
    collection: &str,
    name: &str,
) -> Result<Option<Vec<u8>>, DeployError> {
    let error = server_error(collection, name);
    let key = EntityKey::address(collection, name).map_err(|e| error(e.to_string()))?;
    remote.export_owned(&key).map_err(|e| error(e.to_string()))
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

/// After a failed import, each narrowed agent whose own bundle did not import: put back when no
/// namespace it was narrowed for imported either. Otherwise the server's namespace no longer has
/// the items, so the narrowed copy is the one that holds; the baseline takes its server side, so
/// the next deploy writes the agent rather than calling it someone else's change.
pub(crate) fn after_failed_import(
    remote: &dyn Remote,
    narrowed: &[&Interim],
    imported: &[String],
    baseline: &mut Baseline,
) -> Box<[String]> {
    let mut notes = Vec::new();
    let mut back = Vec::new();
    for &interim in narrowed {
        if imported.contains(&interim.narrowing.project) {
            continue;
        }
        if !interim
            .namespace_projects
            .iter()
            .any(|project| imported.contains(project))
        {
            back.push(interim);
            continue;
        }
        let name = &interim.narrowing.name;
        let recorded = EntityKey::address("AIAgents", name)
            .map_err(|error| error.to_string())
            .and_then(|key| remote.fetch(&key).map_err(|error| error.to_string()))
            .and_then(|bytes| {
                let bytes = bytes.ok_or("it is no longer on the server".to_string())?;
                normalise::hash_with_parts(&bytes, &interim.server_owned)
                    .map_err(|error| error.to_string())
            })
            .map(|hash| baseline.set_server("AIAgents", name, hash).is_ok());
        let dropped: Vec<String> = interim
            .narrowing
            .dropped
            .iter()
            .map(ToString::to_string)
            .collect();
        notes.push(match recorded {
            Ok(true) => format!(
                "AIAgents/{name} stays without {}, which its namespace no longer has; the baseline records that",
                dropped.join("; ")
            ),
            Ok(false) => format!(
                "AIAgents/{name} stays without {}, which its namespace no longer has",
                dropped.join("; ")
            ),
            Err(why) => format!(
                "AIAgents/{name} stays without {}, which its namespace no longer has; reading it back failed: {why}",
                dropped.join("; ")
            ),
        });
    }
    let mut out: Vec<String> = put_back(remote, &back).into_vec();
    out.extend(notes);
    out.into_boxed_slice()
}
