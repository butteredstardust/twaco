//! TypeScript declarations for the entities in a solution.
//!
//! Repository declarations can be completed by the deliberately small, offline platform cache.

use super::codes::{Coded, ErrorCode};
use super::lock;

mod compiler;
mod model;
mod outcome;
pub(crate) mod platform;
mod render;
mod write;

pub(crate) use compiler::{check, check_with, CompilerRunner};
pub(crate) use model::{
    load_model, load_model_from, DataShape, Entity, Member, Model, Parameter, Service, TypedValue,
};
pub use outcome::{check_summary, finding_json, Refresh};
pub(crate) use outcome::{
    refresh_after_write, CheckError, CheckOutcome, Outcome, PlatformOutcome, TypeFinding,
};
pub(crate) use platform::{
    cached as cached_platform, fetch_platform_with_progress, Platform, PlatformMeta, Remote,
};
pub(crate) use write::write;

use super::server::ServerError;
use super::workspace;
#[cfg(test)]
pub(crate) use compiler::CompilerOutput;
#[cfg(test)]
pub(crate) use platform::fetch_platform;

#[derive(Debug, thiserror::Error)]
pub enum TypesError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Workspace(workspace::WorkspaceError),
    #[error("{0}")]
    Remote(ServerError),
    #[error("{0}")]
    Platform(String),
    /// A write failed after others had already landed; they are named so nobody has to guess.
    #[error("{}", interrupted_message(.source, .written))]
    Interrupted {
        source: workspace::WorkspaceError,
        written: Vec<std::path::PathBuf>,
    },
}

fn interrupted_message(
    source: &workspace::WorkspaceError,
    written: &[std::path::PathBuf],
) -> String {
    const SHOWN: usize = 5;
    let mut names: Vec<String> = written
        .iter()
        .take(SHOWN)
        .map(|path| path.display().to_string())
        .collect();
    if written.len() > SHOWN {
        names.push(format!("and {} more", written.len() - SHOWN));
    }
    format!(
        "{source}; {} generated file(s) were already updated: {}; run `twaco types` again to finish",
        written.len(),
        names.join(", ")
    )
}

impl From<workspace::WorkspaceError> for TypesError {
    fn from(value: workspace::WorkspaceError) -> Self {
        Self::Workspace(value)
    }
}

impl From<lock::LockError> for TypesError {
    fn from(value: lock::LockError) -> Self {
        Self::Lock(value)
    }
}

impl From<ServerError> for TypesError {
    fn from(value: ServerError) -> Self {
        Self::Remote(value)
    }
}

impl Coded for TypesError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Workspace(error) => error.code(),
            Self::Remote(error) => error.code(),
            Self::Platform(_) => ErrorCode::InvalidData,
            Self::Interrupted { source, .. } => source.code(),
        }
    }
}

// Every failure of the type check is about running the compiler or writing its scratch files.
impl Coded for CheckError {
    fn code(&self) -> ErrorCode {
        ErrorCode::IoError
    }
}

#[cfg(test)]
mod tests;
