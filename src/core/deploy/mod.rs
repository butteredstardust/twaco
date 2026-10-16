//! Deploy orchestration through import and read-back.
//!
//! Bundling and workspace selection live elsewhere. This module owns the server-facing state
//! machine: live-parse every script, reuse push's two-sided decision table, import projects in
//! dependency order, then verify every entity and persist all matching baselines in one write.

use super::codes::{Coded, ErrorCode};
use super::{bundle, config, sidecar, workspace};

mod calls;
mod error;
mod model;
mod plan;
mod remote;
mod run;

pub use error::DeployError;
pub use model::{
    Entity, EntityPlan, NotKept, ParseFailure, PlanOptions, PlannedCall, ProjectBundle, Report,
    Script, ServiceCall,
};
pub use plan::{decide_all, decide_all_with_progress, plan_bundles, toml_parameters};
pub use remote::{BaselineStore, DiskBaseline, Remote};
pub use run::{run, run_with_progress, RunOptions};

impl Coded for DeployError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Baseline(error) => error.code(),
            Self::Working { .. }
            | Self::Server { .. }
            | Self::UnknownPlaceholder { .. }
            | Self::PlaceholderNotText { .. } => ErrorCode::InvalidData,
            Self::ParseUnavailable { source, .. } | Self::Import { source, .. } => source.code(),
            Self::ParseFailed(_) => ErrorCode::GateFailed,
            Self::Conflicts(_) => ErrorCode::ServerConflict,
            Self::Call { .. } => ErrorCode::ServerError,
            Self::NotKept(_) => ErrorCode::NotVerified,
            Self::AfterImport { source, .. } => source.code(),
            Self::Unrecorded { failure, .. } => failure.code(),
        }
    }
}

#[cfg(test)]
mod tests;
