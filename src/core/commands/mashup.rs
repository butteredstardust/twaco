//! The command policy around creating and editing mashups.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::{lock, mashup_authoring};

#[derive(Clone, Debug)]
pub struct MashupNewRequest {
    pub request: mashup_authoring::NewRequest,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct MashupNewOutcome {
    pub plan: mashup_authoring::NewPlan,
    pub applied: bool,
    pub effects: Effects,
}

#[derive(Debug, thiserror::Error)]
pub enum MashupCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Config(config::ConfigError),
    #[error("{0}")]
    Authoring(mashup_authoring::AuthoringError),
}

impl Coded for MashupCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::Authoring(error) => error.code(),
        }
    }
}

/// Plan without a lock; to apply, lock first and plan again from what is on disk then.
pub fn new(
    solution: &Solution,
    request: &MashupNewRequest,
    notices: &mut Notices,
) -> Result<MashupNewOutcome, MashupCommandError> {
    match request.mode {
        Mode::Plan => Ok(MashupNewOutcome {
            plan: mashup_authoring::plan_new(solution, &request.request)
                .map_err(MashupCommandError::Authoring)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(MashupCommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(MashupCommandError::Config)?;
            let plan = mashup_authoring::plan_new(&reloaded, &request.request)
                .map_err(MashupCommandError::Authoring)?;
            mashup_authoring::apply_new(&reloaded, &plan, &lock)
                .map_err(MashupCommandError::Authoring)?;
            Ok(MashupNewOutcome {
                plan,
                applied: true,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}
