//! The command policy around comparing or adopting a Composer export.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::adopt;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::lock;
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdoptRequest {
    pub export: PathBuf,
    pub only: Vec<String>,
    pub mode: Mode,
    pub lock_label: &'static str,
}

pub enum AdoptOutcome {
    Plan {
        report: adopt::Report,
        effects: Effects,
    },
    Applied {
        report: adopt::Report,
        outcome: adopt::ApplyOutcome,
        effects: Effects,
    },
}

impl AdoptOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }
}

#[derive(Debug)]
pub enum AdoptCommandError {
    Lock(lock::LockError),
    Adopt(adopt::AdoptError),
}

impl fmt::Display for AdoptCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(f),
            Self::Adopt(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AdoptCommandError {}

impl Coded for AdoptCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Adopt(error) => error.code(),
        }
    }
}

/// Compare an export, or take the workspace lock before reading it again to adopt its mechanical
/// changes.
pub fn execute(
    solution: &Solution,
    request: &AdoptRequest,
    notices: &mut Notices,
) -> Result<AdoptOutcome, AdoptCommandError> {
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(AdoptCommandError::Lock)?,
        ),
    };
    let report = adopt::compare(solution, &request.export, &request.only)
        .map_err(AdoptCommandError::Adopt)?;
    match request.mode {
        Mode::Plan => Ok(AdoptOutcome::Plan {
            report,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let outcome = adopt::apply(solution, &request.export, &report)
                .map_err(AdoptCommandError::Adopt)?;
            Ok(AdoptOutcome::Applied {
                report,
                outcome,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_does_not_take_the_lock_and_applying_takes_it_before_reading_the_export() {
        let root = std::env::temp_dir().join(format!("twaco-command-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = |mode| AdoptRequest {
            export: root.join("missing.xml"),
            only: Vec::new(),
            mode,
            lock_label: "adopt",
        };
        assert!(matches!(
            execute(&solution, &request(Mode::Plan), &mut Notices::default()),
            Err(AdoptCommandError::Adopt(_))
        ));
        assert!(matches!(
            execute(&solution, &request(Mode::Apply), &mut Notices::default()),
            Err(AdoptCommandError::Lock(_))
        ));
        drop(held);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn applying_reports_the_recovery_notice_from_its_lock() {
        let root =
            std::env::temp_dir().join(format!("twaco-command-adopt-notice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(root.join(".twaco/.baseline.json.1.twaco-tmp"), b"half").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let request = AdoptRequest {
            export: root.join("missing.xml"),
            only: Vec::new(),
            mode: Mode::Apply,
            lock_label: "adopt",
        };
        let mut notices = Notices::default();
        assert!(matches!(
            execute(&solution, &request, &mut notices),
            Err(AdoptCommandError::Adopt(_))
        ));
        assert_eq!(notices.lines().len(), 1, "{:?}", notices.lines());
        std::fs::remove_dir_all(root).unwrap();
    }
}
