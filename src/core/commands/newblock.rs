//! The command policy around creating a building block.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::{lock, newblock};

#[derive(Clone, Debug)]
pub struct NewBlockRequest {
    pub request: newblock::Request,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub enum NewBlockOutcome {
    Plan {
        plan: newblock::Plan,
        effects: Effects,
    },
    Applied {
        plan: newblock::Plan,
        effects: Effects,
    },
}
impl NewBlockOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }
    pub fn plan(&self) -> &newblock::Plan {
        match self {
            Self::Plan { plan, .. } | Self::Applied { plan, .. } => plan,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NewBlockCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Config(config::ConfigError),
    #[error("{0}")]
    NewBlock(newblock::NewBlockError),
}
impl Coded for NewBlockCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::NewBlock(error) => error.code(),
        }
    }
}

/// Plan against the supplied solution. An apply locks first, then reloads its configuration
/// because another completed command may have changed the project declarations.
pub fn execute(
    solution: &Solution,
    request: &NewBlockRequest,
    notices: &mut Notices,
) -> Result<NewBlockOutcome, NewBlockCommandError> {
    match request.mode {
        Mode::Plan => {
            let plan = newblock::plan(solution, &request.request)
                .map_err(NewBlockCommandError::NewBlock)?;
            Ok(NewBlockOutcome::Plan {
                plan,
                effects: Effects::new(Access::Read, Access::None),
            })
        }
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(NewBlockCommandError::Lock)?;
            let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
                .map_err(NewBlockCommandError::Config)?;
            let plan = newblock::plan(&reloaded, &request.request)
                .map_err(NewBlockCommandError::NewBlock)?;
            newblock::apply(&reloaded, &plan, &lock).map_err(NewBlockCommandError::NewBlock)?;
            Ok(NewBlockOutcome::Applied {
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
    fn planning_does_not_take_the_lock_and_applying_takes_it_before_reloading_configuration() {
        let root =
            std::env::temp_dir().join(format!("twaco-command-newblock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = |mode| NewBlockRequest {
            request: newblock::Request {
                name: "invalid".to_string(),
                kind: newblock::BlockType::Standard,
                display_name: None,
                description: String::new(),
                parent: None,
                model_logic: false,
                management_shape: true,
                root: None,
                base_extension: None,
            },
            mode,
            lock_label: "new building-block",
        };
        assert!(matches!(
            execute(&solution, &request(Mode::Plan), &mut Notices::default()),
            Err(NewBlockCommandError::NewBlock(_))
        ));
        assert!(matches!(
            execute(&solution, &request(Mode::Apply), &mut Notices::default()),
            Err(NewBlockCommandError::Lock(_))
        ));
        drop(held);
        std::fs::remove_dir_all(root).unwrap();
    }
}
