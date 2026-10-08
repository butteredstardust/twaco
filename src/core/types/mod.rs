//! TypeScript declarations for the entities in a solution.
//!
//! Repository declarations can be completed by the deliberately small, offline platform cache.

mod compiler;
mod model;
mod outcome;
mod platform;
mod render;
mod write;

pub(crate) use compiler::check_with;
pub use compiler::{check, CompilerOutput, CompilerRunner};
pub(crate) use model::{load_model, Entity, Member, Model, Service, TypedValue};
pub use outcome::{
    check_summary, finding_json, refresh_after_write, CheckError, CheckOutcome, Outcome,
    PlatformOutcome, Refresh, TypeFinding,
};
pub use platform::{fetch_platform, fetch_platform_with_progress, Remote};
pub use write::write;

use super::server::ServerError;
use super::workspace;

#[derive(Debug, thiserror::Error)]
pub enum TypesError {
    #[error("{0}")]
    Workspace(workspace::WorkspaceError),
    #[error("{0}")]
    Remote(ServerError),
    #[error("{0}")]
    Platform(String),
}

impl From<workspace::WorkspaceError> for TypesError {
    fn from(value: workspace::WorkspaceError) -> Self {
        Self::Workspace(value)
    }
}

impl From<ServerError> for TypesError {
    fn from(value: ServerError) -> Self {
        Self::Remote(value)
    }
}

#[cfg(test)]
mod tests;
