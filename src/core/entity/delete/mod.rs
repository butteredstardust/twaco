//! Planning and applying guarded entity deletion.
//!
//! A delete is planned from read-only server calls first. Structural dependents, repository
//! definitions and file-repository Things are made visible before an apply can remove anything.
//! Script strings and Mashup JSON are outside ThingWorx's incoming-dependency graph and the
//! result says so rather than presenting the guard as complete.

use crate::core::codes::{Coded, ErrorCode};
use crate::core::ledger;

mod guards;
mod method;
mod remote;
mod report;
mod run;
mod targets;

impl Coded for DeleteError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote { why, .. } => why.code(),
            Self::Target(_) => ErrorCode::InvalidArguments,
            Self::Ledger { .. } => ErrorCode::InvalidData,
            Self::Write { .. } | Self::Backup(_) => ErrorCode::IoError,
        }
    }
}

#[cfg(test)]
mod tests;

pub const DEPENDENCY_LIMIT: &str =
    "GetIncomingDependencies sees structural dependents only, never names inside scripts or mashup JSON";
pub const FORCE_DEPRECATION: &str =
    "--force is deprecated for entity delete; it now means --allow-repository-defined --allow-outside-dependents and never covers FileRepository data loss";

pub use guards::{Acknowledged, GuardCode};
pub use method::{method_for, Method};
pub use remote::{Dependent, Remote};
pub use report::{DeleteError, EntityResult, Report, Status};
pub use run::run;
pub use targets::{prepare, Prepared};
