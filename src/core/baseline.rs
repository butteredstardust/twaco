//! The tracked ancestor used to attribute local and server changes.
//!
//! This is intentionally not a cache. `.twaco/baseline.json` is reviewable project state, and
//! its nested sorted maps make the collection/name identity explicit and diffs deterministic.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

pub const RELATIVE_PATH: &str = ".twaco/baseline.json";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Entry {
    pub local: String,
    pub server: String,
}

impl Entry {
    /// Whether both sides were hashed by this normalisation version.
    pub fn is_current(&self) -> bool {
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
pub struct Baseline {
    #[serde(default)]
    entities: BTreeMap<String, BTreeMap<String, Entry>>,
}

impl Baseline {
    /// The entry recorded for an entity, if its hashes are of the current normalisation: one
    /// from another version cannot be compared, so it counts as no baseline at all.
    pub fn get(&self, collection: &str, name: &str) -> Option<&Entry> {
        self.entities
            .get(collection)?
            .get(name)
            .filter(|entry| entry.is_current())
    }

    /// How many entries were recorded under another normalisation version.
    pub fn outdated(&self) -> usize {
        self.entities
            .values()
            .flat_map(BTreeMap::values)
            .filter(|entry| !entry.is_current())
            .count()
    }

    pub fn set(&mut self, collection: &str, name: &str, local: String, server: String) {
        self.entities
            .entry(collection.to_string())
            .or_default()
            .insert(name.to_string(), Entry { local, server });
    }

    /// Remove one entity, pruning an empty collection, and report whether it existed.
    pub fn remove(&mut self, collection: &str, name: &str) -> bool {
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
    pub fn set_server(
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

    pub fn load(root: &Path) -> Result<Self, BaselineError> {
        let path = root.join(RELATIVE_PATH);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => {
                return Err(BaselineError::Io {
                    path,
                    why: error.to_string(),
                });
            }
        };
        serde_json::from_slice(&bytes).map_err(|error| BaselineError::Invalid {
            path,
            why: error.to_string(),
        })
    }

    /// The complete deterministic JSON document, as `write` writes it.
    pub fn to_bytes(&self, root: &Path) -> Result<Vec<u8>, BaselineError> {
        let mut bytes =
            serde_json::to_vec_pretty(self).map_err(|error| BaselineError::Invalid {
                path: root.join(RELATIVE_PATH),
                why: error.to_string(),
            })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Write a complete deterministic JSON document through a same-directory temporary file.
    pub fn write(&self, root: &Path) -> Result<(), BaselineError> {
        let path = root.join(RELATIVE_PATH);
        let parent = path.parent().expect("baseline has a parent");
        std::fs::create_dir_all(parent).map_err(|error| BaselineError::Io {
            path: parent.to_path_buf(),
            why: error.to_string(),
        })?;
        let bytes = self.to_bytes(root)?;
        super::workspace::atomic_replace(&path, &bytes).map_err(|error| BaselineError::Io {
            path: path.clone(),
            why: error.to_string(),
        })
    }
}

#[derive(Debug)]
pub enum BaselineError {
    Io { path: PathBuf, why: String },
    Invalid { path: PathBuf, why: String },
    Missing { collection: String, name: String },
}

impl fmt::Display for BaselineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BaselineError::Io { path, why } => write!(f, "{}: {why}", path.display()),
            BaselineError::Invalid { path, why } => {
                write!(f, "invalid baseline {}: {why}", path.display())
            }
            BaselineError::Missing { collection, name } => {
                write!(f, "no baseline entry for {collection}/{name}")
            }
        }
    }
}

impl std::error::Error for BaselineError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let nonce = crate::test_nonce();
        let path =
            std::env::temp_dir().join(format!("twaco-baseline-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn write_is_atomic_deterministic_and_sorted() {
        let root = temp();
        let mut baseline = Baseline::default();
        baseline.set(
            "Things",
            "Z",
            "v5:two-local".to_string(),
            "v5:two-server".to_string(),
        );
        baseline.set(
            "DataShapes",
            "A",
            "v5:one".to_string(),
            "v5:one".to_string(),
        );
        baseline.set(
            "Things",
            "A",
            "v5:three".to_string(),
            "v5:three".to_string(),
        );
        baseline.write(&root).unwrap();
        let first = std::fs::read(root.join(RELATIVE_PATH)).unwrap();
        baseline.write(&root).unwrap();
        let second = std::fs::read(root.join(RELATIVE_PATH)).unwrap();
        assert_eq!(first, second);
        let text = String::from_utf8(first).unwrap();
        assert!(text.find("DataShapes").unwrap() < text.find("Things").unwrap());
        assert!(text.find("\"A\": {").unwrap() < text.find("\"Z\"").unwrap());
        assert_eq!(
            Baseline::load(&root).unwrap().get("Things", "Z"),
            Some(&Entry {
                local: "v5:two-local".into(),
                server: "v5:two-server".into()
            })
        );
        assert!(
            std::fs::read_dir(root.join(".twaco"))
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with("twaco-tmp")),
            "the same-directory temporary must be gone after rename"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_entry_from_another_hash_version_counts_as_no_baseline() {
        let baseline: Baseline = serde_json::from_slice(
            br#"{"entities":{"Things":{"Old":"v4:old","Half":{"local":"v5:a","server":"v4:b"},"New":"v5:new"}}}"#,
        )
        .unwrap();
        assert_eq!(baseline.get("Things", "Old"), None);
        assert_eq!(baseline.get("Things", "Half"), None);
        assert!(baseline.get("Things", "New").is_some());
        assert_eq!(baseline.outdated(), 2);
    }

    #[test]
    fn old_string_entries_migrate_to_two_equal_sides_when_written() {
        let root = temp();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(
            root.join(RELATIVE_PATH),
            br#"{"entities":{"Things":{"T":"v5:old"}}}"#,
        )
        .unwrap();

        let baseline = Baseline::load(&root).unwrap();
        assert_eq!(
            baseline.get("Things", "T"),
            Some(&Entry {
                local: "v5:old".into(),
                server: "v5:old".into()
            })
        );
        baseline.write(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(RELATIVE_PATH)).unwrap(),
            "{\n  \"entities\": {\n    \"Things\": {\n      \"T\": {\n        \"local\": \"v5:old\",\n        \"server\": \"v5:old\"\n      }\n    }\n  }\n}\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn new_two_sided_entries_round_trip_and_server_updates_preserve_local() {
        let root = temp();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(
            root.join(RELATIVE_PATH),
            br#"{"entities":{"Things":{"T":{"local":"v5:local","server":"v5:server"}}}}"#,
        )
        .unwrap();

        let mut baseline = Baseline::load(&root).unwrap();
        baseline
            .set_server("Things", "T", "v5:after-deploy".into())
            .unwrap();
        assert_eq!(
            baseline.get("Things", "T"),
            Some(&Entry {
                local: "v5:local".into(),
                server: "v5:after-deploy".into()
            })
        );
        assert!(matches!(
            baseline.set_server("Things", "Missing", "v5:x".into()),
            Err(BaselineError::Missing { .. })
        ));
        baseline.write(&root).unwrap();
        assert_eq!(
            Baseline::load(&root).unwrap().get("Things", "T"),
            baseline.get("Things", "T")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn remove_reports_presence_and_prunes_an_empty_collection() {
        let mut baseline = Baseline::default();
        baseline.set("Things", "A", "v5:a".into(), "v5:a".into());
        baseline.set("Things", "B", "v5:b".into(), "v5:b".into());
        assert!(baseline.remove("Things", "A"));
        assert!(!baseline.remove("Things", "A"));
        assert!(baseline.get("Things", "B").is_some());
        assert!(baseline.remove("Things", "B"));
        assert!(!baseline.entities.contains_key("Things"));
    }
}
