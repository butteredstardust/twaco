//! The command policy around durable collaborator handoffs.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::adopt::{self, Handoff};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::lock;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoffRequest {
    List,
    Record { name: String, files: Vec<PathBuf> },
}

pub enum HandoffOutcome {
    Listed {
        handoffs: Vec<Handoff>,
        effects: Effects,
    },
    Planned {
        name: String,
        files: Vec<PathBuf>,
        effects: Effects,
    },
    Recorded {
        handoff: Handoff,
        effects: Effects,
    },
}

impl HandoffOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Listed { effects, .. }
            | Self::Planned { effects, .. }
            | Self::Recorded { effects, .. } => *effects,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HandoffCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Adopt(adopt::AdoptError),
}

impl Coded for HandoffCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Adopt(error) => error.code(),
        }
    }
}

/// List recorded handoffs, or validate and copy delivered exports under the workspace lock.
pub fn execute(
    solution: &Solution,
    request: &HandoffRequest,
    mode: Mode,
    lock_label: &str,
    notices: &mut Notices,
) -> Result<HandoffOutcome, HandoffCommandError> {
    match request {
        HandoffRequest::List => Ok(HandoffOutcome::Listed {
            handoffs: adopt::handoffs(solution).map_err(HandoffCommandError::Adopt)?,
            effects: Effects::new(Access::Read, Access::None),
        }),
        HandoffRequest::Record { name, files } => {
            // Validate a plan with the same parser as a real recording, without making its
            // directory visible. `record_handoff` repeats this just before copying under lock.
            if mode == Mode::Plan {
                if files.is_empty() {
                    return Err(HandoffCommandError::Adopt(adopt::AdoptError::Base {
                        base: name.clone(),
                        why: "record at least one <Entities> document".to_string(),
                    }));
                }
                for file in files {
                    adopt::Side::from_export(file).map_err(HandoffCommandError::Adopt)?;
                }
                return Ok(HandoffOutcome::Planned {
                    name: name.clone(),
                    files: files.clone(),
                    effects: Effects::new(Access::Read, Access::None),
                });
            }
            let _lock =
                lock_workspace(solution, lock_label, notices).map_err(HandoffCommandError::Lock)?;
            let handoff =
                adopt::record_handoff(solution, name, files).map_err(HandoffCommandError::Adopt)?;
            Ok(HandoffOutcome::Recorded {
                handoff,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}
