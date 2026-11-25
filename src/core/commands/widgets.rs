//! The command policy around the widget cache: `widgets --platform` and `widgets --webapp`.

use super::{lock_workspace, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::types::Remote;
use crate::core::widget_cache::{self, Outcome, WidgetCacheError};
use crate::core::{lock, profile};
use std::path::PathBuf;

/// Where the widget files come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WidgetSource {
    /// The server of this profile, by GET only.
    Server { profile: String },
    /// A Thingworx webapp folder on disk.
    Folder(PathBuf),
}

/// A request to rebuild the widget cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetsRefreshRequest {
    pub source: WidgetSource,
    pub lock_label: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum WidgetsCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Cache(WidgetCacheError),
}

impl Coded for WidgetsCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
            Self::Profile(why) => why.code(),
            Self::Cache(why) => why.code(),
        }
    }
}

/// Rebuild the widget cache under the workspace lock. Nothing is sent to the server but GET
/// requests for the widget files.
pub fn execute<R, F>(
    solution: &Solution,
    request: &WidgetsRefreshRequest,
    open: F,
    notices: &mut Notices,
) -> Result<Outcome, WidgetsCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let held =
        lock_workspace(solution, request.lock_label, notices).map_err(WidgetsCommandError::Lock)?;
    match &request.source {
        WidgetSource::Folder(folder) => widget_cache::refresh(solution, None, Some(folder), &held),
        WidgetSource::Server { profile } => {
            let profile =
                profile::load(&solution.root, profile).map_err(WidgetsCommandError::Profile)?;
            let remote = open(profile);
            widget_cache::refresh(solution, Some(&remote), None, &held)
        }
    }
    .map_err(WidgetsCommandError::Cache)
}
