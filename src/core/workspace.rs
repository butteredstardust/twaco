//! Finding things on disk: which files are entities, which project owns them, and where each
//! one's sidecars live.
//!
//! Everything here is about layout rather than content. It is the layer that lets a command say
//! "sync Management_TS" without knowing which of a solution's projects holds it or which
//! collection folder it sits in.

use super::config::{Project, Solution};
use super::entity::{self, EntityInfo};
use super::entity_key::ServiceTarget;
use super::sidecar::ServiceSidecar;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// The line ending a text mostly uses: CRLF when it has more of those than bare line feeds.
pub fn line_ending(text: &str) -> &'static str {
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
/// and scratch trees full of entity documents that are not the project's source.
const KNOWN_COLLECTIONS: &[&str] = &[
    "ApplicationKeys",
    "Dashboards",
    "DataShapes",
    "DataTables",
    "Groups",
    "Localizations",
    "MashupGadgets",
    "Mashups",
    "MediaEntities",
    "Menus",
    "ModelTags",
    "Networks",
    "Organizations",
    "Projects",
    "Resources",
    "Schedulers",
    "StateDefinitions",
    "StyleDefinitions",
    "StyleThemes",
    "Subsystems",
    "ThingShapes",
    "ThingTemplates",
    "Things",
    "Timers",
    "Users",
    "ValueStreams",
    "Streams",
    "Widgets",
    "MCPNamespaces",
    "AIAgents",
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

/// The entity files alone, for callers that have already dealt with the failures.
pub fn entities(solution: &Solution) -> Vec<EntityFile> {
    discover(solution).entities
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
    out.sort();
    (out, failures)
}

/// Every `.xml` under `dir`. A link is reported rather than followed, so nothing outside the
/// project is read as its own; a folder that cannot be read is reported, not skipped. A
/// collection folder that does not exist is simply empty.
fn collect_xml(dir: &Path, out: &mut Vec<PathBuf>, failures: &mut Vec<String>) {
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

/// Resolve one entity by name, accepting a bare name or a fully qualified one.
///
/// An ambiguous bare name is an error listing the candidates rather than a first-match win: two
/// projects in a solution can each have a `Manager`, and picking one silently would edit the
/// wrong file.
pub fn resolve<'a>(found: &'a [EntityFile], name: &str) -> Result<&'a EntityFile, WorkspaceError> {
    let exact: Vec<&EntityFile> = found.iter().filter(|e| e.info.name == name).collect();
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
    let suffix: Vec<&EntityFile> = found
        .iter()
        .filter(|e| {
            e.info
                .name
                .rsplit('.')
                .next()
                .is_some_and(|last| last.eq_ignore_ascii_case(name))
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
pub fn call_target(found: &[EntityFile], given: &str) -> Result<ServiceTarget, WorkspaceError> {
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
pub fn services_dir(solution: &Solution, entity: &EntityFile) -> PathBuf {
    solution.src_root().join(&entity.info.name).join("services")
}

/// Where one DataShape's field sidecar lives.
pub fn fields_path(solution: &Solution, entity: &EntityFile) -> PathBuf {
    solution
        .src_root()
        .join(&entity.info.name)
        .join("fields.json")
}

/// Write one DataShape's field sidecar.
pub fn write_fields(path: &Path, text: &str) -> Result<(), WorkspaceError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WorkspaceError::Io {
            path: parent.to_path_buf(),
            why: e.to_string(),
        })?;
    }
    write_lf(path, text)
}

/// Where one mashup's content and stylesheet live.
pub fn mashup_dir(solution: &Solution, entity: &EntityFile) -> PathBuf {
    solution.src_root().join(&entity.info.name).join("mashup")
}

/// Write one mashup's two sidecars.
pub fn write_mashup(dir: &Path, assets: &super::mashup::Assets) -> Result<(), WorkspaceError> {
    std::fs::create_dir_all(dir).map_err(|e| WorkspaceError::Io {
        path: dir.to_path_buf(),
        why: e.to_string(),
    })?;
    write_lf(&dir.join("content.json"), &assets.content)?;
    write_lf(&dir.join("custom.css"), &assets.css)
}

/// Read one mashup's two sidecars.
///
/// `Ok(None)` means the mashup is not under management; an error means it is and the files
/// will not read. Those are different things, and collapsing them turned an unreadable
/// stylesheet into an intentionally empty one that a sync then wrote into the entity.
pub fn read_mashup(dir: &Path) -> Result<Option<super::mashup::Assets>, WorkspaceError> {
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
pub fn read_datatable(path: &Path) -> Result<Option<String>, WorkspaceError> {
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
pub fn datatable_path(solution: &Solution, entity: &EntityFile) -> PathBuf {
    solution
        .src_root()
        .join(&entity.info.name)
        .join("datatable.json")
}

/// Write one DataTable's configuration sidecar.
pub fn write_datatable(path: &Path, text: &str) -> Result<(), WorkspaceError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WorkspaceError::Io {
            path: parent.to_path_buf(),
            why: e.to_string(),
        })?;
    }
    write_lf(path, text)
}

/// Read the committed sidecars for one entity.
///
/// Sidecars are normalised to LF in memory. They are written LF, but a checkout can convert
/// them, and a comparison that tripped over that would report every service as changed on a
/// machine configured differently from the one that wrote them.
pub fn read_sidecars(dir: &Path) -> BTreeMap<String, ServiceSidecar> {
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
                definition: definition.replace("\r\n", "\n"),
                script: script.replace("\r\n", "\n"),
            },
        );
    }
    out
}

/// Write one entity's sidecars, and report any that no longer correspond to a service.
///
/// Stale directories are named rather than deleted. Removing files is not something an extract
/// should decide on its own, and a sidecar left behind by a renamed service is worth a person's
/// attention.
///
/// Always LF, whatever the entity XML around them uses: these are source files an editor and a
/// formatter work on, not fragments of the document.
pub fn write_sidecars(
    dir: &Path,
    services: &[ServiceSidecar],
) -> Result<Vec<String>, WorkspaceError> {
    for service in services {
        let service_dir = dir.join(&service.name);
        std::fs::create_dir_all(&service_dir).map_err(|e| WorkspaceError::Io {
            path: service_dir.clone(),
            why: e.to_string(),
        })?;
        write_lf(&service_dir.join("definition.xml"), &service.definition)?;
        write_lf(&service_dir.join("script.js"), &service.script)?;
    }

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
    Ok(stale)
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
pub fn write_lf_if_changed(path: &Path, text: &str) -> Result<bool, WorkspaceError> {
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
pub fn atomic_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
    }
    result
}

/// Every `script.js` under the sidecar root, for the formatter to work on.
pub fn script_files(solution: &Solution) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.file_name().is_some_and(|n| n == "script.js") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&solution.src_root(), &mut out);
    out.sort();
    out
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
        let nonce = crate::test_nonce();
        let dir = std::env::temp_dir().join(format!("twaco-atomic-{}-{nonce}", std::process::id()));
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
        let _ = std::fs::remove_dir_all(dir);
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
}
