//! The command policy around moving or copying a member.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{lock, relocate};
use std::fmt;

#[derive(Clone, Debug)]
pub struct RelocateRequest {
    pub request: relocate::Request,
    pub mode: Mode,
    pub lock_label: String,
}

#[derive(Debug)]
pub enum RelocateOutcome {
    Plan {
        plan: relocate::Plan,
        effects: Effects,
    },
    Applied {
        plan: relocate::Plan,
        problems: Vec<String>,
        effects: Effects,
    },
}
impl RelocateOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }
    pub fn plan(&self) -> &relocate::Plan {
        match self {
            Self::Plan { plan, .. } | Self::Applied { plan, .. } => plan,
        }
    }
    pub fn problems(&self) -> &[String] {
        match self {
            Self::Plan { .. } => &[],
            Self::Applied { problems, .. } => problems,
        }
    }
}

#[derive(Debug)]
pub enum RelocateCommandError {
    Lock(lock::LockError),
    Relocate(relocate::RelocateError),
}
impl fmt::Display for RelocateCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(f),
            Self::Relocate(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for RelocateCommandError {}
impl Coded for RelocateCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Relocate(error) => error.code(),
        }
    }
}

/// Plan without a lock, or lock before discovering the source and target files an apply changes.
pub fn execute(
    solution: &Solution,
    request: &RelocateRequest,
    notices: &mut Notices,
) -> Result<RelocateOutcome, RelocateCommandError> {
    let lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, &request.lock_label, notices)
                .map_err(RelocateCommandError::Lock)?,
        ),
    };
    let plan =
        relocate::plan(solution, &request.request).map_err(RelocateCommandError::Relocate)?;
    match lock {
        None => Ok(RelocateOutcome::Plan {
            plan,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Some(lock) => {
            relocate::apply(&plan, &lock).map_err(RelocateCommandError::Relocate)?;
            let problems = relocate::verify(solution, &plan);
            Ok(RelocateOutcome::Applied {
                plan,
                problems,
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
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-relocate-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = |mode| RelocateRequest {
            request: relocate::Request {
                member: relocate::Member::Service,
                copy: false,
                from: "A".to_string(),
                to: "B".to_string(),
                name: "C".to_string(),
                new_name: None,
                leave_delegate: false,
            },
            mode,
            lock_label: "move service".to_string(),
        };
        assert!(matches!(
            execute(&solution, &request(Mode::Plan), &mut Notices::default()),
            Err(RelocateCommandError::Relocate(_))
        ));
        assert!(matches!(
            execute(&solution, &request(Mode::Apply), &mut Notices::default()),
            Err(RelocateCommandError::Lock(_))
        ));
        drop(held);
    }
}
