//! Bases for the adopt three-way comparison: delivered exports, repository revisions and
//! locally recorded handoffs.

use super::{export_entities, repository_entities, service_bodies, AdoptError, Element, EntityRef};
use crate::core::config::Solution;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One side of a comparison: entities by `Collection/Name`, and script bodies by entity and
/// service name.
#[derive(Clone, Debug, Default)]
#[allow(private_interfaces)] // Element deliberately remains an internal parsed representation.
pub struct Side {
    pub entities: BTreeMap<EntityRef, Element>,
    pub services: BTreeMap<(String, String), String>,
}

impl Side {
    pub fn from_export(path: &Path) -> Result<Self, AdoptError> {
        let entities = export_entities(path)?;
        let services = services_from(&entities);
        Ok(Self { entities, services })
    }

    /// Reads the entity XML, then lets an existing service sidecar be authoritative.
    pub fn from_solution(solution: &Solution) -> Result<Self, AdoptError> {
        let found = repository_entities(solution)?;
        let entities = found
            .iter()
            .map(|(entity, (element, _))| (entity.clone(), element.clone()))
            .collect();
        let mut services = services_from(&entities);
        for entity in entities.keys() {
            let services_root = solution.src_root().join(&entity.name).join("services");
            let sidecar_names = std::fs::read_dir(&services_root)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().join("script.js").is_file())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect::<Vec<_>>();
            for service in services
                .keys()
                .filter(|(name, _)| name == &entity.name)
                .map(|(_, service)| service.clone())
                .chain(sidecar_names)
                .collect::<Vec<_>>()
            {
                let sidecar = solution
                    .src_root()
                    .join(&entity.name)
                    .join("services")
                    .join(&service)
                    .join("script.js");
                if sidecar.is_file() {
                    let body = std::fs::read_to_string(&sidecar).map_err(|error| {
                        AdoptError::Repository {
                            path: sidecar.clone(),
                            why: error.to_string(),
                        }
                    })?;
                    services.insert((entity.name.clone(), service), body);
                }
            }
        }
        Ok(Self { entities, services })
    }
}

fn services_from(entities: &BTreeMap<EntityRef, Element>) -> BTreeMap<(String, String), String> {
    let mut services = BTreeMap::new();
    for (entity, element) in entities {
        for (service, body) in service_bodies(element) {
            services.insert((entity.name.clone(), service), body);
        }
    }
    services
}

/// Where a selected base was obtained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BaseSource {
    File(PathBuf),
    Revision(String),
    Handoff(String),
}

/// A comparison base and an explanation suitable for presenting in a report.
#[derive(Clone, Debug)]
pub struct Base {
    pub side: Side,
    pub label: String,
}

/// Durable metadata beside a recorded handoff's delivered XML files.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Handoff {
    pub name: String,
    pub commit: Option<String>,
    pub dirty: bool,
    pub created: u64,
    pub files: Vec<String>,
}

fn handoff_root(solution: &Solution) -> PathBuf {
    solution.root.join(".twaco").join("handoffs")
}

/// Copy delivered `<Entities>` documents under the solution so a later export can identify the
/// collaborator's starting point.  Names are directory components only, never paths.
pub fn record_handoff(
    solution: &Solution,
    name: &str,
    files: &[PathBuf],
) -> Result<Handoff, AdoptError> {
    if !plain_name(name) {
        return Err(AdoptError::Base {
            base: name.to_string(),
            why: "handoff names must be plain file names".to_string(),
        });
    }
    let directory = handoff_root(solution).join(name);
    if directory.exists() {
        return Err(AdoptError::AlreadyExists {
            name: name.to_string(),
        });
    }
    if files.is_empty() {
        return Err(AdoptError::Base {
            base: name.to_string(),
            why: "record at least one <Entities> document".to_string(),
        });
    }
    for file in files {
        Side::from_export(file)?;
    }
    // Take this before making `.twaco/handoffs` itself visible to `git status`.
    let commit = git_text(&solution.root, ["rev-parse", "HEAD"]);
    let dirty =
        git_text(&solution.root, ["status", "--porcelain"]).is_some_and(|text| !text.is_empty());
    std::fs::create_dir_all(&directory).map_err(|error| AdoptError::Repository {
        path: directory.clone(),
        why: error.to_string(),
    })?;
    let mut recorded = Vec::new();
    for file in files {
        let Some(file_name) = file.file_name().and_then(|name| name.to_str()) else {
            return Err(AdoptError::Base {
                base: file.display().to_string(),
                why: "file has no usable name".to_string(),
            });
        };
        let target = directory.join(file_name);
        std::fs::copy(file, &target).map_err(|error| AdoptError::Repository {
            path: file.clone(),
            why: error.to_string(),
        })?;
        recorded.push(file_name.to_string());
    }
    let handoff = Handoff {
        name: name.to_string(),
        commit,
        dirty,
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        files: recorded,
    };
    let json = serde_json::to_vec_pretty(&handoff).expect("handoff is serializable");
    std::fs::write(directory.join("handoff.json"), json).map_err(|error| {
        AdoptError::Repository {
            path: directory.join("handoff.json"),
            why: error.to_string(),
        }
    })?;
    Ok(handoff)
}

/// Recorded handoffs, newest first. Invalid entries are refused rather than silently ignored.
pub fn handoffs(solution: &Solution) -> Result<Vec<Handoff>, AdoptError> {
    let root = handoff_root(solution);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|error| AdoptError::Repository {
        path: root.clone(),
        why: error.to_string(),
    })? {
        let entry = entry.map_err(|error| AdoptError::Repository {
            path: root.clone(),
            why: error.to_string(),
        })?;
        if !entry
            .file_type()
            .map_err(|error| AdoptError::Repository {
                path: entry.path(),
                why: error.to_string(),
            })?
            .is_dir()
        {
            continue;
        }
        let metadata = entry.path().join("handoff.json");
        let bytes = std::fs::read(&metadata).map_err(|error| AdoptError::Repository {
            path: metadata.clone(),
            why: error.to_string(),
        })?;
        let handoff: Handoff =
            serde_json::from_slice(&bytes).map_err(|error| AdoptError::Repository {
                path: metadata,
                why: error.to_string(),
            })?;
        found.push(handoff);
    }
    found.sort_by_key(|handoff| std::cmp::Reverse(handoff.created));
    Ok(found)
}

/// Resolve an explicit base, or choose the recorded handoff with the most entities equal to the
/// export. A tie keeps `handoffs`' newest-first ordering.
pub fn resolve(
    solution: &Solution,
    source: Option<&str>,
    export: &Side,
) -> Result<Option<Base>, AdoptError> {
    let known = handoffs(solution)?;
    if let Some(source) = source {
        let file = Path::new(source);
        if file.is_file() {
            return Ok(Some(Base {
                side: Side::from_export(file)?,
                label: format!("file {}", file.display()),
            }));
        }
        if let Some(handoff) = known.iter().find(|handoff| handoff.name == source) {
            return Ok(Some(Base {
                side: handoff_side(solution, handoff)?,
                label: format!("handoff {}", handoff.name),
            }));
        }
        return revision(solution, source)
            .map(Some)
            .map_err(|error| AdoptError::Base {
                base: source.to_string(),
                why: format!(
                    "{error}; record a handoff or supply an export file (handoffs: {})",
                    known
                        .iter()
                        .map(|h| h.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
    }
    let mut selected: Option<(usize, Handoff, Side)> = None;
    for handoff in known {
        let side = handoff_side(solution, &handoff)?;
        let matches = export
            .entities
            .iter()
            .filter(|(entity, theirs)| {
                side.entities
                    .get(*entity)
                    .is_some_and(|base| super::entity_same(entity, theirs, base, &[]))
            })
            .count();
        if matches > 0 && selected.as_ref().is_none_or(|(best, _, _)| matches > *best) {
            selected = Some((matches, handoff, side));
        }
    }
    Ok(selected.map(|(matches, handoff, side)| Base {
        side,
        label: format!(
            "handoff {} (picked: {} of {} entities identical)",
            handoff.name,
            matches,
            export.entities.len()
        ),
    }))
}

fn handoff_side(solution: &Solution, handoff: &Handoff) -> Result<Side, AdoptError> {
    let directory = handoff_root(solution).join(&handoff.name);
    let mut merged = Side::default();
    for file in &handoff.files {
        let side = Side::from_export(&directory.join(file))?;
        merged.entities.extend(side.entities);
        merged.services.extend(side.services);
    }
    Ok(merged)
}

fn revision(solution: &Solution, revision: &str) -> Result<Base, String> {
    let git_root = git_root(&solution.root)
        .ok_or_else(|| "this solution is outside a git repository".to_string())?;
    let names = git_output(&git_root, ["ls-tree", "-r", "--name-only", revision])?;
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    let solution_relative = solution
        .root
        .strip_prefix(&git_root)
        .map_err(|_| "solution is outside its git worktree".to_string())?;
    let config_relative = solution_relative.join("twaco.toml");
    let config = config_relative.to_string_lossy().replace('\\', "/");
    if !names.lines().any(|name| name == config) {
        return Err("revision has no twaco.toml for this solution".to_string());
    }
    let prefix = solution_relative.to_string_lossy().replace('\\', "/");
    for name in names.lines().filter(|name| {
        name == &config || prefix.is_empty() || name.starts_with(&format!("{prefix}/"))
    }) {
        let bytes = git_bytes(&git_root, ["show", &format!("{revision}:{name}")])?;
        let target = temporary.path().join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(target, bytes).map_err(|error| error.to_string())?;
    }
    let historical = Solution::load(&temporary.path().join(config_relative))
        .map_err(|error| error.to_string())?;
    Ok(Base {
        side: Side::from_solution(&historical).map_err(|error| error.to_string())?,
        label: format!("revision {revision}"),
    })
}

/// Committed versions of one tracked repository file, newest first.
pub fn history_versions(solution: &Solution, path: &Path, limit: usize) -> Vec<Vec<u8>> {
    let Some(root) = git_root(&solution.root) else {
        return Vec::new();
    };
    let Ok(relative) = path.strip_prefix(&root) else {
        return Vec::new();
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    if git_output(&root, ["ls-files", "--error-unmatch", &relative]).is_err() {
        return Vec::new();
    }
    let limit = limit.min(50).to_string();
    let Ok(commits) = git_output(&root, ["log", "--format=%H", "-n", &limit, "--", &relative])
    else {
        return Vec::new();
    };
    commits
        .lines()
        .filter_map(|commit| git_bytes(&root, ["show", &format!("{commit}:{relative}")]).ok())
        .collect()
}

fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && Path::new(name).file_name().and_then(|part| part.to_str()) == Some(name)
        && !name.contains(['/', '\\'])
}

fn git_root(path: &Path) -> Option<PathBuf> {
    git_text(path, ["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn git_text<const N: usize>(directory: &Path, arguments: [&str; N]) -> Option<String> {
    git_output(directory, arguments)
        .ok()
        .map(|text| text.trim().to_string())
}

fn git_output<const N: usize>(directory: &Path, arguments: [&str; N]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn git_bytes<const N: usize>(directory: &Path, arguments: [&str; N]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
