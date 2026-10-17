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
    let recorded = check_handoff(solution, name, files)?;
    let directory = handoff_root(solution).join(name);
    // Take this before making `.twaco/handoffs` itself visible to `git status`.
    let commit = git_text(&solution.root, ["rev-parse", "HEAD"]);
    let dirty =
        git_text(&solution.root, ["status", "--porcelain"]).is_some_and(|text| !text.is_empty());
    // Written beside its final place and renamed into it, so an interruption leaves a hidden
    // folder `handoffs` skips rather than a handoff without its `handoff.json`.
    let staging = handoff_root(solution).join(format!(".{name}.partial"));
    let io = |path: &Path, error: std::io::Error| AdoptError::Repository {
        path: path.to_path_buf(),
        why: error.to_string(),
    };
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|error| io(&staging, error))?;
    }
    std::fs::create_dir_all(&staging).map_err(|error| io(&staging, error))?;
    for (file, file_name) in files.iter().zip(&recorded) {
        std::fs::copy(file, staging.join(file_name)).map_err(|error| io(file, error))?;
    }
    let handoff = Handoff {
        name: name.to_string(),
        commit,
        dirty,
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        files: recorded,
    };
    let json = serde_json::to_vec_pretty(&handoff).expect("handoff is serializable");
    let metadata = staging.join("handoff.json");
    std::fs::write(&metadata, json).map_err(|error| io(&metadata, error))?;
    std::fs::rename(&staging, &directory).map_err(|error| io(&directory, error))?;
    Ok(handoff)
}

/// Everything `record_handoff` refuses, without writing anything: a plan of a recording runs it,
/// so a plan that passes can be recorded. Returns the file names the handoff will hold.
pub fn check_handoff(
    solution: &Solution,
    name: &str,
    files: &[PathBuf],
) -> Result<Vec<String>, AdoptError> {
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
    let mut recorded: Vec<String> = Vec::new();
    for file in files {
        Side::from_export(file)?;
        let Some(file_name) = file.file_name().and_then(|name| name.to_str()) else {
            return Err(AdoptError::Base {
                base: file.display().to_string(),
                why: "file has no usable name".to_string(),
            });
        };
        // One folder holds them all: a second file of the same name would replace the first.
        if recorded
            .iter()
            .any(|seen| seen.eq_ignore_ascii_case(file_name))
        {
            return Err(AdoptError::Base {
                base: name.to_string(),
                why: format!("two files are named {file_name}; rename one"),
            });
        }
        recorded.push(file_name.to_string());
    }
    Ok(recorded)
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
        // A recording still in progress, or one that was interrupted.
        if entry.file_name().to_string_lossy().starts_with('.') {
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
    // A revision is an argument git would otherwise read as an option (`--output=...`).
    if revision.starts_with('-') {
        return Err("a revision cannot start with '-'".to_string());
    }
    let commit = git_output(
        &git_root,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
    )
    .map_err(|_| "no such revision".to_string())?
    .trim()
    .to_string();
    // NUL-separated, so a name git would quote (non-ASCII, a tab) arrives as it is. Each entry is
    // `<mode> <type> <object>\t<name>`; a submodule is a `commit`, with no content here.
    let listed = git_bytes(&git_root, ["ls-tree", "-r", "-z", &commit])?;
    let (names, objects): (Vec<String>, Vec<String>) = listed
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (meta, name) = entry.split_once('\t')?;
            let mut fields = meta.split(' ');
            let (_mode, kind, object) = (fields.next()?, fields.next()?, fields.next()?);
            (kind == "blob").then(|| (name.to_string(), object.to_string()))
        })
        .unzip();
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    // Git says where the solution sits in its worktree; comparing paths ourselves fails where
    // the two spell one folder differently (a symlinked /var, a Windows short name).
    let prefix = git_output(&solution.root, ["rev-parse", "--show-prefix"])?;
    let solution_relative = PathBuf::from(prefix.trim().trim_end_matches('/'));
    let config_relative = solution_relative.join("twaco.toml");
    let config = config_relative.to_string_lossy().replace('\\', "/");
    if !names.iter().any(|name| name == &config) {
        return Err("revision has no twaco.toml for this solution".to_string());
    }
    let prefix = solution_relative.to_string_lossy().replace('\\', "/");
    let (wanted, wanted_objects): (Vec<&String>, Vec<&str>) = names
        .iter()
        .zip(&objects)
        .filter(|(name, _)| {
            *name == &config || prefix.is_empty() || name.starts_with(&format!("{prefix}/"))
        })
        .map(|(name, object)| (name, object.as_str()))
        .unzip();
    let contents = git_blobs(&git_root, &wanted_objects)?;
    for (name, bytes) in wanted.into_iter().zip(contents) {
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
    // Git names the file relative to its worktree, whatever spelling `path` uses.
    let (Some(folder), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
    else {
        return Vec::new();
    };
    let Ok(listed) = git_output(folder, ["ls-files", "--full-name", "--", name]) else {
        return Vec::new();
    };
    let relative = listed.lines().next().unwrap_or_default().trim().to_string();
    if relative.is_empty() {
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
        // A leading dot marks a recording in progress.
        && !name.starts_with('.')
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

/// The content of each blob, in order, from one `git cat-file --batch` rather than a process
/// per file.
fn git_blobs(directory: &Path, objects: &[&str]) -> Result<Vec<Vec<u8>>, String> {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut child = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["cat-file", "--batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    // Fed from a thread: git answers while it reads, and both pipes have finite buffers.
    let requests: String = objects.iter().map(|object| format!("{object}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(requests.as_bytes()));
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let read = (|| {
        let mut contents = Vec::with_capacity(objects.len());
        for object in objects {
            let mut header = String::new();
            stdout
                .read_line(&mut header)
                .map_err(|error| error.to_string())?;
            // `<object> blob <size>`, or `<object> missing`.
            let size = header
                .trim_end()
                .strip_prefix(&format!("{object} blob "))
                .and_then(|size| size.parse::<usize>().ok())
                .ok_or_else(|| format!("git cannot read object {object}: {}", header.trim()))?;
            let mut bytes = vec![0; size + 1];
            stdout
                .read_exact(&mut bytes)
                .map_err(|error| error.to_string())?;
            bytes.pop(); // the newline after each object
            contents.push(bytes);
        }
        Ok(contents)
    })();
    // Closing our end first lets a writer blocked on a full pipe fail and finish.
    drop(stdout);
    let _ = writer.join();
    let _ = child.kill();
    let _ = child.wait();
    read
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
