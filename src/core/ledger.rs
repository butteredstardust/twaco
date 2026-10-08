//! The rename ledger, `.twaco/renames.json`: what each rename replaced, so the server work that
//! follows a rename (`entity carry`, `datatable copy`, `entity delete`) knows the old and new names.
//!
//! One typed reader, which refuses anything that is not a record, and one writer, which goes through
//! `workspace::atomic_replace`. The file is a JSON array, two-space indented, ending in a newline;
//! fields keep the order below, and any field twaco does not know is kept as it was.

use super::workspace;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
use std::path::{Path, PathBuf};

pub const RELATIVE_PATH: &str = ".twaco/renames.json";

/// What was renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Entity,
    Prefix,
    Field,
    Service,
    Param,
    Table,
    Property,
}

impl Kind {
    /// Whether the rename created differently named entities on the server (and so left old ones).
    pub fn replaces_entities(self) -> bool {
        matches!(self, Kind::Entity | Kind::Prefix)
    }
}

/// An entity a rename touched: for `entity` and `prefix` the old and new names; for the member
/// kinds the entity that was edited (old and new are then the same).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub collection: String,
    pub old: String,
    pub new: String,
    /// The date `entity delete` removed the old entity from the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<String>,
    /// The date `entity carry` copied the old entity's permissions to the new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carried: Option<String>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Entity {
    pub fn new(
        collection: impl Into<String>,
        old: impl Into<String>,
        new: impl Into<String>,
    ) -> Entity {
        Entity {
            collection: collection.into(),
            old: old.into(),
            new: new.into(),
            deleted: None,
            carried: None,
            other: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub date: String,
    pub kind: Kind,
    pub old: String,
    pub new: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    pub entities: Vec<Entity>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Ledger(pub Vec<Record>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    /// The file could not be read or is not a ledger.
    Invalid {
        path: PathBuf,
        why: String,
    },
    Write {
        path: PathBuf,
        why: String,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LedgerError::Invalid { path, why } => {
                write!(f, "cannot read rename ledger {}: {why}", path.display())
            }
            LedgerError::Write { path, why } => write!(f, "cannot write {}: {why}", path.display()),
        }
    }
}

impl std::error::Error for LedgerError {}

impl Ledger {
    /// The ledger at `path`; a missing file is an empty ledger.
    pub fn read(path: &Path) -> Result<Ledger, LedgerError> {
        let invalid = |why: String| LedgerError::Invalid {
            path: path.to_path_buf(),
            why,
        };
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Ledger::default())
            }
            Err(error) => return Err(invalid(error.to_string())),
        };
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
        let Some(records) = value.as_array() else {
            return Err(invalid("the top level must be a JSON array".to_string()));
        };
        let mut ledger = Vec::with_capacity(records.len());
        for (at, record) in records.iter().enumerate() {
            let parsed: Record = serde_json::from_value(record.clone()).map_err(|error| {
                invalid(format!(
                    "entry {at} is not a rename record (date, kind, old, new, entities): {error}"
                ))
            })?;
            ledger.push(parsed);
        }
        Ok(Ledger(ledger))
    }

    /// The whole ledger as the bytes `write` writes.
    pub fn to_bytes(&self, path: &Path) -> Result<Vec<u8>, LedgerError> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|error| LedgerError::Write {
            path: path.to_path_buf(),
            why: error.to_string(),
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Write the whole ledger, replacing the file in one step and creating `.twaco` if needed.
    pub fn write(&self, path: &Path) -> Result<(), LedgerError> {
        let write = |why: String| LedgerError::Write {
            path: path.to_path_buf(),
            why,
        };
        let bytes = self.to_bytes(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| LedgerError::Write {
                path: parent.to_path_buf(),
                why: error.to_string(),
            })?;
        }
        workspace::atomic_replace(path, &bytes).map_err(|error| write(error.to_string()))
    }

    /// `(record, entity)` positions of the entities that `entity` and `prefix` renames left on the
    /// server and that `keep` selects; positions stay valid for [`Ledger::entity_mut`].
    pub fn replaced(&self, keep: impl Fn(&Entity) -> bool) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        for (record_at, record) in self.0.iter().enumerate() {
            if !record.kind.replaces_entities() {
                continue;
            }
            for (entity_at, entity) in record.entities.iter().enumerate() {
                if keep(entity) {
                    found.push((record_at, entity_at));
                }
            }
        }
        found
    }

    pub fn entity(&self, (record, entity): (usize, usize)) -> &Entity {
        &self.0[record].entities[entity]
    }

    pub fn entity_mut(&mut self, (record, entity): (usize, usize)) -> &mut Entity {
        &mut self.0[record].entities[entity]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> (tempfile::TempDir, PathBuf) {
        let dir_guard = tempfile::Builder::new()
            .prefix(&format!("twaco-ledger-{tag}-"))
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        (dir_guard, dir)
    }

    const TEXT: &str = r#"[
  {
    "date": "2026-10-01",
    "kind": "prefix",
    "old": "A.Old",
    "new": "A.New",
    "entities": [
      {
        "collection": "Things",
        "old": "A.Old.T",
        "new": "A.New.T",
        "deleted": "2026-10-02"
      }
    ]
  },
  {
    "date": "2026-10-02",
    "kind": "param",
    "old": "x",
    "new": "y",
    "scope": "A.New.T",
    "service": "Run",
    "entities": [],
    "futureField": 7
  }
]
"#;

    #[test]
    fn a_ledger_reads_and_writes_back_byte_for_byte_keeping_unknown_fields() {
        let (_dir, root) = dir("round");
        let path = root.join(".twaco/renames.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, TEXT).unwrap();
        let ledger = Ledger::read(&path).unwrap();
        assert_eq!(ledger.0.len(), 2);
        assert_eq!(ledger.0[0].kind, Kind::Prefix);
        assert_eq!(ledger.0[1].scope.as_deref(), Some("A.New.T"));
        ledger.write(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEXT);
    }

    #[test]
    fn marks_are_written_after_the_names_and_selection_skips_what_is_done() {
        let (_dir, root) = dir("marks");
        let path = root.join(".twaco/renames.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, TEXT).unwrap();
        let mut ledger = Ledger::read(&path).unwrap();
        ledger.0[0]
            .entities
            .push(Entity::new("Things", "A.Old.U", "A.New.U"));
        let pending = ledger.replaced(|entity| entity.deleted.is_none());
        assert_eq!(
            pending,
            [(0, 1)],
            "the deleted one and the member-kind record are not pending"
        );
        ledger.entity_mut(pending[0]).carried = Some("2026-10-03".to_string());
        ledger.write(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"new\": \"A.New.U\",\n        \"carried\": \"2026-10-03\""),
            "{text}"
        );
    }

    #[test]
    fn a_missing_file_is_empty_and_anything_that_is_not_a_record_is_refused() {
        let (_dir, root) = dir("bad");
        let path = root.join("renames.json");
        assert_eq!(Ledger::read(&path).unwrap(), Ledger::default());
        for bad in [
            "not json",
            "{}",
            "[1]",
            r#"[{"date":"d","kind":"entity","old":"a","new":"b"}]"#,
            r#"[{"date":"d","kind":"frobnicate","old":"a","new":"b","entities":[]}]"#,
            r#"[{"date":"d","kind":"entity","old":"a","new":"b","entities":[{"collection":"Things","old":"a"}]}]"#,
            r#"[{"date":"d","kind":"entity","old":"a","new":"b","entities":[{"collection":"Things","old":"a","new":"b","deleted":3}]}]"#,
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(
                matches!(Ledger::read(&path), Err(LedgerError::Invalid { .. })),
                "{bad}"
            );
        }
    }
}
