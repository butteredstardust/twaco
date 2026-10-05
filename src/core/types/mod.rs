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
pub use platform::{fetch_platform, Remote};
pub use write::write;

use super::server::ServerError;
use super::workspace;
use std::fmt;

#[derive(Debug)]
pub enum TypesError {
    Workspace(workspace::WorkspaceError),
    Remote(ServerError),
    Platform(String),
}

impl fmt::Display for TypesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypesError::Workspace(error) => write!(f, "{error}"),
            TypesError::Remote(error) => write!(f, "{error}"),
            TypesError::Platform(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for TypesError {}

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
