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

#[derive(Clone, Debug)]
pub struct MashupAddRequest {
    pub request: mashup_authoring::AddRequest,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct MashupAddOutcome {
    pub plan: mashup_authoring::AddPlan,
    pub applied: bool,
    pub effects: Effects,
}

#[derive(Clone, Debug)]
pub struct MashupAddContainedRequest {
    pub request: mashup_authoring::AddContainedRequest,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct MashupAddContainedOutcome {
    pub plan: mashup_authoring::AddContainedPlan,
    pub applied: bool,
    pub effects: Effects,
}

#[derive(Clone, Debug)]
pub struct MashupBindRequest {
    pub request: mashup_authoring::BindRequest,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct MashupBindOutcome {
    pub plan: mashup_authoring::BindPlan,
    pub applied: bool,
    pub effects: Effects,
}

#[derive(Clone, Debug)]
pub struct MashupOnRequest {
    pub request: mashup_authoring::OnRequest,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct MashupOnOutcome {
    pub plan: mashup_authoring::OnPlan,
    pub applied: bool,
    pub effects: Effects,
}

#[derive(Clone, Debug)]
pub struct MashupParameterRequest {
    pub request: mashup_authoring::ParameterRequest,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct MashupParameterOutcome {
    pub plan: mashup_authoring::ParameterPlan,
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

/// Plan without a lock; to apply, lock first and plan again from what is on disk then.
pub fn add(
    solution: &Solution,
    request: &MashupAddRequest,
    notices: &mut Notices,
) -> Result<MashupAddOutcome, MashupCommandError> {
    match request.mode {
        Mode::Plan => Ok(MashupAddOutcome {
            plan: mashup_authoring::plan_add(solution, &request.request)
                .map_err(MashupCommandError::Authoring)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(MashupCommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(MashupCommandError::Config)?;
            let plan = mashup_authoring::plan_add(&reloaded, &request.request)
                .map_err(MashupCommandError::Authoring)?;
            mashup_authoring::apply_edit(&reloaded, &plan.edit, &lock)
                .map_err(MashupCommandError::Authoring)?;
            Ok(MashupAddOutcome {
                plan,
                applied: true,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}

/// Plan without a lock; to apply, lock first and plan again from what is on disk then.
pub fn add_contained(
    solution: &Solution,
    request: &MashupAddContainedRequest,
    notices: &mut Notices,
) -> Result<MashupAddContainedOutcome, MashupCommandError> {
    match request.mode {
        Mode::Plan => Ok(MashupAddContainedOutcome {
            plan: mashup_authoring::plan_add_contained(solution, &request.request)
                .map_err(MashupCommandError::Authoring)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(MashupCommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(MashupCommandError::Config)?;
            let plan = mashup_authoring::plan_add_contained(&reloaded, &request.request)
                .map_err(MashupCommandError::Authoring)?;
            mashup_authoring::apply_edit(&reloaded, &plan.edit, &lock)
                .map_err(MashupCommandError::Authoring)?;
            Ok(MashupAddContainedOutcome {
                plan,
                applied: true,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}

/// Plan without a lock; to apply, lock first and plan again from what is on disk then.
pub fn bind(
    solution: &Solution,
    request: &MashupBindRequest,
    notices: &mut Notices,
) -> Result<MashupBindOutcome, MashupCommandError> {
    match request.mode {
        Mode::Plan => Ok(MashupBindOutcome {
            plan: mashup_authoring::plan_bind(solution, &request.request)
                .map_err(MashupCommandError::Authoring)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(MashupCommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(MashupCommandError::Config)?;
            let plan = mashup_authoring::plan_bind(&reloaded, &request.request)
                .map_err(MashupCommandError::Authoring)?;
            mashup_authoring::apply_edit(&reloaded, &plan.edit, &lock)
                .map_err(MashupCommandError::Authoring)?;
            Ok(MashupBindOutcome {
                plan,
                applied: true,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}

/// Plan without a lock; to apply, lock first and plan again from what is on disk then.
pub fn on(
    solution: &Solution,
    request: &MashupOnRequest,
    notices: &mut Notices,
) -> Result<MashupOnOutcome, MashupCommandError> {
    match request.mode {
        Mode::Plan => Ok(MashupOnOutcome {
            plan: mashup_authoring::plan_on(solution, &request.request)
                .map_err(MashupCommandError::Authoring)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(MashupCommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(MashupCommandError::Config)?;
            let plan = mashup_authoring::plan_on(&reloaded, &request.request)
                .map_err(MashupCommandError::Authoring)?;
            mashup_authoring::apply_edit(&reloaded, &plan.edit, &lock)
                .map_err(MashupCommandError::Authoring)?;
            Ok(MashupOnOutcome {
                plan,
                applied: true,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}

/// Plan without a lock; to apply, lock first and plan again from what is on disk then.
pub fn parameter(
    solution: &Solution,
    request: &MashupParameterRequest,
    notices: &mut Notices,
) -> Result<MashupParameterOutcome, MashupCommandError> {
    match request.mode {
        Mode::Plan => Ok(MashupParameterOutcome {
            plan: mashup_authoring::plan_parameter(solution, &request.request)
                .map_err(MashupCommandError::Authoring)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(MashupCommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(MashupCommandError::Config)?;
            let plan = mashup_authoring::plan_parameter(&reloaded, &request.request)
                .map_err(MashupCommandError::Authoring)?;
            mashup_authoring::apply_parameter(&reloaded, &plan, &lock)
                .map_err(MashupCommandError::Authoring)?;
            Ok(MashupParameterOutcome {
                plan,
                applied: true,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}
