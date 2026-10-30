//! The tracked ancestor used to attribute local and server changes.
//!
//! This is intentionally not a cache. `.twaco/baselines/<profile>.json` is reviewable project
//! state, one file per server profile, and its nested sorted maps make the collection/name
//! identity explicit and diffs deterministic.

use super::codes::{Coded, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const LEGACY_RELATIVE_PATH: &str = ".twaco/baseline.json";
pub(crate) const DIRECTORY: &str = ".twaco/baselines";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) local: String,
    pub(crate) server: String,
}

impl Entry {
    /// Whether both sides were hashed by this normalisation version.
    pub(crate) fn is_current(&self) -> bool {
        let current = |hash: &str| {
            hash.split_once(':')
                .is_some_and(|(version, _)| version == super::normalise::HASH_VERSION)
        };
        current(&self.local) && current(&self.server)
    }
}

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StoredEntry {
            One(String),
            Two { local: String, server: String },
        }

        Ok(match StoredEntry::deserialize(deserializer)? {
            StoredEntry::One(hash) => Entry {
                local: hash.clone(),
                server: hash,
            },
            StoredEntry::Two { local, server } => Entry { local, server },
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Baseline {
    #[serde(default)]
    entities: BTreeMap<String, BTreeMap<String, Entry>>,
}

impl Baseline {
    /// A baseline belongs to both the normaliser version and this entity's ownership policy.
    pub(crate) fn get_for(&self, collection: &str, name: &str, working: &str) -> Option<&Entry> {
        let scope = working.rsplit_once(':')?.0;
        self.get(collection, name).filter(|entry| {
            entry.local.rsplit_once(':').map(|p| p.0) == Some(scope)
                && entry.server.rsplit_once(':').map(|p| p.0) == Some(scope)
        })
    }
    /// The entry recorded for an entity, if its hashes are of the current normalisation: one
    /// from another version cannot be compared, so it counts as no baseline at all.
    pub(crate) fn get(&self, collection: &str, name: &str) -> Option<&Entry> {
        self.entities
            .get(collection)?
            .get(name)
            .filter(|entry| entry.is_current())
    }

    /// How many entries were recorded under another normalisation version.
    pub(crate) fn outdated(&self) -> usize {
        self.entities
            .values()
            .flat_map(BTreeMap::values)
            .filter(|entry| !entry.is_current())
            .count()
    }

    pub(crate) fn set(&mut self, collection: &str, name: &str, local: String, server: String) {
        self.entities
            .entry(collection.to_string())
            .or_default()
            .insert(name.to_string(), Entry { local, server });
    }

    /// Remove one entity, pruning an empty collection, and report whether it existed.
    pub(crate) fn remove(&mut self, collection: &str, name: &str) -> bool {
        let Some(items) = self.entities.get_mut(collection) else {
            return false;
        };
        let removed = items.remove(name).is_some();
        if items.is_empty() {
            self.entities.remove(collection);
        }
        removed
    }

    /// Update only the observed server side after a post-deploy service changes it.
    ///
    /// Those changes are part of the deploy (restoring a Database password that every import
    /// blanks, for example). The working file correctly does not contain them, so preserving
    /// `local` while advancing `server` is what lets both sides still read as in sync.
    pub(crate) fn set_server(
        &mut self,
        collection: &str,
        name: &str,
        server: String,
    ) -> Result<(), BaselineError> {
        let Some(entry) = self
            .entities
            .get_mut(collection)
            .and_then(|items| items.get_mut(name))
        else {
            return Err(BaselineError::Missing {
                collection: collection.to_string(),
                name: name.to_string(),
            });
        };
        entry.server = server;
        Ok(())
    }

    /// The profile's committed baseline path. Callers that accept a profile name validate it
    /// before this is reached; [`load`](Self::load) and [`write`](Self::write) validate too.
    pub(crate) fn path(root: &Path, profile: &str) -> PathBuf {
        root.join(DIRECTORY).join(format!("{profile}.json"))
    }

    pub(crate) fn load(root: &Path, profile: &str) -> Result<Self, BaselineError> {
        super::profile::validate_name(profile).map_err(BaselineError::Profile)?;
        let path = Self::path(root, profile);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if profile == "default" {
                    return Self::load_path(root.join(LEGACY_RELATIVE_PATH));
                }
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(BaselineError::Io {
                    path,
                    why: error.to_string(),
                });
            }
        };
        Self::from_bytes(path, &bytes)
    }

    fn load_path(path: PathBuf) -> Result<Self, BaselineError> {
        match std::fs::read(&path) {
            Ok(bytes) => Self::from_bytes(path, &bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(BaselineError::Io {
                path,
                why: error.to_string(),
            }),
        }
    }

    pub(crate) fn load_file(path: &Path) -> Result<Self, BaselineError> {
        Self::load_path(path.to_path_buf())
    }

    fn from_bytes(path: PathBuf, bytes: &[u8]) -> Result<Self, BaselineError> {
        serde_json::from_slice(bytes).map_err(|error| BaselineError::Invalid {
            path,
            why: error.to_string(),
        })
    }

    /// Profile names which have a baseline file, including the legacy default baseline.
    pub(crate) fn profiles(root: &Path) -> Result<Vec<String>, BaselineError> {
        let directory = root.join(DIRECTORY);
        let mut profiles = BTreeMap::new();
        match std::fs::read_dir(&directory) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| BaselineError::Io {
                        path: directory.clone(),
                        why: error.to_string(),
                    })?;
                    let path = entry.path();
                    if !entry
                        .file_type()
                        .map_err(|error| BaselineError::Io {
                            path: path.clone(),
                            why: error.to_string(),
                        })?
                        .is_file()
                        || path.extension().and_then(|value| value.to_str()) != Some("json")
                    {
                        continue;
                    }
                    // A file no profile can be named after belongs to nobody: skip it.
                    let Some(name) = path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .filter(|name| super::profile::validate_name(name).is_ok())
                    else {
                        continue;
                    };
                    profiles.insert(name.to_string(), ());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(BaselineError::Io {
                    path: directory,
                    why: error.to_string(),
                })
            }
        }
        if root.join(LEGACY_RELATIVE_PATH).is_file() {
            profiles.insert("default".to_string(), ());
        }
        Ok(profiles.into_keys().collect())
    }

    /// The complete deterministic JSON document, as `write` writes it.
    pub(crate) fn to_bytes(&self, root: &Path) -> Result<Vec<u8>, BaselineError> {
        let mut bytes =
            serde_json::to_vec_pretty(self).map_err(|error| BaselineError::Invalid {
                path: root.join(DIRECTORY),
                why: error.to_string(),
            })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Write a complete deterministic JSON document through a same-directory temporary file.
    pub(crate) fn write(&self, root: &Path, profile: &str) -> Result<(), BaselineError> {
        super::profile::validate_name(profile).map_err(BaselineError::Profile)?;
        let path = Self::path(root, profile);
        let parent = path.parent().expect("baseline has a parent");
        std::fs::create_dir_all(parent).map_err(|error| BaselineError::Io {
            path: parent.to_path_buf(),
            why: error.to_string(),
        })?;
        let bytes = self.to_bytes(root)?;
        super::workspace::atomic_replace(&path, &bytes).map_err(|error| BaselineError::Io {
            path: path.clone(),
            why: error.to_string(),
        })?;
        if profile == "default" {
            let legacy = root.join(LEGACY_RELATIVE_PATH);
            match std::fs::remove_file(&legacy) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(BaselineError::Io {
                        path: legacy,
                        why: error.to_string(),
                    })
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BaselineError {
    #[error("{0}")]
    Profile(#[from] super::profile::ProfileError),
    #[error("{}: {why}", .path.display())]
    Io { path: PathBuf, why: String },
    #[error("invalid baseline {}: {why}", .path.display())]
    Invalid { path: PathBuf, why: String },
    #[error("no baseline entry for {collection}/{name}")]
    Missing { collection: String, name: String },
}

impl Coded for BaselineError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(error) => error.code(),
            Self::Io { .. } => ErrorCode::IoError,
            Self::Invalid { .. } => ErrorCode::InvalidData,
            Self::Missing { .. } => ErrorCode::InvalidData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> (tempfile::TempDir, PathBuf) {
        let path_guard = tempfile::Builder::new()
            .prefix("twaco-baseline-")
            .tempdir()
            .unwrap();
        let path = path_guard.path().to_path_buf();
        std::fs::create_dir_all(&path).unwrap();
        (path_guard, path)
    }

    #[test]
    fn write_is_atomic_deterministic_and_sorted() {
        let (_dir, root) = temp();
        let mut baseline = Baseline::default();
        baseline.set(
            "Things",
            "Z",
            "v10:two-local".to_string(),
            "v10:two-server".to_string(),
        );
        baseline.set(
            "DataShapes",
            "A",
            "v10:one".to_string(),
            "v10:one".to_string(),
        );
        baseline.set(
            "Things",
            "A",
            "v10:three".to_string(),
            "v10:three".to_string(),
        );
        baseline.write(&root, "default").unwrap();
        let first = std::fs::read(Baseline::path(&root, "default")).unwrap();
        baseline.write(&root, "default").unwrap();
        let second = std::fs::read(Baseline::path(&root, "default")).unwrap();
        assert_eq!(first, second);
        let text = String::from_utf8(first).unwrap();
        assert!(text.find("DataShapes").unwrap() < text.find("Things").unwrap());
        assert!(text.find("\"A\": {").unwrap() < text.find("\"Z\"").unwrap());
        assert_eq!(
            Baseline::load(&root, "default").unwrap().get("Things", "Z"),
            Some(&Entry {
                local: "v10:two-local".into(),
                server: "v10:two-server".into()
            })
        );
        assert!(
            std::fs::read_dir(root.join(".twaco"))
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with("twaco-tmp")),
            "the same-directory temporary must be gone after rename"
        );
    }

    #[test]
    fn an_entry_from_another_hash_version_counts_as_no_baseline() {
        let baseline: Baseline = serde_json::from_slice(
            br#"{"entities":{"Things":{"Old":"v4:old","Half":{"local":"v10:a","server":"v4:b"},"New":"v10:new"}}}"#,
        )
        .unwrap();
        assert_eq!(baseline.get("Things", "Old"), None);
        assert_eq!(baseline.get("Things", "Half"), None);
        assert!(baseline.get("Things", "New").is_some());
        assert_eq!(baseline.outdated(), 2);
    }

    #[test]
    fn old_string_entries_migrate_to_two_equal_sides_when_written() {
        let (_dir, root) = temp();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(
            root.join(LEGACY_RELATIVE_PATH),
            br#"{"entities":{"Things":{"T":"v10:old"}}}"#,
        )
        .unwrap();

        let baseline = Baseline::load(&root, "default").unwrap();
        assert_eq!(
            baseline.get("Things", "T"),
            Some(&Entry {
                local: "v10:old".into(),
                server: "v10:old".into()
            })
        );
        baseline.write(&root, "default").unwrap();
        assert!(!root.join(LEGACY_RELATIVE_PATH).exists());
        assert_eq!(
            std::fs::read_to_string(Baseline::path(&root, "default")).unwrap(),
            "{\n  \"entities\": {\n    \"Things\": {\n      \"T\": {\n        \"local\": \"v10:old\",\n        \"server\": \"v10:old\"\n      }\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn new_two_sided_entries_round_trip_and_server_updates_preserve_local() {
        let (_dir, root) = temp();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(
            root.join(LEGACY_RELATIVE_PATH),
            br#"{"entities":{"Things":{"T":{"local":"v10:local","server":"v10:server"}}}}"#,
        )
        .unwrap();

        let mut baseline = Baseline::load(&root, "default").unwrap();
        baseline
            .set_server("Things", "T", "v10:after-deploy".into())
            .unwrap();
        assert_eq!(
            baseline.get("Things", "T"),
            Some(&Entry {
                local: "v10:local".into(),
                server: "v10:after-deploy".into()
            })
        );
        assert!(matches!(
            baseline.set_server("Things", "Missing", "v10:x".into()),
            Err(BaselineError::Missing { .. })
        ));
        baseline.write(&root, "default").unwrap();
        assert_eq!(
            Baseline::load(&root, "default").unwrap().get("Things", "T"),
            baseline.get("Things", "T")
        );
    }

    #[test]
    fn the_legacy_file_is_only_the_default_profiles_baseline() {
        let (_dir, root) = temp();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(
            root.join(LEGACY_RELATIVE_PATH),
            br#"{"entities":{"Things":{"T":"v10:old"}}}"#,
        )
        .unwrap();

        assert!(Baseline::load(&root, "default")
            .unwrap()
            .get("Things", "T")
            .is_some());
        assert!(Baseline::load(&root, "development")
            .unwrap()
            .get("Things", "T")
            .is_none());

        let mut development = Baseline::default();
        development.set(
            "Things",
            "T",
            "v10:development".into(),
            "v10:development".into(),
        );
        development.write(&root, "development").unwrap();
        assert_eq!(
            Baseline::profiles(&root).unwrap(),
            ["default", "development"]
        );
        assert!(Baseline::load(&root, "default")
            .unwrap()
            .get("Things", "T")
            .is_some());
    }

    #[test]
    fn profiles_keep_independent_baselines() {
        let (_dir, root) = temp();
        let mut first = Baseline::default();
        first.set("Things", "T", "v10:first".into(), "v10:first".into());
        first.write(&root, "first").unwrap();
        let mut second = Baseline::default();
        second.set("Things", "T", "v10:second".into(), "v10:second".into());
        second.write(&root, "second").unwrap();

        let mut first = Baseline::load(&root, "first").unwrap();
        first.set("Things", "T", "v10:recorded".into(), "v10:recorded".into());
        first.write(&root, "first").unwrap();
        assert_eq!(
            Baseline::load(&root, "second")
                .unwrap()
                .get("Things", "T")
                .unwrap()
                .local,
            "v10:second"
        );
    }

    #[test]
    fn remove_reports_presence_and_prunes_an_empty_collection() {
        let mut baseline = Baseline::default();
        baseline.set("Things", "A", "v10:a".into(), "v10:a".into());
        baseline.set("Things", "B", "v10:b".into(), "v10:b".into());
        assert!(baseline.remove("Things", "A"));
        assert!(!baseline.remove("Things", "A"));
        assert!(baseline.get("Things", "B").is_some());
        assert!(baseline.remove("Things", "B"));
        assert!(!baseline.entities.contains_key("Things"));
    }
}
