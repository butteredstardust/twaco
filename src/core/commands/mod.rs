//! Command contracts shared by the command-line and MCP adapters.

pub mod adopt;
pub mod bundle;
pub mod call;
pub mod carry;
pub mod config_table;
pub mod datatable_copy;
pub mod db;
pub mod delete;
pub mod deploy;
pub mod export;
pub mod extensions;
pub mod extract;
pub mod fmt;
pub mod imports;
pub mod logs;
pub mod newblock;
pub mod package;
pub mod permissions;
pub mod push;
pub mod relocate;
pub mod rename;
pub mod repo;
pub mod restore;
pub mod retemplate;
pub mod status;
pub mod sync;
pub mod types;

use super::config::Solution;
use super::lock::{self, LockError, WorkspaceLock};

/// Whether a command describes a change or carries it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Plan,
    Apply,
}

/// The level of access an outcome used or may have used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    None,
    Read,
    Write,
}

/// The workspace and server access associated with a command outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effects {
    pub workspace: Access,
    pub server: Access,
}

impl Effects {
    pub const fn new(workspace: Access, server: Access) -> Self {
        Self { workspace, server }
    }
}

/// What an executor has to tell the person that is not its result: files swept after an
/// interrupted write, and interrupted operations finished or undone, when it took the workspace
/// lock. Filled when the lock is taken, so it is there even if the command then fails.
#[derive(Debug, Default)]
pub struct Notices(Vec<String>);

impl Notices {
    pub fn lines(&self) -> &[String] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Take the workspace lock for a command that changes the workspace, recording what taking it
/// did. Every executor takes its lock here, so none of them is a second place that decides how.
pub fn lock_workspace(
    solution: &Solution,
    label: &str,
    notices: &mut Notices,
) -> Result<WorkspaceLock, LockError> {
    let held = lock::acquire_for(solution, label)?;
    for path in &held.recovered {
        notices.0.push(format!(
            "removed {}, left by an interrupted write",
            path.display()
        ));
    }
    notices.0.extend(held.recovery.iter().cloned());
    Ok(held)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taking_the_lock_records_what_it_swept_and_what_it_recovered() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-commands-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(root.join(".twaco/.baseline.json.1.twaco-tmp"), b"half").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let mut notices = Notices::default();
        let held = lock_workspace(&solution, "test", &mut notices).unwrap();
        assert_eq!(notices.lines().len(), 1, "{:?}", notices.lines());
        assert!(
            notices.lines()[0].contains("left by an interrupted write"),
            "{:?}",
            notices.lines()
        );
        drop(held);
        let mut quiet = Notices::default();
        drop(lock_workspace(&solution, "test", &mut quiet).unwrap());
        assert!(quiet.is_empty());
    }
}
