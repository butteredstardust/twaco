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

/// What a comparison has to go on: the (local, server) pair, and whether an entry exists that
/// was recorded under a different server-owned policy and could not be restated.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub(crate) pair: Option<(String, String)>,
    pub(crate) policy_changed: bool,
}

impl Ancestor {
    pub(crate) fn sides(&self) -> Option<(&str, &str)> {
        self.pair
            .as_ref()
            .map(|(local, server)| (local.as_str(), server.as_str()))
    }
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
        self.get_in_scope(collection, name, working.rsplit_once(':')?.0)
    }

    /// The entry recorded under exactly this hash scope (see `normalise::scope_with_grants`):
    /// the one definition of "recorded" that status and doctor share.
    pub(crate) fn get_in_scope(&self, collection: &str, name: &str, scope: &str) -> Option<&Entry> {
        self.get(collection, name).filter(|entry| {
            entry.local.rsplit_once(':').map(|p| p.0) == Some(scope)
                && entry.server.rsplit_once(':').map(|p| p.0) == Some(scope)
        })
    }

    /// Every recorded address as (collection, name), whatever version recorded it.
    pub(crate) fn addresses(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entities
            .iter()
            .flat_map(|(c, items)| items.keys().map(move |n| (c.as_str(), n.as_str())))
    }

    /// Remove the entries `keep` rejects and return their `Collection/Name` addresses.
    pub(crate) fn prune(&mut self, keep: impl Fn(&str, &str) -> bool) -> Vec<String> {
        let stale: Vec<(String, String)> = self
            .addresses()
            .filter(|(c, n)| !keep(c, n))
            .map(|(c, n)| (c.to_string(), n.to_string()))
            .collect();
        for (collection, name) in &stale {
            self.remove(collection, name);
        }
        stale.into_iter().map(|(c, n)| format!("{c}/{n}")).collect()
    }
    /// The ancestor to compare against: [`Baseline::get_for`], and, when the entry was recorded
    /// under another server-owned policy, the same entry restated for the current one where that
    /// is provable. A baseline recorded with no ownership and no grants can be rebuilt from the
    /// server's bytes: if they still hash to the recorded server side under that plain policy,
    /// the server is unchanged since, and the recorded server side stands for today's hash too.
    /// Any other policy cannot be rebuilt, which `policy_changed` says.
    pub(crate) fn ancestor(
        &self,
        collection: &str,
        name: &str,
        working: &str,
        server: Option<(&[u8], &str)>,
        grants: &super::permissions::Sets,
    ) -> Ancestor {
        if let Some(entry) = self.get_for(collection, name, working) {
            return Ancestor {
                pair: Some((entry.local.clone(), entry.server.clone())),
                policy_changed: false,
            };
        }
        let Some(entry) = self.get(collection, name) else {
            return Ancestor::default();
        };
        let rebased = server.and_then(|(bytes, current)| {
            let scope = entry.server.rsplit_once(':')?.0;
            if entry.local.rsplit_once(':')?.0 != scope {
                return None;
            }
            let plain = super::normalise::hash_with_grants(bytes, &[], grants).ok()?;
            // The server is unchanged since the baseline; a repository that agrees with it is
            // unchanged too, so both sides restate to today's hash (else status would see drift).
            (plain.rsplit_once(':')?.0 == scope && plain == entry.server).then(|| {
                if working == current {
                    (current.to_string(), current.to_string())
                } else {
                    (entry.local.clone(), current.to_string())
                }
            })
        });
        Ancestor {
            policy_changed: rebased.is_none(),
            pair: rebased,
        }
    }

    /// The entry recorded for an entity, if its hashes are of the current normalisation: one
    /// from another version cannot be compared, so it counts as no baseline at all.
    pub(crate) fn get(&self, collection: &str, name: &str) -> Option<&Entry> {
        self.entities
            .get(collection)?
            .get(name)
            .filter(|entry| entry.is_current())
    }

    /// Whether an entry exists, whatever normalisation version recorded it.
    pub(crate) fn contains(&self, collection: &str, name: &str) -> bool {
        self.entities
            .get(collection)
            .is_some_and(|items| items.contains_key(name))
    }

    /// How many entries were recorded under another normalisation version.
    #[cfg(test)]
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

    #[test]
    fn a_plain_baseline_is_restated_for_a_new_policy_only_while_the_server_is_unchanged() {
        use crate::core::normalise;
        use crate::core::server_owned::Part;
        let thing = |script: &str| {
            format!(
                "<Entities><Things><Thing name=\"T\" projectName=\"P\"><code><![CDATA[{script}]]></code></Thing></Things></Entities>"
            )
            .into_bytes()
        };
        let (old, other) = (thing("old();"), thing("other();"));
        let plain = normalise::hash(&old).unwrap();
        let mut baseline = Baseline::default();
        baseline.set("Things", "T", plain.clone(), plain.clone());
        let parts = [Part::Members];
        let working = normalise::hash_with_parts(&thing("new();"), &parts).unwrap();
        let grants = Default::default();
        let now = |bytes: &[u8]| normalise::hash_with_parts(bytes, &parts).unwrap();

        let same = baseline.ancestor("Things", "T", &working, Some((&old, &now(&old))), &grants);
        assert_eq!(same.pair, Some((plain.clone(), now(&old))));
        assert!(!same.policy_changed);

        // Repository and server both unchanged: both sides restate, so status sees no drift.
        let agreed =
            baseline.ancestor("Things", "T", &now(&old), Some((&old, &now(&old))), &grants);
        assert_eq!(agreed.pair, Some((now(&old), now(&old))));
        assert!(!agreed.policy_changed);

        let changed = baseline.ancestor(
            "Things",
            "T",
            &working,
            Some((&other, &now(&other))),
            &grants,
        );
        assert_eq!(changed.pair, None);
        assert!(changed.policy_changed);

        let absent = Baseline::default().ancestor("Things", "T", &working, None, &grants);
        assert_eq!(absent, Ancestor::default());
    }

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
            "v13:two-local".to_string(),
            "v13:two-server".to_string(),
        );
        baseline.set(
            "DataShapes",
            "A",
            "v13:one".to_string(),
            "v13:one".to_string(),
        );
        baseline.set(
            "Things",
            "A",
            "v13:three".to_string(),
            "v13:three".to_string(),
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
                local: "v13:two-local".into(),
                server: "v13:two-server".into()
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
            br#"{"entities":{"Things":{"Old":"v12:old","Half":{"local":"v13:a","server":"v4:b"},"New":"v13:new"}}}"#,
        )
        .unwrap();
        assert_eq!(baseline.get("Things", "Old"), None);
        assert_eq!(baseline.get("Things", "Half"), None);
        assert!(baseline.get("Things", "New").is_some());
        assert_eq!(baseline.outdated(), 2);
    }

    #[test]
    fn prune_removes_only_what_keep_rejects_and_names_it() {
        let mut baseline: Baseline = serde_json::from_slice(
            br#"{"entities":{"Things":{"Acme.Gone":"v10:old","Acme.Kept":"v13:a"},"Users":{"Acme.User":"v13:b"}}}"#,
        )
        .unwrap();
        let pruned = baseline.prune(|_, name| name != "Acme.Gone");
        assert_eq!(pruned, ["Things/Acme.Gone"]);
        assert!(baseline.contains("Things", "Acme.Kept"));
        assert!(baseline.contains("Users", "Acme.User"));
        assert!(!baseline.contains("Things", "Acme.Gone"));
    }

    #[test]
    fn old_string_entries_migrate_to_two_equal_sides_when_written() {
        let (_dir, root) = temp();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(
            root.join(LEGACY_RELATIVE_PATH),
            br#"{"entities":{"Things":{"T":"v13:old"}}}"#,
        )
        .unwrap();

        let baseline = Baseline::load(&root, "default").unwrap();
        assert_eq!(
            baseline.get("Things", "T"),
            Some(&Entry {
                local: "v13:old".into(),
                server: "v13:old".into()
            })
        );
        baseline.write(&root, "default").unwrap();
        assert!(!root.join(LEGACY_RELATIVE_PATH).exists());
        assert_eq!(
            std::fs::read_to_string(Baseline::path(&root, "default")).unwrap(),
            "{\n  \"entities\": {\n    \"Things\": {\n      \"T\": {\n        \"local\": \"v13:old\",\n        \"server\": \"v13:old\"\n      }\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn new_two_sided_entries_round_trip_and_server_updates_preserve_local() {
        let (_dir, root) = temp();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(
            root.join(LEGACY_RELATIVE_PATH),
            br#"{"entities":{"Things":{"T":{"local":"v13:local","server":"v13:server"}}}}"#,
        )
        .unwrap();

        let mut baseline = Baseline::load(&root, "default").unwrap();
        baseline
            .set_server("Things", "T", "v13:after-deploy".into())
            .unwrap();
        assert_eq!(
            baseline.get("Things", "T"),
            Some(&Entry {
                local: "v13:local".into(),
                server: "v13:after-deploy".into()
            })
        );
        assert!(matches!(
            baseline.set_server("Things", "Missing", "v13:x".into()),
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
            br#"{"entities":{"Things":{"T":"v13:old"}}}"#,
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
            "v13:development".into(),
            "v13:development".into(),
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
        first.set("Things", "T", "v13:first".into(), "v13:first".into());
        first.write(&root, "first").unwrap();
        let mut second = Baseline::default();
        second.set("Things", "T", "v13:second".into(), "v13:second".into());
        second.write(&root, "second").unwrap();

        let mut first = Baseline::load(&root, "first").unwrap();
        first.set("Things", "T", "v13:recorded".into(), "v13:recorded".into());
        first.write(&root, "first").unwrap();
        assert_eq!(
            Baseline::load(&root, "second")
                .unwrap()
                .get("Things", "T")
                .unwrap()
                .local,
            "v13:second"
        );
    }

    #[test]
    fn remove_reports_presence_and_prunes_an_empty_collection() {
        let mut baseline = Baseline::default();
        baseline.set("Things", "A", "v13:a".into(), "v13:a".into());
        baseline.set("Things", "B", "v13:b".into(), "v13:b".into());
        assert!(baseline.remove("Things", "A"));
        assert!(!baseline.remove("Things", "A"));
        assert!(baseline.get("Things", "B").is_some());
        assert!(baseline.remove("Things", "B"));
        assert!(!baseline.entities.contains_key("Things"));
    }
}
