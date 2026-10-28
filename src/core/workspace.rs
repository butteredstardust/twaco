//! Finding things on disk: which files are entities, which project owns them, and where each
//! one's sidecars live.
//!
//! Everything here is about layout rather than content. It is the layer that lets a command say
//! "sync Management_TS" without knowing which of a solution's projects holds it or which
//! collection folder it sits in.

use super::codes::{Coded, ErrorCode};
use super::config::{Project, Solution};
use super::entity::key::ServiceTarget;
use super::entity::{self, EntityInfo};
use super::sidecar::ServiceSidecar;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// The line ending a text mostly uses: CRLF when it has more of those than bare line feeds.
pub(crate) fn line_ending(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let bare = text.matches('\n').count() - crlf;
    if crlf > bare {
        "\r\n"
    } else {
        "\n"
    }
}

/// ThingWorx collection folder names, used when a project does not list its own.
///
/// A fixed list rather than "every subdirectory": a repository also holds `exported/`, `dist/`
/// and scratch trees full of entity documents that are not the project's source. It is every
/// collection an export can hold ([`bundle::COLLECTION_ORDER`](super::bundle::COLLECTION_ORDER))
/// and the folders source control files some Things and tables under; [`unread_folders`] names
/// what a project root holds outside them.
pub(crate) const KNOWN_COLLECTIONS: &[&str] = &[
    "AIAgents",
    "ApplicationKeys",
    "Authenticators",
    "Dashboards",
    "DataShapes",
    "DataTables",
    "DataTags",
    "DirectoryServices",
    "ExtensionPackages",
    "Groups",
    "LocalizationTables",
    "Localizations",
    "Logs",
    "MCPNamespaces",
    "MashupGadgets",
    "Mashups",
    "MediaEntities",
    "Menus",
    "ModelTags",
    "Networks",
    "NotificationContents",
    "NotificationDefinitions",
    "Organizations",
    "PermissionGroups",
    "PersistenceProviderPackages",
    "PersistenceProviders",
    "Projects",
    "QueueProviderPackages",
    "QueueProviders",
    "Resources",
    "Roles",
    "Schedulers",
    "ScriptFunctionLibraries",
    "StateDefinitions",
    "Streams",
    "StyleDefinitions",
    "StyleThemes",
    "Subsystems",
    "ThingGroups",
    "ThingPackages",
    "ThingShapes",
    "ThingTemplates",
    "Things",
    "Timers",
    "Users",
    "ValueStreams",
    "Widgets",
];

/// One entity file, and what it says about itself.
#[derive(Debug, Clone)]
pub struct EntityFile {
    pub path: PathBuf,
    pub info: EntityInfo,
    /// The project whose folders it was found under.
    pub found_under: String,
}

impl EntityFile {
    /// Whether the document agrees with the project it was filed under.
    pub fn is_misfiled(&self) -> bool {
        !self.info.project.is_empty() && self.info.project != self.found_under
    }
}

#[derive(Debug)]
pub enum WorkspaceError {
    UnknownEntity { name: String },
    UnknownCollection { name: String, found: Vec<String> },
    Ambiguous { name: String, found: Vec<String> },
    InvalidCallTarget { name: String },
    Io { path: PathBuf, why: String },
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkspaceError::UnknownEntity { name } => {
                write!(f, "no entity named {name} in this solution")
            }
            WorkspaceError::UnknownCollection { name, found } => {
                write!(
                    f,
                    "unknown collection {name}; use one of {}",
                    found.join(", ")
                )
            }
            WorkspaceError::Ambiguous { name, found } => {
                write!(f, "{name} is ambiguous; it could be {}", found.join(", "))
            }
            WorkspaceError::InvalidCallTarget { name } => {
                write!(
                    f,
                    "call target {name:?} must be a Thing name or Collection/Name"
                )
            }
            WorkspaceError::Io { path, why } => write!(f, "{}: {why}", path.display()),
        }
    }
}

impl std::error::Error for WorkspaceError {}

/// What a scan of the solution found, including what it could not read.
#[derive(Debug, Default)]
pub struct Discovery {
    pub entities: Vec<EntityFile>,
    /// Candidate files that could not be read or parsed. A repository is full of XML that is
    /// not an entity export, and that is silent by design -- but a file that *looks* like one
    /// and will not parse is a defect, not a thing to skip past.
    pub unreadable: Vec<String>,
}

/// Every entity file in a solution, in a stable order.
pub fn discover(solution: &Solution) -> Discovery {
    let mut found = Discovery::default();
    for project in &solution.projects {
        let (paths, failures) = entity_paths(solution, project);
        found.unreadable.extend(failures);
        for path in paths {
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    found.unreadable.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            match entity::parse(&bytes) {
                Ok(info) => found
                    .entities
                    .push(EntityFile { path, info, found_under: project.name.clone() }),
                // An export whose first entity is not where one is looked for (an empty first
                // collection) still holds entities: skipping it would lose them quietly.
                Err(entity::ParseFailureKind::NotAnEntity) if super::imports::entities_in_xml(&bytes).is_ok() => found
                    .unreadable
                    .push(format!("{}: an export whose first collection holds no entity; keep one entity per file", path.display())),
                // Not an entity export: a sidecar fragment, a localization table, someone's
                // notes. Expected, and not worth a word.
                Err(entity::ParseFailureKind::NotAnEntity) => {}
                Err(entity::ParseFailureKind::Scan(e)) => {
                    found.unreadable.push(format!("{}: {e}", path.display()))
                }
            }
        }
    }
    found.entities.sort_by(|a, b| a.path.cmp(&b.path));
    found.unreadable.sort();
    found
}

/// The stable project report shared by the command line and MCP adapters.
pub fn projects_json(solution: &Solution) -> Result<Value, super::config::ConfigError> {
    let order = solution.deploy_order()?;
    let found = discover(solution);
    let projects: Vec<Value> = order
        .iter()
        .map(|project| {
            let text = solution.relative(&solution.project_root(project));
            let text = text.trim_start_matches("./");
            let root = if text.is_empty() || text == "." {
                "."
            } else {
                text
            };
            json!({
                "name": project.name,
                "root": root,
                "depends_on": project.depends_on,
                "entities": found.entities.iter().filter(|entity| entity.found_under == project.name).count(),
            })
        })
        .collect();
    let misfiled: Vec<Value> = found
        .entities
        .iter()
        .filter(|entity| entity.is_misfiled())
        .map(|entity| {
            json!({
                "name": entity.info.name,
                "project": entity.info.project,
                "found_under": entity.found_under,
            })
        })
        .collect();
    Ok(json!({
        "solution": solution.solution.name,
        "projects": projects,
        "unreadable": found.unreadable,
        "misfiled": misfiled,
    }))
}

/// The entity files alone, for callers that have already dealt with the failures.
pub fn entities(solution: &Solution) -> Vec<EntityFile> {
    discover(solution).entities
}

/// Folders directly under a project root that hold entity documents discovery never reads,
/// each with how many: a misspelt or unknown collection, whose entities deploy and bundle would
/// leave out without a word. A project at the solution root or with its own `collections` is
/// not judged, nor are the sidecar and bundle folders, dot folders or another project's root.
pub(crate) fn unread_folders(solution: &Solution) -> Vec<(String, usize)> {
    let src = solution.src_root();
    let dist = solution.root.join(&solution.solution.dist);
    let roots: Vec<PathBuf> = solution
        .projects
        .iter()
        .map(|project| solution.project_root(project))
        .collect();
    let mut found = Vec::new();
    let mut judged = std::collections::BTreeSet::new();
    for project in &solution.projects {
        let root = solution.project_root(project);
        // Two projects may share a root; one look is enough.
        if !project.collections.is_empty() || root == solution.root || !judged.insert(root.clone())
        {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let candidates: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                // A folder discovery reads, in any case on a case-insensitive disk, and what
                // every walk skips (build output, dependencies) are not judged.
                !name.starts_with('.')
                    && !KNOWN_COLLECTIONS
                        .iter()
                        .any(|known| known.eq_ignore_ascii_case(&name))
                    && !SKIPPED_DIRS.contains(&name.as_ref())
                    && *path != src
                    && *path != dist
                    && !roots.iter().any(|other| other.starts_with(path))
            })
            .collect();
        if candidates.is_empty() {
            continue;
        }
        // The repository walk honours .gitignore, so an ignored export folder is not judged.
        let tree = walk_repository(&root);
        for folder in candidates {
            let entities = tree
                .files
                .iter()
                .filter(|file| {
                    file.starts_with(&folder)
                        && file.extension().is_some_and(|extension| extension == "xml")
                })
                .filter(|file| std::fs::read(file).is_ok_and(|bytes| entity::parse(&bytes).is_ok()))
                .count();
            if entities > 0 {
                found.push((solution.relative(&folder), entities));
            }
        }
    }
    found.sort();
    found
}

/// Candidate entity file paths for one project, and the folders that could not be read.
fn entity_paths(solution: &Solution, project: &Project) -> (Vec<PathBuf>, Vec<String>) {
    let root = solution.project_root(project);
    let wanted: Vec<&str> = if project.collections.is_empty() {
        KNOWN_COLLECTIONS.to_vec()
    } else {
        project.collections.iter().map(String::as_str).collect()
    };
    let mut out = Vec::new();
    let mut failures = Vec::new();
    for collection in wanted {
        // Recursive: a project is free to file entities in subfolders of a collection, and a
        // flat read would make those invisible rather than reporting them.
        collect_xml(&root.join(collection), &mut out, &mut failures);
    }
    // A project at `.` also sees every nested project folder while walking its collection
    // trees. The more specific configured root owns those files.
    out.retain(|path| {
        !solution.projects.iter().any(|other| {
            if other.name == project.name {
                return false;
            }
            let other_root = solution.project_root(other);
            other_root != root && other_root.starts_with(&root) && path.starts_with(other_root)
        })
    });
    out.sort();
    (out, failures)
}

/// Every `.xml` under `dir`. A link is reported rather than followed, so nothing outside the
/// project is read as its own; a folder that cannot be read is reported, not skipped. A
/// collection folder that does not exist is simply empty.
pub(crate) fn collect_xml(dir: &Path, out: &mut Vec<PathBuf>, failures: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            failures.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                failures.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        let path = entry.path();
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if kind.is_symlink() {
            failures.push(format!(
                "{}: a link; twaco does not follow links",
                path.display()
            ));
        } else if kind.is_dir() {
            collect_xml(&path, out, failures);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
        {
            out.push(path);
        }
    }
}

/// Resolve a full name, last dotted segment, or `Collection/Name`.
///
/// An ambiguous bare name is an error listing the candidates rather than a first-match win: two
/// projects in a solution can each have a `Manager`, and picking one silently would edit the
/// wrong file.
pub fn resolve<'a>(found: &'a [EntityFile], name: &str) -> Result<&'a EntityFile, WorkspaceError> {
    let (collection, entity_name) = match name.split_once('/') {
        Some((collection, entity_name)) => {
            if !KNOWN_COLLECTIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(collection))
                && !found
                    .iter()
                    .any(|e| e.info.collection.eq_ignore_ascii_case(collection))
            {
                let mut collections: Vec<String> =
                    KNOWN_COLLECTIONS.iter().map(|s| (*s).to_string()).collect();
                collections.extend(found.iter().map(|e| e.info.collection.clone()));
                collections.sort();
                collections.dedup();
                return Err(WorkspaceError::UnknownCollection {
                    name: collection.to_string(),
                    found: collections,
                });
            }
            (Some(collection), entity_name)
        }
        None => (None, name),
    };
    let pool: Vec<&EntityFile> = found
        .iter()
        .filter(|e| collection.is_none_or(|c| e.info.collection.eq_ignore_ascii_case(c)))
        .collect();
    let exact: Vec<&EntityFile> = pool
        .iter()
        .copied()
        .filter(|e| e.info.name == entity_name)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        return Err(WorkspaceError::Ambiguous {
            name: name.to_string(),
            found: exact.iter().map(|e| e.path.display().to_string()).collect(),
        });
    }
    // A bare name: match on the last dotted segment, the way a person refers to an entity.
    // Case-insensitively, because this runs on Windows and `management_ts` is what gets typed.
    let suffix: Vec<&EntityFile> = pool
        .iter()
        .copied()
        .filter(|e| {
            e.info
                .name
                .rsplit('.')
                .next()
                .is_some_and(|last| last.eq_ignore_ascii_case(entity_name))
        })
        .collect();
    match suffix.len() {
        0 => Err(WorkspaceError::UnknownEntity {
            name: name.to_string(),
        }),
        1 => Ok(suffix[0]),
        _ => Err(WorkspaceError::Ambiguous {
            name: name.to_string(),
            found: suffix.iter().map(|e| e.info.name.clone()).collect(),
        }),
    }
}

/// The `Collection/Name` a service call goes to, from what a person typed.
///
/// `Collection/Name` is taken as given. A name the solution holds, in full or by its last dotted
/// segment, is qualified with its own collection, so `Manager` reaches `Things/Acme.Manager` and
/// a template's service is not sent to a Thing. A name the solution does not hold is passed on
/// unchanged, for the platform's own Things and Resources; only an ambiguous one is refused.
/// The solution's entity wins a short name: a platform Thing of the same name is reached as
/// `Things/<Name>`, and the CLI says which target it resolved.
pub(crate) fn call_target(
    found: &[EntityFile],
    given: &str,
) -> Result<ServiceTarget, WorkspaceError> {
    if given.contains('/') {
        return ServiceTarget::parse(given).map_err(|_| WorkspaceError::InvalidCallTarget {
            name: given.to_string(),
        });
    }
    match resolve(found, given) {
        Ok(entity) => {
            ServiceTarget::entity(&entity.info.collection, &entity.info.name).map_err(|_| {
                WorkspaceError::InvalidCallTarget {
                    name: given.to_string(),
                }
            })
        }
        Err(WorkspaceError::UnknownEntity { .. }) => {
            ServiceTarget::parse(given).map_err(|_| WorkspaceError::InvalidCallTarget {
                name: given.to_string(),
            })
        }
        Err(error) => Err(error),
    }
}

/// Where one entity's service sidecars live.
pub(crate) fn services_dir(solution: &Solution, entity: &EntityFile) -> PathBuf {
    solution.src_root().join(&entity.info.name).join("services")
}

/// Where one DataShape's field sidecar lives.
pub(crate) fn fields_path(solution: &Solution, entity: &EntityFile) -> PathBuf {
    solution
        .src_root()
        .join(&entity.info.name)
        .join("fields.json")
}

/// Where one mashup's content and stylesheet live.
pub(crate) fn mashup_dir(solution: &Solution, entity: &EntityFile) -> PathBuf {
    solution.src_root().join(&entity.info.name).join("mashup")
}

/// Read one mashup's two sidecars.
///
/// `Ok(None)` means the mashup is not under management; an error means it is and the files
/// will not read. Those are different things, and collapsing them turned an unreadable
/// stylesheet into an intentionally empty one that a sync then wrote into the entity.
pub(crate) fn read_mashup(dir: &Path) -> Result<Option<super::mashup::Assets>, WorkspaceError> {
    let content = match std::fs::read_to_string(dir.join("content.json")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(WorkspaceError::Io {
                path: dir.join("content.json"),
                why: e.to_string(),
            })
        }
    };
    // A mashup with no stylesheet has no file. Any other failure is a failure.
    let css = match std::fs::read_to_string(dir.join("custom.css")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(WorkspaceError::Io {
                path: dir.join("custom.css"),
                why: e.to_string(),
            })
        }
    };
    Ok(Some(super::mashup::Assets {
        content: content.replace("\r\n", "\n"),
        css: css.replace("\r\n", "\n"),
    }))
}

/// Read one DataTable's configuration sidecar, telling "not managed" from "will not read".
pub(crate) fn read_datatable(path: &Path) -> Result<Option<String>, WorkspaceError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.replace("\r\n", "\n"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(WorkspaceError::Io {
            path: path.to_path_buf(),
            why: e.to_string(),
        }),
    }
}

/// Where one DataTable's configuration sidecar lives.
pub(crate) fn datatable_path(solution: &Solution, entity: &EntityFile) -> PathBuf {
    solution
        .src_root()
        .join(&entity.info.name)
        .join("datatable.json")
}

/// Read the committed sidecars for one entity.
///
/// Sidecars are normalised to LF in memory. They are written LF, but a checkout can convert
/// them, and a comparison that tripped over that would report every service as changed on a
/// machine configured differently from the one that wrote them.
pub(crate) fn read_sidecars(dir: &Path) -> BTreeMap<String, ServiceSidecar> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let service = entry.path();
        if !service.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let (Ok(definition), Ok(script)) = (
            std::fs::read_to_string(service.join("definition.xml")),
            std::fs::read_to_string(service.join("script.js")),
        ) else {
            continue;
        };
        out.insert(
            name.clone(),
            ServiceSidecar {
                name,
                definition: lf(&definition),
                script: lf(&script),
            },
        );
    }
    out
}

/// `text` with every line ending LF: CRLF, and a lone CR (a line ending in JavaScript too, and
/// what a CRLF conversion leaves on a last line with no newline of its own).
pub(crate) fn lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The sidecar folders under `dir` that no longer correspond to one of `services`.
///
/// Stale directories are named rather than deleted. Removing files is not something an extract
/// should decide on its own, and a sidecar left behind by a renamed service is worth a person's
/// attention. (Sidecars are written LF, whatever the entity XML around them uses: they are source
/// files an editor and a formatter work on, not fragments of the document.)
pub(crate) fn stale_sidecars(dir: &Path, services: &[ServiceSidecar]) -> Vec<String> {
    let wanted: std::collections::BTreeSet<&str> =
        services.iter().map(|s| s.name.as_str()).collect();
    let mut stale = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() && !wanted.contains(name.as_str()) && path.join("script.js").is_file()
            {
                stale.push(name);
            }
        }
    }
    stale.sort();
    stale
}

/// Replace a file's contents in one step, so an interrupted write cannot truncate it.
///
/// The temporary file sits in the same directory as its target, because a rename across volumes
/// is not atomic and may not be a rename at all.
fn write_lf(path: &Path, text: &str) -> Result<(), WorkspaceError> {
    let normalised = text.replace("\r\n", "\n");
    atomic_replace(path, normalised.as_bytes()).map_err(|e| WorkspaceError::Io {
        path: path.to_path_buf(),
        why: e.to_string(),
    })
}

/// Atomically write generated UTF-8 text with LF endings, but leave an identical file alone.
///
/// Generated editor files are watched by editors and build tools, so preserving the timestamp
/// on a fixed point is observable behaviour rather than just an optimisation.
pub(crate) fn write_lf_if_changed(path: &Path, text: &str) -> Result<bool, WorkspaceError> {
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    if matches!(std::fs::read(path), Ok(current) if current == normalised.as_bytes()) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WorkspaceError::Io {
            path: parent.to_path_buf(),
            why: e.to_string(),
        })?;
    }
    write_lf(path, &normalised)?;
    Ok(true)
}

/// Replace an entity document in one step. Same reasoning as `write_lf`, without the newline
/// normalisation: an entity's bytes are exactly what the splice produced.
pub fn write_entity(path: &Path, bytes: &[u8]) -> Result<(), WorkspaceError> {
    atomic_replace(path, bytes).map_err(|e| WorkspaceError::Io {
        path: path.to_path_buf(),
        why: e.to_string(),
    })
}

/// Replace a file's contents in one step: the one way twaco writes a file that may already exist.
///
/// The bytes go to a same-directory temporary (a rename across volumes is not atomic) named so
/// that no one could have prepared it and created new, because a link planted at a predictable
/// name would otherwise have the write land wherever it points. The temporary is synced, takes the
/// mode of the file it replaces (an executable bit, read-only), and is renamed over it; whatever
/// fails, it is removed. A crash leaves a hidden `*.twaco-tmp` that the workspace lock sweeps.
pub(crate) fn atomic_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(test)]
    crate::xml_oracle::check_write(path, bytes);
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(
        ".{}.{}-{nanos}-{}.twaco-tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut file| {
            // On Unix the temporary takes the old file's mode before it holds any byte: a
            // profile's password must never sit in a file others could read, even briefly.
            #[cfg(unix)]
            if let Ok(metadata) = std::fs::metadata(path) {
                file.set_permissions(metadata.permissions())?;
            }
            std::io::Write::write_all(&mut file, bytes).and_then(|()| file.sync_all())
        })
        .and_then(|()| {
            if let Ok(metadata) = std::fs::metadata(path) {
                let _ = std::fs::set_permissions(&temporary, metadata.permissions());
            }
            std::fs::rename(&temporary, path)
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    } else {
        sync_directory(parent);
    }
    result
}

/// Make a rename in `directory` durable. On Unix the new name is only on disk once the directory
/// is, so a power loss right after a write could otherwise bring back the old file. Best effort:
/// some filesystems refuse to sync a directory, and the file itself is already synced. Windows
/// has no such step.
fn sync_directory(directory: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = std::fs::File::open(directory) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = directory;
}

/// Directories a walk of the repository never enters: version control, editors, generated and
/// foreign trees, and twaco's own state (the baseline, profiles and generated types, among them
/// copies of every script made for `types --check`, which would report a script twice).
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".idea",
    ".vscode",
    "target",
    "dist",
    "build",
    "out",
    "coverage",
    "vendor",
    "node_modules",
    "distribution-backend",
    ".twaco",
];

/// What a walk found, each list in stable order. `unreadable` names what it could not read, so
/// a caller that must see everything (a gate) can say so rather than pass on part of the tree.
#[derive(Debug, Default)]
pub(crate) struct Tree {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) dirs: Vec<PathBuf>,
    /// Links (and anything else neither a file nor a folder), reported, never followed.
    pub(crate) links: Vec<PathBuf>,
    pub(crate) unreadable: Vec<String>,
}

/// Every file and directory of the repository at `root` that twaco's source checks consider.
///
/// The walk honours the repository's own `.gitignore` and `.ignore` (before `git init` too, as a
/// later commit would; not a parent folder's, the user's global excludes or `.git/info/exclude`,
/// none of which travel with the repository), skips [`SKIPPED_DIRS`], and never follows a link:
/// one can point out of the solution, or at an ancestor, and the walk would never end.
pub(crate) fn walk_repository(root: &Path) -> Tree {
    walk(
        ignore::WalkBuilder::new(root)
            .hidden(false)
            .require_git(false)
            .parents(false)
            .git_global(false)
            .git_exclude(false)
            .filter_entry(|entry| {
                let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                !(is_dir
                    && entry.depth() > 0
                    && SKIPPED_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()))
            }),
    )
}

/// Every file and directory under `root`, such as the sidecar tree. Nothing is skipped by name or
/// by an ignore file: sidecar folders are named for services, and a service may well be called
/// `build` or `out`. Links are not followed, as in [`walk_repository`].
pub(crate) fn walk_all(root: &Path) -> Tree {
    walk(
        ignore::WalkBuilder::new(root)
            .standard_filters(false)
            .hidden(false),
    )
}

fn walk(builder: &mut ignore::WalkBuilder) -> Tree {
    let mut tree = Tree::default();
    for entry in builder.build() {
        match entry {
            Ok(entry) if entry.depth() == 0 => {}
            Ok(entry) => match entry.file_type() {
                Some(kind) if kind.is_file() => tree.files.push(entry.into_path()),
                Some(kind) if kind.is_dir() => tree.dirs.push(entry.into_path()),
                // A link, or something that is neither: not followed, not read.
                _ => tree.links.push(entry.into_path()),
            },
            Err(error) => tree.unreadable.push(error.to_string()),
        }
    }
    tree.files.sort();
    tree.dirs.sort();
    tree.links.sort();
    tree
}

/// Every `script.js` under the sidecar root, for the formatter and the script gates, and what
/// could not be read there. An absent sidecar root holds no scripts and is not a failure.
pub(crate) fn script_files(solution: &Solution) -> Tree {
    let root = solution.src_root();
    if !root.exists() {
        return Tree::default();
    }
    let mut tree = walk_all(&root);
    tree.files
        .retain(|path| path.file_name().is_some_and(|name| name == "script.js"));
    tree.dirs.clear();
    tree
}

impl Coded for WorkspaceError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::UnknownEntity { .. } | Self::UnknownCollection { .. } => ErrorCode::UnknownEntity,
            Self::Ambiguous { .. } => ErrorCode::Ambiguous,
            Self::InvalidCallTarget { .. } => ErrorCode::InvalidArguments,
            Self::Io { .. } => ErrorCode::IoError,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity::EntityInfo;

    fn hidden_temporaries(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".twaco-tmp"))
            .count()
    }

    #[test]
    fn atomic_replace_creates_replaces_keeps_no_temporary_and_cleans_up_a_failed_rename() {
        let dir_guard = tempfile::Builder::new()
            .prefix("twaco-atomic-")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("a.txt");
        atomic_replace(&target, b"one").unwrap();
        atomic_replace(&target, b"two").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"two");
        assert_eq!(hidden_temporaries(&dir), 0);
        // A directory in the way makes the rename fail: the error is returned and nothing is left.
        let blocked = dir.join("blocked");
        std::fs::create_dir_all(blocked.join("inner")).unwrap();
        assert!(atomic_replace(&blocked, b"x").is_err());
        assert_eq!(hidden_temporaries(&dir), 0);
        assert!(blocked.join("inner").is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
            atomic_replace(&target, b"three").unwrap();
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o750
            );
        }
    }

    fn file(name: &str, project: &str, path: &str) -> EntityFile {
        EntityFile {
            path: PathBuf::from(path),
            info: EntityInfo {
                collection: "Things".to_string(),
                name: name.to_string(),
                project: project.to_string(),
            },
            found_under: project.to_string(),
        }
    }

    #[test]
    fn a_fully_qualified_name_resolves() {
        let found = vec![
            file("A.Manager", "A", "a.xml"),
            file("B.Manager", "B", "b.xml"),
        ];
        assert_eq!(
            resolve(&found, "A.Manager").unwrap().path,
            PathBuf::from("a.xml")
        );
    }

    #[test]
    fn collection_names_scope_full_and_short_entity_names() {
        let thing = file("Acme.Shared", "Acme.App", "thing.xml");
        let mut shape = thing.clone();
        shape.info.collection = "ThingShapes".into();
        shape.path = PathBuf::from("shape.xml");
        let found = [thing, shape];
        assert!(matches!(
            resolve(&found, "Acme.Shared"),
            Err(WorkspaceError::Ambiguous { .. })
        ));
        assert_eq!(
            resolve(&found, "Things/Acme.Shared").unwrap().path,
            PathBuf::from("thing.xml")
        );
        assert_eq!(
            resolve(&found, "things/shared").unwrap().path,
            PathBuf::from("thing.xml")
        );
        assert_eq!(
            resolve(&found, "ThingShapes/Shared").unwrap().path,
            PathBuf::from("shape.xml")
        );
        let unknown = resolve(&found, "Unknown/Acme.Shared")
            .unwrap_err()
            .to_string();
        assert!(
            unknown.contains("unknown collection Unknown")
                && unknown.contains("Things")
                && unknown.contains("ThingShapes")
        );
        let missing = resolve(&found, "Mashups/Acme.Shared")
            .unwrap_err()
            .to_string();
        assert!(missing.contains("Mashups/Acme.Shared"));
        assert!(resolve(&found[..1], "Shared").is_ok());
    }

    #[test]
    fn a_bare_name_resolves_when_only_one_project_has_it() {
        let found = vec![
            file("A.Manager", "A", "a.xml"),
            file("B.Other", "B", "b.xml"),
        ];
        assert_eq!(
            resolve(&found, "Manager").unwrap().path,
            PathBuf::from("a.xml")
        );
    }

    #[test]
    fn an_ambiguous_bare_name_lists_the_candidates_rather_than_guessing() {
        let found = vec![
            file("A.Manager", "A", "a.xml"),
            file("B.Manager", "B", "b.xml"),
        ];
        match resolve(&found, "Manager") {
            Err(WorkspaceError::Ambiguous { found, .. }) => {
                assert_eq!(found, vec!["A.Manager", "B.Manager"]);
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_name_says_so() {
        let found = vec![file("A.Manager", "A", "a.xml")];
        assert!(matches!(
            resolve(&found, "Ghost"),
            Err(WorkspaceError::UnknownEntity { .. })
        ));
    }

    #[test]
    fn a_call_target_is_qualified_from_the_solution_or_passed_on() {
        let mut template = file("Acme.Base_TT", "A", "t.xml");
        template.info.collection = "ThingTemplates".to_string();
        let found = vec![
            file("Acme.Manager", "A", "m.xml"),
            template,
            file("Acme.Other.Manager", "B", "o.xml"),
        ];
        assert_eq!(
            call_target(&found[..1], "Manager").unwrap().to_string(),
            "Things/Acme.Manager"
        );
        assert_eq!(
            call_target(&found, "Acme.Manager").unwrap().to_string(),
            "Things/Acme.Manager",
            "a full name is exact"
        );
        assert_eq!(
            call_target(&found, "Base_TT").unwrap().to_string(),
            "ThingTemplates/Acme.Base_TT"
        );
        assert_eq!(
            call_target(&found, "Resources/EntityServices")
                .unwrap()
                .to_string(),
            "Resources/EntityServices"
        );
        assert_eq!(
            call_target(&found, "PlatformThing").unwrap().to_string(),
            "PlatformThing",
            "not ours: as typed"
        );
        assert_eq!(
            call_target(&found[..1], "Things/Manager")
                .unwrap()
                .to_string(),
            "Things/Manager",
            "a platform namesake, explicitly"
        );
        assert!(matches!(
            call_target(&found, "Manager"),
            Err(WorkspaceError::Ambiguous { .. })
        ));
    }

    #[test]
    fn an_invalid_call_target_is_refused() {
        assert!(matches!(
            call_target(&[], "Things/../Users"),
            Err(WorkspaceError::InvalidCallTarget { .. })
        ));
    }

    #[test]
    fn a_bare_name_is_matched_regardless_of_case() {
        let found = vec![file("Acme.Management_TS", "A", "a.xml")];
        assert!(
            resolve(&found, "management_ts").is_ok(),
            "Windows users type what they see"
        );
    }

    #[test]
    fn a_misfiled_entity_is_recognised() {
        let mut entity = file("A.Manager", "A", "a.xml");
        assert!(!entity.is_misfiled());
        entity.found_under = "B".to_string();
        assert!(entity.is_misfiled());
    }

    #[test]
    fn an_entity_declaring_no_project_is_not_misfiled() {
        let mut entity = file("A.Manager", "", "a.xml");
        entity.found_under = "Anything".to_string();
        assert!(!entity.is_misfiled(), "undeclared is not the same as wrong");
    }

    #[test]
    fn every_exported_collection_is_read_and_an_unknown_folder_of_entities_is_named() {
        for collection in super::super::bundle::COLLECTION_ORDER {
            assert!(KNOWN_COLLECTIONS.contains(collection), "{collection}");
        }
        let dir = tempfile::Builder::new()
            .prefix("twaco-unread-")
            .tempdir()
            .unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("twaco.toml"),
            "[solution]\nsrc = \"P-SC/src\"\ndist = \"P-SC/dist\"\n\n\
             [[project]]\nname = \"P\"\nroot = \"P-SC\"\n\n\
             [[project]]\nname = \"Q\"\nroot = \"P-SC\"\n",
        )
        .unwrap();
        let entity = |collection: &str, tag: &str, name: &str| {
            format!("<Entities><{collection}><{tag} name=\"{name}\" projectName=\"P\"/></{collection}></Entities>")
        };
        for (folder, xml) in [
            (
                "P-SC/ThingGroups",
                entity("ThingGroups", "ThingGroup", "P.G"),
            ),
            ("P-SC/Thinsg", entity("Things", "Thing", "P.Lost")),
            ("P-SC/dist", entity("Things", "Thing", "P.Bundled")),
            ("P-SC/src", entity("Things", "Thing", "P.Fragment")),
            ("P-SC/.twaco", entity("Things", "Thing", "P.Backup")),
            ("P-SC/notes", "<notes/>".to_string()),
            // Ignored by git, a dependency folder, and a collection in another case.
            ("P-SC/exported", entity("Things", "Thing", "P.Exported")),
            (
                "P-SC/node_modules/x",
                entity("Things", "Thing", "P.Dependency"),
            ),
            (
                "P-SC/thingshapes",
                entity("ThingShapes", "ThingShape", "P.Cased"),
            ),
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            std::fs::write(root.join(folder).join("e.xml"), xml).unwrap();
        }
        std::fs::write(root.join("P-SC/.gitignore"), "exported/\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let names: Vec<_> = entities(&solution)
            .into_iter()
            .map(|file| file.info.name)
            .collect();
        assert!(names.contains(&"P.G".to_string()), "{names:?}");
        // Once, though two projects share the root.
        assert_eq!(unread_folders(&solution), [("P-SC/Thinsg".to_string(), 1)]);
        let gate = super::super::check::run(&solution)
            .gates
            .into_iter()
            .find(|gate| gate.name == "project")
            .unwrap();
        let unread: Vec<_> = gate
            .findings
            .iter()
            .filter(|finding| finding.rule == "unread-folder")
            .map(|finding| finding.file.as_str())
            .collect();
        assert_eq!(unread, ["P-SC/Thinsg"]);
    }
}
