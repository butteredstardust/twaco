//! The command policy around copying DataTable rows.

use super::{Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{datatable_copy, profile};
use std::collections::BTreeMap;

/// The arguments that affect a DataTable copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataTableCopyRequest {
    pub old: String,
    pub new: String,
    pub map: BTreeMap<String, String>,
    pub drop_unmapped: bool,
    pub append: bool,
    pub max_rows: u64,
    pub mode: Mode,
    pub profile: String,
}

/// One completed DataTable copy, before either adapter projects it to its wire format.
#[derive(Debug)]
pub enum DataTableCopyOutcome {
    Plan {
        report: datatable_copy::Report,
        effects: Effects,
    },
    Applied {
        report: datatable_copy::Report,
        effects: Effects,
    },
}

impl DataTableCopyOutcome {
    /// The access this specific outcome implies.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }
}

/// A remote that can copy and verify DataTable rows.
pub trait Remote: datatable_copy::Remote {}

impl<T: datatable_copy::Remote + ?Sized> Remote for T {}

/// A failure before a typed DataTable copy outcome could be produced.
#[derive(Debug, thiserror::Error)]
pub enum DataTableCopyCommandError {
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Copy(datatable_copy::CopyError),
}

impl Coded for DataTableCopyCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(error) => error.code(),
            Self::Copy(error) => error.code(),
        }
    }
}

/// Plan or apply a DataTable row copy. It only changes the server, so it never takes the
/// workspace lock.
pub fn execute<R, F>(
    solution: &Solution,
    request: &DataTableCopyRequest,
    open: F,
    _: &mut Notices,
) -> Result<DataTableCopyOutcome, DataTableCopyCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile = profile::load(&solution.root, &request.profile)
        .map_err(DataTableCopyCommandError::Profile)?;
    let domain_request = datatable_copy::Request {
        old: request.old.clone(),
        new: request.new.clone(),
        map: request.map.clone(),
        drop_unmapped: request.drop_unmapped,
        append: request.append,
        max_rows: request.max_rows,
        apply: matches!(request.mode, Mode::Apply),
    };
    let report = datatable_copy::run(&open(profile), solution, &domain_request)
        .map_err(DataTableCopyCommandError::Copy)?;
    Ok(match request.mode {
        Mode::Plan => DataTableCopyOutcome::Plan {
            report,
            effects: Effects::new(Access::Read, Access::Read),
        },
        Mode::Apply => DataTableCopyOutcome::Applied {
            report,
            effects: Effects::new(Access::Read, Access::Write),
        },
    })
}
