//! Backups of server entities, taken before twaco does something to the server that cannot be
//! undone from the repository: deleting an entity, or overwriting one that has changes the
//! repository has never seen (`--force`).
//!
//! A backup set is a folder `.twaco/backups/<stamp>/` holding each entity's Exporter XML, which the
//! server imports back as it is (verified on a live server: export, delete, import), and a
//! `backup.json` that says what and why. Only the newest [`KEEP`] sets are kept. `entity restore`
//! imports a set again, and plans unless told to apply.
//!
//! What an export holds is the entity as the server would export it: definitions, configuration,
//! permissions. Not held: persisted property values, DataTable rows, stream data and the contents
//! of a file repository.

use super::config::Solution;
use super::deploy;
use super::entity;
use super::entity_key::EntityKey;
use super::push::{self, Decision, Refusal};
use super::server::{Client, ServerError};
use super::workspace;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

pub const DIR: &str = ".twaco/backups";
const MANIFEST: &str = "backup.json";
/// How many sets are kept; older ones are removed when a new one is saved.
pub const KEEP: usize = 20;

/// Reading an entity's export, and importing one back.
pub trait Remote {
    /// The entity's Exporter XML, or `None` when the server does not have it.
    fn export(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError>;
    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), ServerError>;
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError>;
}

impl Remote for Client {
    fn export(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
        let bytes = self.export_xml(Some(key.collection()), Some(key.name()), None)?;
        // The Exporter answers 200 with an empty export for an entity it does not have.
        Ok(entity::parse(&bytes)
            .ok()
            .filter(|info| info.name == key.name())
            .map(|_| bytes))
    }

    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), ServerError> {
        self.import_entity(file_name, xml)
    }

    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        self.entity_exists(key)
    }
}

#[derive(Debug)]
pub enum BackupError {
    Remote {
        entity: String,
        why: ServerError,
    },
    Io {
        path: PathBuf,
        why: String,
    },
    /// The entity is on the server but its export holds nothing twaco can save.
    Unreadable {
        entity: String,
    },
    NoSuchSet {
        id: String,
    },
    /// What a forced push or deploy would overwrite could not be worked out.
    Plan(String),
    Invalid {
        path: PathBuf,
        why: String,
    },
}

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackupError::Remote { entity, why } => write!(f, "{entity}: {why}"),
            BackupError::Io { path, why } => write!(f, "{}: {why}", path.display()),
            BackupError::Unreadable { entity } => write!(f, "{entity}: the server's export of it is empty or not an entity"),
            BackupError::Plan(why) => write!(f, "could not work out what the forced write would overwrite, so nothing was changed: {why}"),
            BackupError::NoSuchSet { id } => write!(f, "no backup set {id} under {DIR}; `twaco entity restore` lists them"),
            BackupError::Invalid { path, why } => write!(f, "{}: not a backup set: {why}", path.display()),
        }
    }
}

impl std::error::Error for BackupError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub collection: String,
    pub name: String,
    /// The file holding it, relative to the set. Absent in sets saved before 0.1.5, which
    /// named files by the low byte of each character.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub created: String,
    pub reason: String,
    pub entities: Vec<Item>,
}

/// A saved set: where it is and what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Set {
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// A set's id: the local time to the second.
pub fn new_stamp() -> String {
    jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string()
}

fn root(solution: &Solution) -> PathBuf {
    solution.root.join(DIR)
}

/// A file name for an entity: names are plain, but nothing is trusted to be a path segment.
/// Every byte of anything else is written `%XX`, so two names never share a file.
fn file_stem(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// How sets saved before 0.1.5 named a file: the low byte of each character, so `é` (U+00E9)
/// and `ǩ` (U+01E9) collided. Read only, to restore such a set, whose manifest records no files.
fn legacy_file_stem(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32 & 0xFF)
            }
        })
        .collect()
}

/// The most a file name of a set holds before its `.xml`, well inside every filesystem's limit
/// once the collection folder and the set's own path are added.
const MAX_STEM: usize = 120;

/// Where each entity of a set goes, relative to the set: `<Collection>/<Name>.xml`, the name
/// cut to [`MAX_STEM`] and, where it would share a file with another (two names differing only
/// in case are one file on Windows and macOS, and a cut can make two the same), given `~2`,
/// `~3`... The manifest records the result, so a restore never has to work it out again.
fn file_names<'a>(keys: impl IntoIterator<Item = &'a EntityKey>) -> Vec<String> {
    let mut used = std::collections::BTreeSet::new();
    keys.into_iter()
        .map(|key| {
            let folder = file_stem(key.collection());
            let mut stem = file_stem(key.name());
            stem.truncate(MAX_STEM);
            let mut file = format!("{folder}/{stem}.xml");
            let mut n = 1;
            while !used.insert(file.to_lowercase()) {
                n += 1;
                file = format!("{folder}/{stem}~{n}.xml");
            }
            file
        })
        .collect()
}

/// The file of a set that holds an entity: as the manifest records it, or for a set saved
/// before manifests recorded files, as that version named it.
fn saved_path(set: &Set, item: &Item) -> Result<PathBuf, BackupError> {
    match &item.file {
        Some(file) => {
            // The manifest is a file anyone can edit: only plain names inside the set.
            let plain = Path::new(file)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)));
            if !plain {
                return Err(BackupError::Invalid {
                    path: set.dir.join(MANIFEST),
                    why: format!("{file:?} is not a file inside the set"),
                });
            }
            Ok(set.dir.join(file))
        }
        None => Ok(set
            .dir
            .join(legacy_file_stem(&item.collection))
            .join(format!("{}.xml", legacy_file_stem(&item.name)))),
    }
}

/// Save the server's current version of each entity that exists. Entities the server does not
/// have are left out (there is nothing to lose). Returns `None` when none existed.
pub fn save(
    remote: &dyn Remote,
    solution: &Solution,
    reason: &str,
    entities: &[EntityKey],
    stamp: &str,
) -> Result<Option<Set>, BackupError> {
    let mut fetched = Vec::new();
    for key in entities {
        let label = key.to_string();
        if let Some(bytes) = remote.export(key).map_err(|why| BackupError::Remote {
            entity: label.clone(),
            why,
        })? {
            fetched.push((key, bytes));
        } else if remote.exists(key).map_err(|why| BackupError::Remote {
            entity: label.clone(),
            why,
        })? {
            // It is there, but the export came back empty: refuse rather than delete or replace
            // something that was not saved.
            return Err(BackupError::Unreadable { entity: label });
        }
    }
    if fetched.is_empty() {
        return Ok(None);
    }
    let base = root(solution);
    let (id, dir) = unused_dir(&base, stamp);
    let io = |path: &Path, error: std::io::Error| BackupError::Io {
        path: path.to_path_buf(),
        why: error.to_string(),
    };
    let files = file_names(fetched.iter().map(|(key, _)| *key));
    for ((_, bytes), file) in fetched.iter().zip(&files) {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().expect("a backup file has a parent"))
            .map_err(|error| io(&dir, error))?;
        workspace::atomic_replace(&path, bytes).map_err(|error| io(&path, error))?;
    }
    let manifest = Manifest {
        created: stamp.to_string(),
        reason: reason.to_string(),
        entities: fetched
            .iter()
            .zip(files)
            .map(|((key, _), file)| Item {
                collection: key.collection().to_string(),
                name: key.name().to_string(),
                file: Some(file),
            })
            .collect(),
    };
    let mut text = serde_json::to_string_pretty(&manifest).expect("a manifest serialises");
    text.push('\n');
    let path = dir.join(MANIFEST);
    workspace::atomic_replace(&path, text.as_bytes()).map_err(|error| io(&path, error))?;
    rotate(solution, KEEP);
    Ok(Some(Set { id, dir, manifest }))
}

/// `base/<stamp>`, or `base/<stamp>-2`, ... when that exists.
fn unused_dir(base: &Path, stamp: &str) -> (String, PathBuf) {
    let mut id = stamp.to_string();
    let mut counter = 1;
    while base.join(&id).exists() {
        counter += 1;
        id = format!("{stamp}-{counter}");
    }
    let dir = base.join(&id);
    (id, dir)
}

/// Every set, oldest first.
pub fn list(solution: &Solution) -> Vec<Set> {
    let mut sets = Vec::new();
    let Ok(entries) = std::fs::read_dir(root(solution)) else {
        return sets;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if let Some(manifest) = std::fs::read_to_string(dir.join(MANIFEST))
            .ok()
            .and_then(|text| serde_json::from_str::<Manifest>(&text).ok())
        {
            sets.push(Set {
                id: entry.file_name().to_string_lossy().into_owned(),
                dir,
                manifest,
            });
        }
    }
    sets.sort_by(|a, b| a.id.cmp(&b.id));
    sets
}

/// Remove all but the newest `keep` sets. Only folders holding a manifest are ever removed.
pub fn rotate(solution: &Solution, keep: usize) {
    let sets = list(solution);
    for set in sets.iter().take(sets.len().saturating_sub(keep)) {
        let _ = std::fs::remove_dir_all(&set.dir);
    }
}

pub fn find(solution: &Solution, id: &str) -> Result<Set, BackupError> {
    list(solution)
        .into_iter()
        .find(|set| set.id == id)
        .ok_or_else(|| BackupError::NoSuchSet { id: id.to_string() })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Not applied: the server has this entity, and an apply would replace it with the backup.
    WouldReplace,
    /// Not applied: the server lacks it, and an apply would create it.
    WouldCreate,
    Restored,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Restored {
    pub collection: String,
    pub name: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Plan, or with `apply` perform, importing a set (or the entities of it that `only` names, as
/// `Collection/Name` or a bare name) back to the server. Each import is confirmed by asking the
/// server for the entity; a failure is reported and the rest are still tried.
pub fn restore(
    remote: &dyn Remote,
    set: &Set,
    only: &[String],
    apply: bool,
) -> Result<Vec<Restored>, BackupError> {
    for wanted in only {
        let known = set.manifest.entities.iter().any(|item| {
            wanted == &item.name || wanted == &format!("{}/{}", item.collection, item.name)
        });
        if !known {
            return Err(BackupError::Invalid {
                path: set.dir.clone(),
                why: format!("{wanted} is not in this set"),
            });
        }
    }
    let mut report = Vec::new();
    for item in &set.manifest.entities {
        let label = format!("{}/{}", item.collection, item.name);
        if !only.is_empty()
            && !only
                .iter()
                .any(|wanted| wanted == &item.name || wanted == &label)
        {
            continue;
        }
        let key = EntityKey::new(item.collection.as_str(), item.name.as_str()).map_err(|why| {
            BackupError::Invalid {
                path: set.dir.join(MANIFEST),
                why: format!("{label} cannot address an entity ({why})"),
            }
        })?;
        let path = saved_path(set, item)?;
        let bytes = std::fs::read(&path).map_err(|error| BackupError::Io {
            path: path.clone(),
            why: error.to_string(),
        })?;
        let present = remote.exists(&key).map_err(|why| BackupError::Remote {
            entity: label.clone(),
            why,
        })?;
        let mut entry = Restored {
            collection: item.collection.clone(),
            name: item.name.clone(),
            status: if present {
                Status::WouldReplace
            } else {
                Status::WouldCreate
            },
            error: None,
        };
        if apply {
            let outcome = remote
                .import(&format!("{}.xml", item.name), &bytes)
                .map_err(|error| error.to_string())
                .and_then(|()| match remote.exists(&key) {
                    Ok(true) => Ok(()),
                    Ok(false) => Err(
                        "the import was accepted, but the server does not have the entity"
                            .to_string(),
                    ),
                    Err(error) => Err(error.to_string()),
                });
            match outcome {
                Ok(()) => entry.status = Status::Restored,
                Err(why) => {
                    entry.status = Status::Failed;
                    entry.error = Some(why);
                }
            }
        }
        report.push(entry);
    }
    Ok(report)
}

/// The entities a forced push or deploy would overwrite although the server holds changes the
/// repository has not seen: the ones worth saving first.
pub fn forced_overwrites(
    decisions: impl IntoIterator<Item = (EntityKey, Decision)>,
) -> Vec<EntityKey> {
    decisions
        .into_iter()
        .filter(|(_, decision)| {
            matches!(
                decision,
                Decision::Refuse(Refusal::UnknownAncestor { .. } | Refusal::Conflict { .. })
            )
        })
        .map(|(key, _)| key)
        .collect()
}

/// Before `entity push --force --apply`: save the server's copy if it holds changes the repository
/// never saw. Returns the set's folder, relative to the solution.
pub fn before_forced_push<R: push::Remote + Remote>(
    client: &R,
    solution: &Solution,
    target: &push::Target,
    stamp: &str,
) -> Result<Option<String>, BackupError> {
    let outcome = push::push(client, &solution.root, target, false, true)
        .map_err(|error| BackupError::Plan(error.to_string()))?;
    let push::Outcome::WouldDo(decision) = outcome else {
        return Ok(None);
    };
    let overwritten = forced_overwrites([(target.key.clone(), decision)]);
    save_overwritten(client, solution, "entity push --force", &overwritten, stamp)
}

/// Before `deploy --force --apply`: save the server's copy of every entity the deploy would
/// overwrite although the server holds changes the repository never saw.
pub fn before_forced_deploy<R: deploy::Remote + Remote>(
    client: &R,
    solution: &Solution,
    projects: &[deploy::ProjectBundle],
    stamp: &str,
) -> Result<Option<String>, BackupError> {
    let baseline = super::baseline::Baseline::load(&solution.root)
        .map_err(|error| BackupError::Plan(error.to_string()))?;
    let plans = deploy::decide_all(client, &baseline, projects)
        .map_err(|error| BackupError::Plan(error.to_string()))?;
    let decisions = plans
        .into_iter()
        .map(|plan| {
            EntityKey::address(&plan.collection, &plan.name)
                .map(|key| (key, plan.decision))
                .map_err(|error| BackupError::Plan(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let overwritten = forced_overwrites(decisions);
    save_overwritten(client, solution, "deploy --force", &overwritten, stamp)
}

fn save_overwritten<R: Remote>(
    client: &R,
    solution: &Solution,
    reason: &str,
    entities: &[EntityKey],
    stamp: &str,
) -> Result<Option<String>, BackupError> {
    if entities.is_empty() {
        return Ok(None);
    }
    Ok(save(client, solution, reason, entities, stamp)?.map(|set| relative(solution, &set.dir)))
}

/// A path as `/`-separated text, relative to the solution when it is under it.
pub fn relative(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake {
        /// `(collection, name)` to its export bytes; an empty vector is an entity whose export is empty.
        entities: RefCell<BTreeMap<(String, String), Vec<u8>>>,
        imported: RefCell<Vec<String>>,
        /// An import that is accepted and does nothing.
        swallow_imports: bool,
    }

    impl Fake {
        fn with(self, collection: &str, name: &str, xml: &str) -> Self {
            self.entities
                .borrow_mut()
                .insert((collection.into(), name.into()), xml.as_bytes().to_vec());
            self
        }
    }

    impl Remote for Fake {
        fn export(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(self
                .entities
                .borrow()
                .get(&(key.collection().to_string(), key.name().to_string()))
                .filter(|bytes| !bytes.is_empty())
                .cloned())
        }
        fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), ServerError> {
            self.imported.borrow_mut().push(file_name.to_string());
            if !self.swallow_imports {
                let name = file_name.trim_end_matches(".xml").to_string();
                self.entities
                    .borrow_mut()
                    .insert(("Things".into(), name), xml.to_vec());
            }
            Ok(())
        }
        fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
            Ok(self
                .entities
                .borrow()
                .contains_key(&(key.collection().to_string(), key.name().to_string())))
        }
    }

    fn solution() -> (tempfile::TempDir, PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-backup-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution)
    }

    fn pairs(names: &[&str]) -> Vec<EntityKey> {
        names
            .iter()
            .map(|name| EntityKey::new("Things", *name).unwrap())
            .collect()
    }

    const XML: &str =
        "<Entities><Things><Thing name=\"A\" projectName=\"P\"></Thing></Things></Entities>";

    #[test]
    fn a_set_holds_the_servers_export_of_each_entity_that_exists_and_says_why() {
        let (_dir, _, solution) = solution();
        let fake = Fake::default().with("Things", "A", XML);
        let set = save(
            &fake,
            &solution,
            "entity delete",
            &pairs(&["A", "Absent"]),
            "20261002-120000",
        )
        .unwrap()
        .unwrap();
        assert_eq!(set.id, "20261002-120000");
        assert_eq!(
            set.manifest.entities,
            [Item {
                collection: "Things".into(),
                name: "A".into(),
                file: Some("Things/A.xml".into()),
            }]
        );
        assert_eq!(
            std::fs::read_to_string(set.dir.join("Things/A.xml")).unwrap(),
            XML
        );
        let listed = list(&solution);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest.reason, "entity delete");
        // Nothing on the server, nothing saved.
        assert!(save(
            &fake,
            &solution,
            "x",
            &pairs(&["Absent"]),
            "20261002-120001"
        )
        .unwrap()
        .is_none());
        // The same stamp twice does not overwrite the first set.
        let again = save(&fake, &solution, "x", &pairs(&["A"]), "20261002-120000")
            .unwrap()
            .unwrap();
        assert_eq!(again.id, "20261002-120000-2");
    }

    #[test]
    fn an_entity_that_exists_but_exports_nothing_refuses_the_backup() {
        let (_dir, _, solution) = solution();
        let fake = Fake::default().with("Things", "Odd", "");
        let error = save(&fake, &solution, "entity delete", &pairs(&["Odd"]), "s").unwrap_err();
        assert!(matches!(error, BackupError::Unreadable { .. }), "{error}");
        assert!(list(&solution).is_empty(), "nothing is half-saved");
    }

    #[test]
    fn only_the_newest_sets_are_kept_and_a_foreign_folder_is_never_removed() {
        let (_dir, root, solution) = solution();
        let fake = Fake::default().with("Things", "A", XML);
        std::fs::create_dir_all(root.join(DIR).join("not-a-set")).unwrap();
        for at in 0..(KEEP + 3) {
            save(
                &fake,
                &solution,
                "r",
                &pairs(&["A"]),
                &format!("2026-{at:04}"),
            )
            .unwrap();
        }
        let sets = list(&solution);
        assert_eq!(sets.len(), KEEP);
        assert_eq!(sets[0].id, "2026-0003", "the oldest were removed");
        assert!(root.join(DIR).join("not-a-set").is_dir());
    }

    #[test]
    fn a_manifest_name_that_cannot_address_an_entity_is_refused_before_the_server_is_asked() {
        let (_dir, _, solution) = solution();
        let fake = Fake::default().with("Things", "A", XML);
        let mut set = save(&fake, &solution, "r", &pairs(&["A"]), "s")
            .unwrap()
            .unwrap();
        // backup.json is a file anyone can edit; `../x` would ask about another route.
        set.manifest.entities[0].name = "../x".to_string();
        let error = restore(&fake, &set, &[], true).unwrap_err().to_string();
        assert!(error.contains("cannot address an entity"), "{error}");
        assert!(fake.imported.borrow().is_empty());
    }

    #[test]
    fn every_entity_of_a_set_has_its_own_file_and_an_old_set_still_restores() {
        let (_dir, _, solution) = solution();
        let long = format!("Acme.{}", "x".repeat(300));
        // Same low byte (é U+00E9, ǩ U+01E9); one file on Windows and macOS (Abc, ABC).
        let names = ["Café", "Cafǩ", "Abc", "ABC", long.as_str()];
        let mut fake = Fake::default();
        for name in names {
            fake = fake.with(
                "Things",
                name,
                &XML.replace("\"A\"", &format!("\"{name}\"")),
            );
        }
        let set = save(&fake, &solution, "r", &pairs(&names), "s")
            .unwrap()
            .unwrap();
        let files: Vec<String> = set
            .manifest
            .entities
            .iter()
            .map(|item| item.file.clone().unwrap())
            .collect();
        let distinct: std::collections::BTreeSet<String> =
            files.iter().map(|file| file.to_lowercase()).collect();
        assert_eq!(distinct.len(), names.len(), "{files:?}");
        assert!(
            files.iter().all(|file| file.len() < MAX_STEM + 20),
            "{files:?}"
        );
        for (item, name) in set.manifest.entities.iter().zip(names) {
            let saved = std::fs::read_to_string(set.dir.join(item.file.as_ref().unwrap())).unwrap();
            assert!(saved.contains(&format!("\"{name}\"")), "{name}: {saved}");
        }

        // A set an older twaco saved records no files and named them by the low byte only.
        let mut old = set.clone();
        for item in &mut old.manifest.entities {
            item.file = None;
        }
        std::fs::rename(
            set.dir.join(&files[0]),
            set.dir.join("Things").join("Caf%E9.xml"),
        )
        .unwrap();
        let planned = restore(&fake, &old, &["Things/Café".to_string()], false).unwrap();
        assert_eq!(planned[0].status, Status::WouldReplace);

        // A recorded file is a plain name inside the set, whoever edited the manifest.
        let mut edited = set.clone();
        edited.manifest.entities[1].file = Some("../../outside.xml".into());
        let error = restore(&fake, &edited, &["Things/Cafǩ".to_string()], false).unwrap_err();
        assert!(
            error.to_string().contains("not a file inside the set"),
            "{error}"
        );
    }

    #[test]
    fn restore_plans_by_default_imports_on_apply_and_confirms_each_entity() {
        let (_dir, _, solution) = solution();
        let fake = Fake::default()
            .with("Things", "A", XML)
            .with("Things", "B", XML);
        let set = save(&fake, &solution, "entity delete", &pairs(&["A", "B"]), "s")
            .unwrap()
            .unwrap();
        fake.entities
            .borrow_mut()
            .remove(&("Things".to_string(), "A".to_string()));
        let plan = restore(&fake, &set, &[], false).unwrap();
        let statuses: Vec<(&str, &Status)> = plan
            .iter()
            .map(|entry| (entry.name.as_str(), &entry.status))
            .collect();
        assert_eq!(
            statuses,
            [("A", &Status::WouldCreate), ("B", &Status::WouldReplace)]
        );
        assert!(fake.imported.borrow().is_empty(), "a plan imports nothing");
        // One entity, by bare name; an unknown one is refused before any request.
        let applied = restore(&fake, &set, &["A".to_string()], true).unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].status, Status::Restored);
        assert_eq!(*fake.imported.borrow(), ["A.xml"]);
        assert!(restore(&fake, &set, &["Nope".to_string()], true).is_err());
    }

    #[test]
    fn an_import_the_server_accepts_but_ignores_is_a_failure() {
        let (_dir, _, solution) = solution();
        let fake = Fake {
            swallow_imports: true,
            ..Default::default()
        }
        .with("Things", "A", XML);
        let set = save(&fake, &solution, "r", &pairs(&["A"]), "s")
            .unwrap()
            .unwrap();
        fake.entities.borrow_mut().clear();
        let applied = restore(&fake, &set, &[], true).unwrap();
        assert_eq!(applied[0].status, Status::Failed);
        assert!(applied[0]
            .error
            .as_deref()
            .unwrap()
            .contains("does not have the entity"));
    }

    #[test]
    fn only_overwrites_of_changes_the_repository_never_saw_are_saved_before_a_force() {
        let decisions = vec![
            (
                EntityKey::new("Things", "Create").unwrap(),
                Decision::Create,
            ),
            (
                EntityKey::new("Things", "Same").unwrap(),
                Decision::AlreadyThere,
            ),
            (
                EntityKey::new("Things", "Update").unwrap(),
                Decision::Update,
            ),
            (
                EntityKey::new("Things", "Gone").unwrap(),
                Decision::Refuse(Refusal::DeletedOnServer),
            ),
            (
                EntityKey::new("Things", "Unknown").unwrap(),
                Decision::Refuse(Refusal::UnknownAncestor { server: "s".into() }),
            ),
            (
                EntityKey::new("Things", "Changed").unwrap(),
                Decision::Refuse(Refusal::Conflict {
                    server: "s".into(),
                    baseline: "b".into(),
                }),
            ),
        ];
        assert_eq!(forced_overwrites(decisions), pairs(&["Unknown", "Changed"]));
        assert_eq!(file_stem("A.b_c-1"), "A.b_c-1");
        assert_eq!(file_stem("../x"), "..%2Fx");
        assert_eq!(file_stem("Café"), "Caf%C3%A9");
    }
}
