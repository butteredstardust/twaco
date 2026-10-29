use super::super::baseline::{Baseline, BaselineError};
use super::super::entity::key::ServiceTarget;
use super::super::push;
use super::super::server::{Client, ScriptCheck, ServerError};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The server operations deploy needs. The parse method is deliberately narrow; this is not a
/// generic ThingWorx service-call interface.
pub trait Remote: push::Remote + Sync {
    fn check_script(&self, script: &str) -> Result<ScriptCheck, ServerError>;
    fn call_service(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, ServerError>;
    /// What the Importer logged as the cause of an import that failed after `since_ms`: an
    /// HTTP 406 says nothing more. An error when the log cannot be read.
    fn import_failures(&self, since_ms: i64) -> Result<Vec<String>, String> {
        let _ = since_ms;
        Ok(Vec::new())
    }
    /// The names of every entity of `collection` on the server.
    fn list_entity_names(&self, collection: &str) -> Result<Vec<String>, ServerError> {
        Err(ServerError::InvalidResponse {
            url: collection.to_string(),
            why: "this server connection cannot list a collection".to_string(),
        })
    }
}

impl Remote for Client {
    fn check_script(&self, script: &str) -> Result<ScriptCheck, ServerError> {
        Client::check_script(self, script)
    }

    fn call_service(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, ServerError> {
        Client::call_service(self, target, service, parameters, timeout)
    }

    fn list_entity_names(&self, collection: &str) -> Result<Vec<String>, ServerError> {
        Client::list_entity_names(self, collection)
    }

    fn import_failures(&self, since_ms: i64) -> Result<Vec<String>, String> {
        use super::super::logs;
        let query = logs::Query {
            log: "ApplicationLog".to_string(),
            from_ms: since_ms,
            to_ms: logs::now_ms() + 1_000,
            level: Some("ERROR"),
            search: Some(logs::Search::Grep("Import Failed:".to_string())),
            user: None,
            thread: None,
            origin: None,
            limit: 20,
            oldest_first: true,
        };
        let mut reasons = Vec::new();
        for entry in logs::query(self, &query)
            .map_err(|error| error.to_string())?
            .entries
        {
            // The Importer logs the same cause several times, some wrapped in "Invalid request".
            if let Some((_, reason)) = entry.content.split_once("Import Failed: ") {
                let reason = reason.trim().to_string();
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
        }
        Ok(reasons)
    }
}

/// Baseline persistence is a seam for proving that a multi-entity deploy writes exactly once.
pub(crate) trait BaselineStore {
    fn load(&self) -> Result<Baseline, BaselineError>;
    fn write(&self, baseline: &Baseline) -> Result<(), BaselineError>;
}

pub(crate) struct DiskBaseline {
    root: PathBuf,
    profile: String,
}

impl DiskBaseline {
    pub(crate) fn new(root: &Path, profile: &str) -> Self {
        Self {
            root: root.to_path_buf(),
            profile: profile.to_string(),
        }
    }
}

impl BaselineStore for DiskBaseline {
    fn load(&self) -> Result<Baseline, BaselineError> {
        Baseline::load(&self.root, &self.profile)
    }

    fn write(&self, baseline: &Baseline) -> Result<(), BaselineError> {
        baseline.write(&self.root, &self.profile)
    }
}
