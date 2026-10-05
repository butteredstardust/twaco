//! Deploy orchestration through import and read-back.
//!
//! Bundling and workspace selection live elsewhere. This module owns the server-facing state
//! machine: live-parse every script, reuse push's two-sided decision table, import projects in
//! dependency order, then verify every entity and persist all matching baselines in one write.

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
pub use plan::{decide_all, plan_bundles, toml_parameters};
pub use remote::{BaselineStore, DiskBaseline, Remote};
pub use run::run;

#[cfg(test)]
mod tests;
