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

pub(crate) const DEPENDENCY_LIMIT: &str =
    "GetIncomingDependencies sees structural dependents only, never names inside scripts or mashup JSON";
pub const FORCE_DEPRECATION: &str =
    "--force is deprecated for entity delete; it now means --allow-repository-defined --allow-outside-dependents and never covers FileRepository data loss";

pub use guards::Acknowledged;
#[cfg(test)]
pub(crate) use guards::GuardCode;
#[cfg(test)]
pub(crate) use method::Method;
#[cfg(test)]
pub(crate) use remote::Dependent;
pub(crate) use remote::Remote;
pub(crate) use report::{DeleteError, Report, Status};
pub(crate) use run::run;
pub(crate) use targets::prepare;
