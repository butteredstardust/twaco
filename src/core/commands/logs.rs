//! The command policy around server log level changes.

use super::{Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{logs, profile};
use std::fmt;

/// The requested log-level operation.
#[derive(Clone, Debug)]
pub struct LogLevelRequest {
    pub log: String,
    pub change: Option<logs::Change>,
    pub mode: Mode,
    pub profile: String,
}

/// The completed log-level operation.
#[derive(Debug)]
pub enum LogLevelOutcome {
    Levels {
        levels: logs::Levels,
        effects: Effects,
    },
    Plan {
        report: logs::ChangeReport,
        effects: Effects,
    },
    Applied {
        report: logs::ChangeReport,
        effects: Effects,
    },
}

impl LogLevelOutcome {
    /// The access this operation used or may have used.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Levels { effects, .. }
            | Self::Plan { effects, .. }
            | Self::Applied { effects, .. } => *effects,
        }
    }
}

/// A remote that can inspect and change server log levels.
pub trait Remote: logs::Remote {}

impl<T: logs::Remote + ?Sized> Remote for T {}

/// A failure before a typed log-level outcome could be produced.
#[derive(Debug)]
pub enum LogLevelCommandError {
    Profile(profile::ProfileError),
    Logs(logs::LogsError),
}

impl fmt::Display for LogLevelCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile(error) => error.fmt(f),
            Self::Logs(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LogLevelCommandError {}

impl Coded for LogLevelCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(error) => error.code(),
            Self::Logs(error) => error.code(),
        }
    }
}

/// Read a level, or plan or apply one change. It changes only the server, so no workspace lock
/// is needed.
pub fn execute<R, F>(
    solution: &Solution,
    request: &LogLevelRequest,
    open: F,
    _: &mut Notices,
) -> Result<LogLevelOutcome, LogLevelCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile =
        profile::load(&solution.root, &request.profile).map_err(LogLevelCommandError::Profile)?;
    let remote = open(profile);
    let Some(change) = &request.change else {
        return logs::levels(&remote, &request.log)
            .map(|levels| LogLevelOutcome::Levels {
                levels,
                effects: Effects::new(Access::None, Access::Read),
            })
            .map_err(LogLevelCommandError::Logs);
    };
    let apply = matches!(request.mode, Mode::Apply);
    let report =
        logs::change(&remote, &request.log, change, apply).map_err(LogLevelCommandError::Logs)?;
    Ok(if apply {
        LogLevelOutcome::Applied {
            report,
            effects: Effects::new(Access::None, Access::Write),
        }
    } else {
        LogLevelOutcome::Plan {
            report,
            effects: Effects::new(Access::None, Access::Read),
        }
    })
}
