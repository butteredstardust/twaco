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
}

/// Baseline persistence is a seam for proving that a multi-entity deploy writes exactly once.
pub(crate) trait BaselineStore {
    fn load(&self) -> Result<Baseline, BaselineError>;
    fn write(&self, baseline: &Baseline) -> Result<(), BaselineError>;
}

pub(crate) struct DiskBaseline {
    root: PathBuf,
}

impl DiskBaseline {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }
}

impl BaselineStore for DiskBaseline {
    fn load(&self) -> Result<Baseline, BaselineError> {
        Baseline::load(&self.root)
    }

    fn write(&self, baseline: &Baseline) -> Result<(), BaselineError> {
        baseline.write(&self.root)
    }
}
