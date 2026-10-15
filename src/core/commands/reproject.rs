//! The command policy around moving entities to another project.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::{lock, reproject};

#[derive(Clone, Debug)]
pub struct ReprojectRequest {
    pub request: reproject::Request,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub enum ReprojectOutcome {
    Plan {
        plan: reproject::Plan,
        effects: Effects,
    },
    Applied {
        plan: reproject::Plan,
        effects: Effects,
    },
}

impl ReprojectOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }

    pub fn plan(&self) -> &reproject::Plan {
        match self {
            Self::Plan { plan, .. } | Self::Applied { plan, .. } => plan,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReprojectCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Config(config::ConfigError),
    #[error("{0}")]
    Reproject(reproject::ReprojectError),
}

impl Coded for ReprojectCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::Reproject(error) => error.code(),
        }
    }
}

/// Plan without a lock; to apply, lock first and plan again from what is on disk then.
pub fn execute(
    solution: &Solution,
    request: &ReprojectRequest,
    notices: &mut Notices,
) -> Result<ReprojectOutcome, ReprojectCommandError> {
    match request.mode {
        Mode::Plan => Ok(ReprojectOutcome::Plan {
            plan: reproject::plan(solution, &request.request)
                .map_err(ReprojectCommandError::Reproject)?,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(ReprojectCommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(ReprojectCommandError::Config)?;
            let plan = reproject::plan(&reloaded, &request.request)
                .map_err(ReprojectCommandError::Reproject)?;
            reproject::apply(&reloaded, &plan, &lock).map_err(ReprojectCommandError::Reproject)?;
            Ok(ReprojectOutcome::Applied {
                plan,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}
