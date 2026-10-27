//! The solution's Solution Framework building blocks: what the repository says about each, what
//! the framework says on a server, and the block's version.
//!
//! A block is a project whose `<project>.EntryPoint` Thing holds the framework's component
//! properties (`componentType`, `componentVersion`, ...). The repository's values are read with
//! the scanner, never by re-serialising. On a server the framework's own services answer:
//! `PTC.SolutionFramework.Manager.GetBuildingBlockList` (the Solution Framework page's list) and
//! `PTC.Base.Manager.GetComponentDeploymentOrder`; a block's entry point answers
//! `GetComponentDeployConfigurationParameters`, the parameters its `DeployComponent` takes.

use super::agents::child_text;
use super::config::Solution;
use super::entity::key::ServiceTarget;
use super::scan::{self, Kind, Span, Token};
use super::server::{Client, ServerError};
use super::workspace;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// One block as the repository has it.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Block {
    /// The project, and the block's name.
    pub name: String,
    /// `componentType`: Standard, Abstract, Implementation, UI, Test...
    #[serde(rename = "type")]
    pub kind: String,
    /// `componentVersion` on the entry point.
    pub version: String,
    /// The Project's `packageVersion`, which the framework's package creator writes too.
    pub project_version: Option<String>,
    /// `componentName`, the name the Solution Framework page shows.
    pub display_name: String,
    pub description: String,
    pub main_mashup: String,
    pub entry_point: String,
    /// The entry point's ThingTemplate.
    pub entry_point_template: String,
    /// The `ComponentManager` table's manager Thing.
    pub manager: Option<String>,
    /// `dependentComponents`: the blocks it needs.
    pub dependencies: Vec<String>,
    pub default_organization: String,
    pub default_group: String,
    /// `<block>.ComponentPermissionHelper`, when the repository has one.
    pub permission_helper: Option<String>,
    /// The entry point's file, relative to the solution.
    pub file: String,
    #[serde(skip)]
    pub(crate) entry_point_path: PathBuf,
    #[serde(skip)]
    pub(crate) project_path: Option<PathBuf>,
}

/// Every block of the solution, by name.
pub fn blocks(solution: &Solution) -> Vec<Block> {
    let found = workspace::discover(solution);
    let mut out = Vec::new();
    for file in &found.entities {
        if file.info.collection != "Things" {
            continue;
        }
        let Some(name) = file.info.name.strip_suffix(".EntryPoint") else {
            continue;
        };
        // The framework's entry point is `<project>.EntryPoint`, in that project.
        if name.is_empty() || file.info.project != name {
            continue;
        }
        let Ok(src) = std::fs::read(&file.path) else {
            continue;
        };
        let Some(mut block) = read_entry_point(&src) else {
            continue;
        };
        block.name = name.to_string();
        block.entry_point = file.info.name.clone();
        block.file = solution.relative(&file.path);
        block.entry_point_path = file.path.clone();
        let project = found
            .entities
            .iter()
            .find(|entity| entity.info.collection == "Projects" && entity.info.name == name);
        if let Some(project) = project {
            block.project_path = Some(project.path.clone());
            block.project_version = std::fs::read(&project.path)
                .ok()
                .and_then(|src| project_version(&src).map(|(version, _)| version));
        }
        let helper = format!("{name}.ComponentPermissionHelper");
        block.permission_helper = found
            .entities
            .iter()
            .any(|entity| entity.info.collection == "Things" && entity.info.name == helper)
            .then_some(helper);
        out.push(block);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Why [`find`] found no one block.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FindError {
    #[error("{0}")]
    Unknown(String),
    /// The name is the end of more than one block's.
    #[error("{0}")]
    Ambiguous(String),
}

impl super::codes::Coded for FindError {
    fn code(&self) -> super::codes::ErrorCode {
        match self {
            Self::Unknown(_) => super::codes::ErrorCode::UnknownEntity,
            Self::Ambiguous(_) => super::codes::ErrorCode::Ambiguous,
        }
    }
}

/// The block named `name`, or one whose name ends with `.name`.
pub fn find(solution: &Solution, name: &str) -> Result<Block, FindError> {
    let all = blocks(solution);
    if let Some(block) = all.iter().find(|block| block.name == name) {
        return Ok(block.clone());
    }
    let suffix = format!(".{name}");
    let matching: Vec<&Block> = all
        .iter()
        .filter(|block| block.name.ends_with(&suffix))
        .collect();
    match matching.as_slice() {
        [one] => Ok((*one).clone()),
        [] if all.is_empty() => Err(FindError::Unknown(format!(
            "the solution has no building block (a project with its <project>.EntryPoint Thing); {name} is not one"
        ))),
        [] => Err(FindError::Unknown(format!(
            "no building block is named {name}; the solution's are {}",
            names(&all)
        ))),
        _ => Err(FindError::Ambiguous(format!(
            "{name} could be {}; name one in full",
            names(&matching.into_iter().cloned().collect::<Vec<_>>())
        ))),
    }
}

fn names(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|block| block.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The framework's properties of an entry point Thing; `None` when it has no `componentType`,
/// which every block's entry point has.
fn read_entry_point(src: &[u8]) -> Option<Block> {
    let tokens = scan::tokenize(src).ok()?;
    let entity = super::sidecar::entity_element(&tokens, src)?;
    let properties = scan::child_tags(&tokens, src, "ThingProperties", entity)
        .into_iter()
        .next()?;
    let value = |property: &str| -> Option<String> {
        let at = scan::child_tags(&tokens, src, property, properties)
            .into_iter()
            .next()?;
        child_text(&tokens, src, "Value", at)
    };
    let kind = value("componentType")?;
    let manager = scan::child_tags(&tokens, src, "ConfigurationTables", entity)
        .into_iter()
        .flat_map(|tables| scan::child_tags(&tokens, src, "ConfigurationTable", tables))
        .find(|&table| {
            attribute(&tokens, src, table, "name").as_deref() == Some("ComponentManager")
        })
        .and_then(|table| {
            scan::tags_within(&tokens, src, "Row", table)
                .into_iter()
                .find_map(|row| child_text(&tokens, src, "managerThingName", row))
        })
        .filter(|manager| !manager.is_empty());
    let dependencies = scan::child_tags(&tokens, src, "dependentComponents", properties)
        .into_iter()
        .flat_map(|at| scan::tags_within(&tokens, src, "Row", at))
        .filter_map(|row| child_text(&tokens, src, "ComponentName", row))
        .filter(|name| !name.is_empty())
        .collect();
    Some(Block {
        kind,
        version: value("componentVersion").unwrap_or_default(),
        display_name: value("componentName").unwrap_or_default(),
        description: value("componentDescription").unwrap_or_default(),
        main_mashup: value("componentMainMashup").unwrap_or_default(),
        default_organization: value("componentDefaultOrg").unwrap_or_default(),
        default_group: value("componentDefaultUserGroup").unwrap_or_default(),
        entry_point_template: attribute(&tokens, src, entity, "thingTemplate").unwrap_or_default(),
        manager,
        dependencies,
        ..Block::default()
    })
}

fn attribute(tokens: &[Token], src: &[u8], at: usize, name: &str) -> Option<String> {
    scan::attribute(src, &tokens[at], name)
        .ok()
        .flatten()
        .map(|value| scan::decode_entities(&String::from_utf8_lossy(value.of(src))))
}

/// The Project's `packageVersion` and its span.
fn project_version(src: &[u8]) -> Option<(String, Span)> {
    let tokens = scan::tokenize(src).ok()?;
    let entity = super::sidecar::entity_element(&tokens, src)?;
    let span = scan::attribute(src, &tokens[entity], "packageVersion")
        .ok()
        .flatten()?;
    Some((String::from_utf8_lossy(span.of(src)).into_owned(), span))
}

/// The span of `componentVersion`'s value text, inside its CDATA section when it has one and
/// without the whitespace around it.
fn component_version_span(src: &[u8]) -> Option<Span> {
    let tokens = scan::tokenize(src).ok()?;
    let entity = super::sidecar::entity_element(&tokens, src)?;
    let properties = scan::child_tags(&tokens, src, "ThingProperties", entity)
        .into_iter()
        .next()?;
    let property = scan::child_tags(&tokens, src, "componentVersion", properties)
        .into_iter()
        .next()?;
    let value = scan::child_tags(&tokens, src, "Value", property)
        .into_iter()
        .next()?;
    let end = scan::element_end(&tokens, value)?;
    let inner: Vec<Span> = tokens[value + 1..end]
        .iter()
        .filter_map(|token| match token.kind {
            Kind::Cdata => Some(token.inner),
            Kind::Text if !token.span.of(src).iter().all(u8::is_ascii_whitespace) => {
                Some(token.span)
            }
            _ => None,
        })
        .collect();
    let [span] = inner.as_slice() else {
        return None;
    };
    let text = span.of(src);
    let start = text.iter().position(|b| !b.is_ascii_whitespace())?;
    let end = text.len() - text.iter().rev().position(|b| !b.is_ascii_whitespace())?;
    Some(Span::new(span.start + start, span.start + end))
}

// ---------------------------------------------------------------------------------------------
// The framework's view on a server

/// The framework service calls the server view makes. A trait so it can be tested without a
/// server; [`Client`] is the real one.
pub trait Remote {
    fn call(&self, thing: &str, service: &str, parameters: &Value) -> Result<Value, ServerError>;
}

impl Remote for Client {
    fn call(&self, thing: &str, service: &str, parameters: &Value) -> Result<Value, ServerError> {
        self.call_service(
            &ServiceTarget::Thing(thing.to_string()),
            service,
            parameters,
            Duration::from_secs(120),
        )
        .map(Option::unwrap_or_default)
    }
}

/// One block as the framework on the server has it.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct ServerBlock {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub version: String,
    /// `Configured`, `Not Configured` or `Error`.
    pub state: String,
    /// When the framework last deployed it, in milliseconds since the epoch.
    pub deployed: Option<i64>,
    pub description: String,
    /// Its place in the framework's deploy order, from 1.
    pub order: Option<usize>,
}

/// Every block the framework on the server knows, in its deploy order.
pub fn server_blocks(remote: &dyn Remote) -> Result<Vec<ServerBlock>, String> {
    let listed = remote
        .call(
            "PTC.SolutionFramework.Manager",
            "GetBuildingBlockList",
            &json!({ "filterCustomBlocks": false }),
        )
        .map_err(|error| {
            format!("the server's Solution Framework could not list its building blocks: {error}")
        })?;
    let order = remote
        .call(
            "PTC.Base.Manager",
            "GetComponentDeploymentOrder",
            &json!({}),
        )
        .map_err(|error| {
            format!("the server's PTC.Base could not give its deploy order: {error}")
        })?;
    let order: Vec<&str> = order
        .get("array")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let text = |row: &Value, field: &str| {
        row.get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut out: Vec<ServerBlock> = listed
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|row| {
            let name = text(row, "buildingBlockName");
            ServerBlock {
                order: order.iter().position(|item| *item == name).map(|at| at + 1),
                kind: text(row, "buildingBlockType"),
                version: text(row, "buildingBlockVersion"),
                state: text(row, "buildingBlockConfiguredState"),
                deployed: row.get("buildingBlockDeployDate").and_then(Value::as_i64),
                description: text(row, "buildingBlockDescription"),
                name,
            }
        })
        .collect();
    out.sort_by_key(|block| (block.order.unwrap_or(usize::MAX), block.name.clone()));
    Ok(out)
}

/// One parameter a block's `DeployComponent` takes, from its
/// `GetComponentDeployConfigurationParameters`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DeployParameter {
    pub name: String,
    pub types: Vec<String>,
    pub description: String,
    pub optional: bool,
}

/// The parameters the block's entry point says its deploy takes.
pub fn deploy_parameters(
    remote: &dyn Remote,
    entry_point: &str,
) -> Result<Vec<DeployParameter>, String> {
    let schema = remote
        .call(
            entry_point,
            "GetComponentDeployConfigurationParameters",
            &json!({}),
        )
        .map_err(|error| {
            format!("{entry_point}.GetComponentDeployConfigurationParameters: {error}")
        })?;
    Ok(parse_parameters(&schema))
}

pub(crate) fn parse_parameters(schema: &Value) -> Vec<DeployParameter> {
    let Some(fields) = schema.as_object() else {
        return Vec::new();
    };
    fields
        .iter()
        .map(|(name, field)| DeployParameter {
            name: name.clone(),
            types: field
                .get("types")
                .and_then(Value::as_array)
                .map(|types| {
                    types
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            description: field
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            optional: field
                .get("optional")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

/// A repository block beside the server's, and what differs.
#[derive(Clone, Debug, Serialize)]
pub struct Compared {
    pub block: Block,
    pub server: Option<ServerBlock>,
    /// What to look at: not on the server, another version there, not configured.
    pub notes: Vec<String>,
}

/// The repository's blocks against the server's.
pub fn compare(blocks: Vec<Block>, server: &[ServerBlock]) -> Vec<Compared> {
    blocks
        .into_iter()
        .map(|block| {
            let found = server.iter().find(|item| item.name == block.name).cloned();
            let mut notes = Vec::new();
            match &found {
                None => notes.push("not on the server".to_string()),
                Some(server) => {
                    if server.version != block.version {
                        notes.push(format!(
                            "the server has version {}, the repository {}",
                            if server.version.is_empty() {
                                "(none)"
                            } else {
                                &server.version
                            },
                            if block.version.is_empty() {
                                "(none)"
                            } else {
                                &block.version
                            }
                        ));
                    }
                    if server.state != "Configured" {
                        notes.push(format!(
                            "{} on the server: the framework has not run its deploy step",
                            if server.state.is_empty() {
                                "no state"
                            } else {
                                &server.state
                            }
                        ));
                    }
                }
            }
            Compared {
                block,
                server: found,
                notes,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The block's version

/// How `block version` changes the version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionChange {
    Bump(Bump),
    Set(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl Bump {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "patch" => Self::Patch,
            "minor" => Self::Minor,
            "major" => Self::Major,
            _ => return None,
        })
    }
}

/// The two files `block version` rewrites, and what it writes.
#[derive(Clone, Debug, Serialize)]
pub struct VersionPlan {
    pub block: String,
    pub from: String,
    pub to: String,
    /// The files, relative to the solution.
    pub files: Vec<String>,
    #[serde(skip)]
    pub(crate) writes: Vec<(PathBuf, Vec<u8>, Vec<u8>)>,
}

fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split('.').map(|part| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    });
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

/// The version change for `name`: the entry point's `componentVersion` and the Project's
/// `packageVersion` together, refused unless both are there and agree.
pub fn plan_version(
    solution: &Solution,
    name: &str,
    change: &VersionChange,
) -> Result<VersionPlan, String> {
    let block = find(solution, name).map_err(|error| error.to_string())?;
    let project_path = block.project_path.clone().ok_or_else(|| {
        format!(
            "{} has no Projects/{}.xml in the repository; its version is kept there and on the entry point",
            block.name, block.name
        )
    })?;
    let entry_src = std::fs::read(&block.entry_point_path)
        .map_err(|error| format!("{}: {error}", block.file))?;
    let project_src = std::fs::read(&project_path)
        .map_err(|error| format!("{}: {error}", solution.relative(&project_path)))?;
    let entry_span = component_version_span(&entry_src).ok_or_else(|| {
        format!(
            "{}: componentVersion has no single value to change",
            block.file
        )
    })?;
    let (project_version, project_span) = project_version(&project_src).ok_or_else(|| {
        format!(
            "{}: the Project has no packageVersion",
            solution.relative(&project_path)
        )
    })?;
    let current = String::from_utf8_lossy(entry_span.of(&entry_src)).into_owned();
    // A bump raises one version; with two that disagree it is not clear which. A version given
    // writes both.
    if current != project_version && matches!(change, VersionChange::Bump(_)) {
        return Err(format!(
            "{} says componentVersion {current} and the Project packageVersion {project_version}; make them agree first (giving the version writes both)",
            block.name
        ));
    }
    build_plan(
        solution,
        &block,
        &current,
        change,
        (&entry_src, entry_span),
        (&project_path, &project_src, project_span),
    )
}

fn build_plan(
    solution: &Solution,
    block: &Block,
    current: &str,
    change: &VersionChange,
    (entry_src, entry_span): (&[u8], Span),
    (project_path, project_src, project_span): (&PathBuf, &[u8], Span),
) -> Result<VersionPlan, String> {
    let to = match change {
        VersionChange::Set(version) => {
            parse_version(version)
                .ok_or_else(|| format!("{version:?} is not a version: write major.minor.patch"))?;
            version.clone()
        }
        VersionChange::Bump(bump) => {
            let [major, minor, patch] = parse_version(current).ok_or_else(|| {
                format!(
                    "{} has version {current:?}, which is not major.minor.patch; giving a version replaces it",
                    block.name
                )
            })?;
            let raised = match bump {
                Bump::Patch => patch.checked_add(1).map(|patch| [major, minor, patch]),
                Bump::Minor => minor.checked_add(1).map(|minor| [major, minor, 0]),
                Bump::Major => major.checked_add(1).map(|major| [major, 0, 0]),
            }
            .ok_or_else(|| format!("{current} cannot be raised further"))?;
            format!("{}.{}.{}", raised[0], raised[1], raised[2])
        }
    };
    if to == current {
        return Err(format!("{} is at {to} already", block.name));
    }
    let splice = |src: &[u8], span: Span| {
        let mut out = src[..span.start].to_vec();
        out.extend_from_slice(to.as_bytes());
        out.extend_from_slice(&src[span.end..]);
        out
    };
    let project_after = splice(project_src, project_span);
    let entry_after = splice(entry_src, entry_span);
    Ok(VersionPlan {
        block: block.name.clone(),
        from: current.to_string(),
        to,
        files: vec![solution.relative(project_path), block.file.clone()],
        writes: vec![
            (project_path.clone(), project_src.to_vec(), project_after),
            (
                block.entry_point_path.clone(),
                entry_src.to_vec(),
                entry_after,
            ),
        ],
    })
}

/// Write a [`VersionPlan`]: both files or neither, each refused if it changed since it was read.
pub(crate) fn apply_version(
    solution: &Solution,
    plan: &VersionPlan,
    lock: &super::lock::WorkspaceLock,
) -> Result<(), super::transaction::TransactionError> {
    let mut transaction = super::transaction::Transaction::new(&solution.root, "block version");
    for (path, before, after) in &plan.writes {
        transaction.replace_file(path, before, after.clone())?;
    }
    transaction.apply(lock)?;
    Ok(())
}

#[cfg(test)]
mod tests;
