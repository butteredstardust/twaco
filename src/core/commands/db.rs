//! The command policy around temporary database Things.

use super::{Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{db, profile};
use std::fmt;

/// The database operation requested by an adapter.
#[derive(Clone, Debug)]
pub enum DbRequest {
    Execute {
        sql: String,
        options: db::Options,
        profile: String,
    },
    Clean {
        mode: Mode,
        profile: String,
    },
}

/// The completed database operation.
#[derive(Debug)]
pub enum DbOutcome {
    Executed {
        report: db::Report,
        effects: Effects,
    },
    Cleaned {
        things: Vec<db::Swept>,
        effects: Effects,
    },
}

impl DbOutcome {
    /// The access this operation used or may have used.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Executed { effects, .. } | Self::Cleaned { effects, .. } => *effects,
        }
    }
}

/// A remote that can run and sweep temporary Database Things.
pub trait Remote: db::Remote + db::Sweeper {}

impl<T: db::Remote + db::Sweeper + ?Sized> Remote for T {}

/// A failure before a typed database outcome could be produced.
#[derive(Debug)]
pub enum DbCommandError {
    Profile(profile::ProfileError),
    Database(db::DbError),
}

impl fmt::Display for DbCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile(error) => error.fmt(f),
            Self::Database(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DbCommandError {}

impl Coded for DbCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(error) => error.code(),
            Self::Database(error) => error.code(),
        }
    }
}

/// Plan or run SQL, or plan or remove stale temporary Things. These operations only change the
/// server, so no workspace lock is needed.
pub fn execute<R, F>(
    solution: &Solution,
    request: &DbRequest,
    open: F,
    _: &mut Notices,
) -> Result<DbOutcome, DbCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile_name = match request {
        DbRequest::Execute { profile, .. } | DbRequest::Clean { profile, .. } => profile,
    };
    let profile = profile::load(&solution.root, profile_name).map_err(DbCommandError::Profile)?;
    let remote = open(profile.clone());
    match request {
        DbRequest::Execute { sql, options, .. } => {
            let report = db::execute(&remote, solution, &profile, sql, options)
                .map_err(DbCommandError::Database)?;
            let server = if report.applied {
                Access::Write
            } else {
                Access::Read
            };
            Ok(DbOutcome::Executed {
                report,
                effects: Effects::new(Access::Read, server),
            })
        }
        DbRequest::Clean { mode, .. } => {
            let apply = matches!(mode, Mode::Apply);
            let things = db::sweep(&remote, apply).map_err(DbCommandError::Database)?;
            Ok(DbOutcome::Cleaned {
                things,
                effects: Effects::new(
                    Access::Read,
                    if apply { Access::Write } else { Access::Read },
                ),
            })
        }
    }
}
