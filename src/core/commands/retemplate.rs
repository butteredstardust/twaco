//! The command policy around changing an entity's template or shapes.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{lock, retemplate};
use std::fmt;

#[derive(Clone, Debug)]
pub struct RetemplateRequest {
    pub request: retemplate::Request,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub enum RetemplateOutcome {
    Plan {
        plan: retemplate::Plan,
        effects: Effects,
    },
    Applied {
        plan: retemplate::Plan,
        effects: Effects,
    },
}

impl RetemplateOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }

    pub fn plan(&self) -> &retemplate::Plan {
        match self {
            Self::Plan { plan, .. } | Self::Applied { plan, .. } => plan,
        }
    }
}

#[derive(Debug)]
pub enum RetemplateCommandError {
    Lock(lock::LockError),
    Retemplate(retemplate::RetemplateError),
}

impl fmt::Display for RetemplateCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(f),
            Self::Retemplate(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for RetemplateCommandError {}
impl Coded for RetemplateCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Retemplate(error) => error.code(),
        }
    }
}

/// Plan without a lock, or lock before reading the entity that will be replaced.
pub fn execute(
    solution: &Solution,
    request: &RetemplateRequest,
    notices: &mut Notices,
) -> Result<RetemplateOutcome, RetemplateCommandError> {
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(RetemplateCommandError::Lock)?,
        ),
    };
    let plan =
        retemplate::plan(solution, &request.request).map_err(RetemplateCommandError::Retemplate)?;
    match request.mode {
        Mode::Plan => Ok(RetemplateOutcome::Plan {
            plan,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            retemplate::apply(&plan).map_err(RetemplateCommandError::Retemplate)?;
            Ok(RetemplateOutcome::Applied {
                plan,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_does_not_take_the_lock_and_applying_takes_it_before_discovery() {
        let root =
            std::env::temp_dir().join(format!("twaco-command-retemplate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = |mode| RetemplateRequest {
            request: retemplate::Request {
                entity: "missing".to_string(),
                ..Default::default()
            },
            mode,
            lock_label: "retemplate",
        };
        assert!(matches!(
            execute(&solution, &request(Mode::Plan), &mut Notices::default()),
            Err(RetemplateCommandError::Retemplate(_))
        ));
        assert!(matches!(
            execute(&solution, &request(Mode::Apply), &mut Notices::default()),
            Err(RetemplateCommandError::Lock(_))
        ));
        drop(held);
        std::fs::remove_dir_all(root).unwrap();
    }
}
