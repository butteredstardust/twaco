//! Command policy for local entity edits.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::{entity_edit, lock};

#[derive(Clone, Debug)]
pub struct EditRequest {
    pub request: entity_edit::Request,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct Outcome {
    pub plan: entity_edit::Plan,
    pub applied: bool,
    pub effects: Effects,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Config(config::ConfigError),
    #[error("{0}")]
    Edit(entity_edit::EditError),
}

impl Coded for CommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::Edit(entity_edit::EditError::Unknown(_)) => ErrorCode::UnknownEntity,
            Self::Edit(_) => ErrorCode::InvalidArguments,
        }
    }
}

/// Plan without a lock; apply locks and plans again from disk before its transaction writes.
pub fn execute(
    solution: &Solution,
    request: &EditRequest,
    notices: &mut Notices,
) -> Result<Outcome, CommandError> {
    match request.mode {
        Mode::Plan => Ok(Outcome {
            plan: entity_edit::plan(solution, &request.request).map_err(CommandError::Edit)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(CommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(CommandError::Config)?;
            let plan =
                entity_edit::plan(&reloaded, &request.request).map_err(CommandError::Edit)?;
            entity_edit::apply(&reloaded, &plan, &lock).map_err(CommandError::Edit)?;
            Ok(Outcome {
                plan,
                applied: true,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}
