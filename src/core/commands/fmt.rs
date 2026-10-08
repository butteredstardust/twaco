//! The command policy around formatting service sidecars.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{lock, workflow};
use std::fmt;

/// The arguments that affect formatting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FmtRequest {
    pub mode: Mode,
    /// The lock-holder label the calling surface has historically shown.
    pub lock_label: &'static str,
}

/// One completed format run, before either adapter projects it to its wire format.
#[derive(Debug)]
pub struct FmtOutcome {
    pub report: workflow::FmtOutcome,
    effects: Effects,
}

impl FmtOutcome {
    /// The access this format run used or would use.
    pub const fn effects(&self) -> Effects {
        self.effects
    }
}

/// A failure before a format outcome could be produced.
#[derive(Debug)]
pub enum FmtCommandError {
    Lock(lock::LockError),
}

impl fmt::Display for FmtCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(why) => why.fmt(f),
        }
    }
}

impl std::error::Error for FmtCommandError {}

impl Coded for FmtCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
        }
    }
}

/// Format scripts. A check is a plan and does not take the workspace lock.
pub fn execute(
    solution: &Solution,
    request: &FmtRequest,
    notices: &mut Notices,
) -> Result<FmtOutcome, FmtCommandError> {
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices).map_err(FmtCommandError::Lock)?,
        ),
    };
    let report = workflow::fmt(solution, matches!(request.mode, Mode::Plan));
    let workspace = match request.mode {
        Mode::Plan => Access::Read,
        Mode::Apply => Access::Write,
    };
    Ok(FmtOutcome {
        report,
        effects: Effects::new(workspace, Access::None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;

    #[test]
    fn a_format_plan_needs_no_lock_but_an_apply_does() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-fmt-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let planned = execute(
            &solution,
            &FmtRequest {
                mode: Mode::Plan,
                lock_label: "fmt",
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(planned.effects(), Effects::new(Access::Read, Access::None));
        let error = execute(
            &solution,
            &FmtRequest {
                mode: Mode::Apply,
                lock_label: "fmt",
            },
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(matches!(error, FmtCommandError::Lock(_)));
        drop(held);
    }
}
