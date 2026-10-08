//! The command policy around assembling one importable bundle.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::bundle;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{lock, workspace};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleRequest {
    pub backend_only: bool,
    pub mode: Mode,
    pub lock_label: &'static str,
}

pub enum BundleOutcome {
    Current {
        target: PathBuf,
        bundle: bundle::Bundle,
        effects: Effects,
    },
    OutOfDate {
        target: PathBuf,
        effects: Effects,
    },
    Missing {
        target: PathBuf,
        effects: Effects,
    },
    Written {
        target: PathBuf,
        bundle: bundle::Bundle,
        effects: Effects,
    },
}

impl BundleOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Current { effects, .. }
            | Self::OutOfDate { effects, .. }
            | Self::Missing { effects, .. }
            | Self::Written { effects, .. } => *effects,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BundleCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("no entity XML found under {}", .root.display())]
    Empty { root: PathBuf },
    #[error("{0}")]
    Build(bundle::BundleError),
    #[error("{}: {why}", .path.display())]
    Create { path: PathBuf, why: std::io::Error },
    #[error("{0}")]
    Write(workspace::WorkspaceError),
}

impl Coded for BundleCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Empty { .. } | Self::Build(_) => ErrorCode::InvalidData,
            Self::Create { .. } => ErrorCode::IoError,
            Self::Write(error) => error.code(),
        }
    }
}

/// Build a bundle, taking the lock before inspecting sources when the generated bundle changes.
pub fn execute(
    solution: &Solution,
    request: &BundleRequest,
    notices: &mut Notices,
) -> Result<BundleOutcome, BundleCommandError> {
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(BundleCommandError::Lock)?,
        ),
    };
    let selection = if request.backend_only {
        bundle::Selection::backend(solution)
    } else {
        bundle::Selection::everything()
    };
    let files = bundle::source_files(solution);
    if files.is_empty() {
        return Err(BundleCommandError::Empty {
            root: solution.root.clone(),
        });
    }
    let built = bundle::build(&files, &selection).map_err(BundleCommandError::Build)?;
    let name = if request.backend_only {
        &solution.bundle.backend_name
    } else {
        &solution.bundle.name
    };
    let directory = solution.root.join(&solution.solution.dist);
    let target = directory.join(name);
    match request.mode {
        Mode::Plan => match std::fs::read(&target) {
            Ok(existing) if existing == built.bytes => Ok(BundleOutcome::Current {
                target,
                bundle: built,
                effects: Effects::new(Access::Read, Access::None),
            }),
            Ok(_) => Ok(BundleOutcome::OutOfDate {
                target,
                effects: Effects::new(Access::Read, Access::None),
            }),
            Err(_) => Ok(BundleOutcome::Missing {
                target,
                effects: Effects::new(Access::Read, Access::None),
            }),
        },
        Mode::Apply => {
            std::fs::create_dir_all(&directory).map_err(|why| BundleCommandError::Create {
                path: directory,
                why,
            })?;
            workspace::write_entity(&target, &built.bytes).map_err(BundleCommandError::Write)?;
            Ok(BundleOutcome::Written {
                target,
                bundle: built,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;

    #[test]
    fn a_bundle_check_needs_no_lock_but_a_write_takes_one_first() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-bundle-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let plan = BundleRequest {
            backend_only: false,
            mode: Mode::Plan,
            lock_label: "bundle",
        };
        assert!(matches!(
            execute(&solution, &plan, &mut Notices::default()),
            Err(BundleCommandError::Empty { .. })
        ));
        let apply = BundleRequest {
            mode: Mode::Apply,
            ..plan
        };
        assert!(matches!(
            execute(&solution, &apply, &mut Notices::default()),
            Err(BundleCommandError::Lock(_))
        ));
        drop(held);
    }
}
