//! The command policy around a solution's building blocks: the server's view, and writing a
//! block's version under the workspace lock.

use super::{lock_workspace, Notices};
use crate::core::block::{self, Remote, VersionChange, VersionPlan};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::{lock, profile, transaction};

#[derive(Debug, thiserror::Error)]
pub enum BlockCommandError {
    /// No one block of that name.
    #[error("{0}")]
    Find(block::FindError),
    /// The change cannot be made: the files or the version given.
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Config(config::ConfigError),
    #[error("{0}")]
    Write(transaction::TransactionError),
}

impl Coded for BlockCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Find(error) => error.code(),
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Lock(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::Write(error) => error.code(),
        }
    }
}

fn plan(
    solution: &Solution,
    name: &str,
    change: &VersionChange,
) -> Result<VersionPlan, BlockCommandError> {
    block::find(solution, name).map_err(BlockCommandError::Find)?;
    block::plan_version(solution, name, change).map_err(BlockCommandError::Invalid)
}

/// The framework's view of the server `profile_name` names.
pub fn server<R, F>(
    solution: &Solution,
    profile_name: &str,
    open: F,
) -> Result<(R, Vec<block::ServerBlock>), String>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile = profile::load(&solution.root, profile_name).map_err(|error| error.to_string())?;
    let remote = open(profile);
    let blocks = block::server_blocks(&remote)?;
    Ok((remote, blocks))
}

/// Plan a block's version change, or, with `apply`, lock the workspace, plan it again against
/// the configuration as it is then, and write it.
pub fn version(
    solution: &Solution,
    name: &str,
    change: &VersionChange,
    apply: bool,
    notices: &mut Notices,
) -> Result<VersionPlan, BlockCommandError> {
    if !apply {
        return plan(solution, name, change);
    }
    let lock =
        lock_workspace(solution, "block version", notices).map_err(BlockCommandError::Lock)?;
    let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
        .map_err(BlockCommandError::Config)?;
    let planned = plan(&reloaded, name, change)?;
    block::apply_version(&reloaded, &planned, &lock).map_err(BlockCommandError::Write)?;
    Ok(planned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_block_is_an_unknown_entity_and_a_held_lock_is_reported_as_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        let change = VersionChange::Set("1.0.1".to_string());
        let error =
            version(&solution, "Nope", &change, false, &mut Notices::default()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnknownEntity);
        let held = lock::acquire(dir.path(), "holder", &[]).unwrap();
        let error = version(&solution, "Nope", &change, true, &mut Notices::default()).unwrap_err();
        assert!(matches!(error, BlockCommandError::Lock(_)), "{error}");
        drop(held);
        let ambiguous = BlockCommandError::Find(block::FindError::Ambiguous("two".to_string()));
        assert_eq!(ambiguous.code(), ErrorCode::Ambiguous);
    }
}
