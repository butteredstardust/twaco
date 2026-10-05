//! Planning and applying guarded entity deletion.
//!
//! A delete is planned from read-only server calls first. Structural dependents, repository
//! definitions and file-repository Things are made visible before an apply can remove anything.
//! Script strings and Mashup JSON are outside ThingWorx's incoming-dependency graph and the
//! result says so rather than presenting the guard as complete.

use super::ledger;

mod guards;
mod method;
mod remote;
mod report;
mod run;
mod targets;

#[cfg(test)]
mod tests;

pub const DEPENDENCY_LIMIT: &str =
    "GetIncomingDependencies sees structural dependents only, never names inside scripts or mashup JSON";
pub const FORCE_DEPRECATION: &str =
    "--force is deprecated for entity delete; it now means --allow-repository-defined --allow-outside-dependents and never covers FileRepository data loss";

pub use guards::{acknowledged, Acknowledged, GuardCode};
pub use method::{method_for, Method};
pub use remote::{Dependent, Remote};
pub use report::{DeleteError, EntityResult, Report, Status};
pub use run::run;
pub use targets::{prepare, Prepared};
