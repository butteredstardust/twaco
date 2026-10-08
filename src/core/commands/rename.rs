//! The command policy around every rename kind.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{lock, rename};
use std::fmt;

#[derive(Clone, Debug)]
pub struct RenameRequest {
    pub request: rename::Request,
    pub mode: Mode,
    pub date: String,
    pub lock_label: String,
}

#[derive(Debug)]
pub enum RenameOutcome {
    Plan {
        outcome: rename::Outcome,
        effects: Effects,
    },
    Applied {
        outcome: rename::Outcome,
        effects: Effects,
    },
}
impl RenameOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }
    pub fn outcome(&self) -> &rename::Outcome {
        match self {
            Self::Plan { outcome, .. } | Self::Applied { outcome, .. } => outcome,
        }
    }
}

#[derive(Debug)]
pub enum RenameCommandError {
    Lock(lock::LockError),
    Invalid(String),
    Rename(rename::RenameError),
}
impl fmt::Display for RenameCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(f),
            Self::Invalid(error) => f.write_str(error),
            Self::Rename(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for RenameCommandError {}
impl Coded for RenameCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Rename(error) => error.code(),
        }
    }
}

/// Build and run a rename. The real workspace lock belongs to the apply; rename's scratch
/// verification continues to acquire its own lock for the copied workspace.
pub fn execute(
    solution: &Solution,
    request: &RenameRequest,
    notices: &mut Notices,
) -> Result<RenameOutcome, RenameCommandError> {
    let lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, &request.lock_label, notices)
                .map_err(RenameCommandError::Lock)?,
        ),
    };
    let mut domain = request.request.clone();
    domain.apply = matches!(request.mode, Mode::Apply);
    let (spec, options) = domain
        .build(&solution.root, &request.date)
        .map_err(RenameCommandError::Invalid)?;
    let outcome = rename::run(solution, &spec, &options, lock.as_ref())
        .map_err(RenameCommandError::Rename)?;
    let effects = Effects::new(
        if matches!(request.mode, Mode::Apply) {
            Access::Write
        } else {
            Access::Read
        },
        Access::None,
    );
    Ok(match request.mode {
        Mode::Plan => RenameOutcome::Plan { outcome, effects },
        Mode::Apply => RenameOutcome::Applied { outcome, effects },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_does_not_take_the_lock_and_applying_takes_it_before_running_the_rename() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-rename-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = |mode| RenameRequest {
            request: rename::Request {
                kind: rename::Kind::Entity,
                scope: None,
                service: None,
                old: "Old".to_string(),
                new: "New".to_string(),
                apply: false,
                include_outside: false,
                skip_checks: false,
                database: rename::DatabaseFlags::default(),
                expect_digest: None,
            },
            mode,
            date: "2026-10-05".to_string(),
            lock_label: "rename entity".to_string(),
        };
        assert!(matches!(
            execute(&solution, &request(Mode::Plan), &mut Notices::default()),
            Err(RenameCommandError::Rename(_))
        ));
        assert!(matches!(
            execute(&solution, &request(Mode::Apply), &mut Notices::default()),
            Err(RenameCommandError::Lock(_))
        ));
        drop(held);
    }
}
