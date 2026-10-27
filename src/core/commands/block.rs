//! The command policy around a solution's building blocks: the server's view, and writing a
//! block's version under the workspace lock.

use super::{lock_workspace, Notices};
use crate::core::block::{self, Remote, VersionChange, VersionPlan};
use crate::core::config::{self, Solution};
use crate::core::profile;

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
) -> Result<VersionPlan, String> {
    if !apply {
        return block::plan_version(solution, name, change);
    }
    let lock =
        lock_workspace(solution, "block version", notices).map_err(|error| error.to_string())?;
    let reloaded = Solution::load(&solution.root.join(config::CONFIG_FILE))
        .map_err(|error| error.to_string())?;
    let plan = block::plan_version(&reloaded, name, change)?;
    block::apply_version(&reloaded, &plan, &lock)?;
    Ok(plan)
}
