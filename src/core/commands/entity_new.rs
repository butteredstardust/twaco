//! Command policy for `new <kind> <name>`.
use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::{entity_new, lock};

#[derive(Clone, Debug)]
pub struct Request {
    pub request: entity_new::NewRequest,
    pub mode: Mode,
    pub lock_label: &'static str,
}
#[derive(Debug)]
pub struct Outcome {
    pub plan: entity_new::Plan,
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
    New(entity_new::NewError),
}
impl Coded for CommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::New(error) => error.code(),
        }
    }
}
pub fn execute(
    solution: &Solution,
    request: &Request,
    notices: &mut Notices,
) -> Result<Outcome, CommandError> {
    match request.mode {
        Mode::Plan => Ok(Outcome {
            plan: entity_new::plan(solution, &request.request).map_err(CommandError::New)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(CommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(CommandError::Config)?;
            let plan = entity_new::plan(&reloaded, &request.request).map_err(CommandError::New)?;
            entity_new::apply(&reloaded, &plan, &lock).map_err(CommandError::New)?;
            Ok(Outcome {
                plan,
                applied: true,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}
