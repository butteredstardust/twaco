use super::super::entity_key::EntityKey;
use super::super::ledger::LedgerError;
use super::super::server::ServerError;
use super::guards::{GuardCode, Refusal};
use super::method::Method;
use super::remote::Dependent;
use super::targets::LedgerLocation;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    Absent,
    Refused,
    Deleted,
    Failed,
}

#[derive(Clone, Debug)]
pub struct EntityResult {
    pub collection: String,
    pub name: String,
    pub status: Status,
    pub method: Method,
    pub dependents: Vec<Dependent>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub(super) refusals: Vec<Refusal>,
    pub(super) ledger: Vec<LedgerLocation>,
}

impl EntityResult {
    pub(super) fn key(&self) -> EntityKey {
        EntityKey::new(&self.collection, &self.name)
            .expect("entity results originate from validated delete targets")
    }

    pub fn refusals(&self) -> impl Iterator<Item = &str> {
        self.refusals.iter().map(|refusal| refusal.message.as_str())
    }

    pub fn refusal_codes(&self) -> impl Iterator<Item = GuardCode> + '_ {
        self.refusals.iter().map(|refusal| refusal.code)
    }

    pub fn refusal_pairs(&self) -> impl Iterator<Item = (GuardCode, &str)> {
        self.refusals
            .iter()
            .map(|refusal| (refusal.code, refusal.message.as_str()))
    }
}

impl Serialize for EntityResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            collection: &'a str,
            name: &'a str,
            status: &'a Status,
            method: &'a Method,
            dependents: &'a [Dependent],
            warnings: &'a [String],
            #[serde(skip_serializing_if = "Vec::is_empty")]
            refusals: Vec<&'a str>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            refusal_codes: Vec<GuardCode>,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: &'a Option<String>,
        }
        Wire {
            collection: &self.collection,
            name: &self.name,
            status: &self.status,
            method: &self.method,
            dependents: &self.dependents,
            warnings: &self.warnings,
            refusals: self.refusals().collect(),
            refusal_codes: self.refusal_codes().collect(),
            error: &self.error,
        }
        .serialize(serializer)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    #[error("{}: invalid rename ledger: {why}", .path.display())]
    Ledger { path: PathBuf, why: String },
    #[error("{0}")]
    Target(String),
    #[error("{entity}: {why}")]
    Remote { entity: String, why: ServerError },
    #[error("cannot write {}: {why}", .path.display())]
    Write { path: PathBuf, why: String },
    /// A backup could not be taken, so nothing was deleted.
    #[error("nothing was deleted, because the backup could not be taken: {0} (--no-backup deletes without one)")]
    Backup(String),
}

impl From<LedgerError> for DeleteError {
    fn from(error: LedgerError) -> Self {
        match error {
            LedgerError::Invalid { path, why } => DeleteError::Ledger { path, why },
            LedgerError::Write { path, why } => DeleteError::Write { path, why },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub applied: bool,
    pub entities: Vec<EntityResult>,
    pub dependency_limit: &'static str,
    /// The folder the entities were saved to before they were deleted, relative to the solution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    #[serde(skip)]
    pub ledger_changed: bool,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.entities
            .iter()
            .any(|entity| matches!(entity.status, Status::Refused | Status::Failed))
    }
}
