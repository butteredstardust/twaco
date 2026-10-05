//! Local file changes that survive a crash.
//!
//! A command that must change several files (a rename, a new building block) used to undo a
//! failure by remembering, in memory, what it had already written. A killed process or a power
//! cut between two writes ran no undo and left the workspace half changed. A transaction writes
//! its intent to disk first, so the next command to take the workspace lock can finish or undo
//! what was interrupted.
//!
//! The protocol, with the workspace lock held throughout:
//!
//! 1. every path is checked (inside the workspace, no link on the way) and every file the
//!    operation expects to find is read and compared with the bytes it was planned against;
//! 2. a journal in the `staging` state is written, then the new bytes of every file are staged
//!    beside its destination and a copy of every original is kept, each synced and re-read;
//! 3. the journal moves to `applying`, and the destinations are changed one at a time by renaming
//!    a staged file over them (or removing them), the journal noting each step;
//! 4. the journal moves to `committed`, and the stages, backups and journal are removed.
//!
//! Recovery reads digests, not progress marks, to know where an interrupted operation stands: a
//! destination holds its before state, its after state, or something else. It rolls forward when
//! every file is in one of the first two and the bytes still to be installed are staged; it
//! rolls back when they are not and every installed file has a backup; and it refuses, naming
//! every path, when a file holds anything else, because that is a person's edit. See
//! `documentation/TRANSACTIONS.md`.
//!
//! This covers local files only. A server call is never part of a transaction.

/// Abort the process at a named point of a transaction. Present only when tests ask for it.
#[cfg(feature = "test-failpoints")]
macro_rules! failpoint {
    ($($name:tt)*) => {
        if std::env::var("TWACO_TEST_FAILPOINT").is_ok_and(|point| point == format!($($name)*)) {
            std::process::abort();
        }
    };
}

#[cfg(not(feature = "test-failpoints"))]
macro_rules! failpoint {
    ($($name:tt)*) => {};
}

mod journal;
mod paths;
mod recover;
#[cfg(test)]
mod tests;

pub use journal::{Format, Journal, Kind, State, Step, DIRECTORY};
pub use recover::{recover_pending, Action, Recovered, Refusal};

use super::lock::WorkspaceLock;
use journal::digest;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The files a command means to change, gathered before anything is written.
pub struct Transaction<'a> {
    root: &'a Path,
    command: &'a str,
    planned: Vec<Planned>,
}

struct Planned {
    kind: Kind,
    path: String,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

/// What an applied transaction did.
#[derive(Debug, PartialEq, Eq)]
pub struct Committed {
    pub operation_id: String,
    pub steps: usize,
}

#[derive(Debug)]
pub enum TransactionError {
    /// A path that may not be named, or a file named twice.
    Invalid(String),
    /// A file is not what the operation was planned against; nothing was changed.
    Stale(String),
    Io {
        path: PathBuf,
        why: String,
    },
    /// A step failed. `rolled_back` says whether the workspace is as it was; when it is not, the
    /// journal at `journal` records what is left and the next command that takes the lock will
    /// finish or refuse.
    Failed {
        why: String,
        rolled_back: bool,
        journal: Option<PathBuf>,
    },
}

impl fmt::Display for TransactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(why) | Self::Stale(why) => f.write_str(why),
            Self::Io { path, why } => write!(f, "{}: {why}", path.display()),
            Self::Failed {
                why,
                rolled_back: true,
                ..
            } => write!(f, "{why}; every change was undone"),
            Self::Failed {
                why,
                rolled_back: false,
                journal,
            } => {
                write!(f, "{why}; some changes could not be undone")?;
                if let Some(journal) = journal {
                    write!(
                        f,
                        " (the journal {} records them; the next twaco command that changes \
                         this workspace finishes or refuses the recovery)",
                        journal.display()
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for TransactionError {}

impl<'a> Transaction<'a> {
    pub fn new(root: &'a Path, command: &'a str) -> Self {
        Transaction {
            root,
            command,
            planned: Vec::new(),
        }
    }

    /// Replace a file that holds `expected_before` with `after`. Identical bytes change nothing
    /// and add no step.
    pub fn replace_file(
        &mut self,
        path: &Path,
        expected_before: &[u8],
        after: Vec<u8>,
    ) -> Result<(), TransactionError> {
        if expected_before == after.as_slice() {
            return Ok(());
        }
        self.plan(
            Kind::Replace,
            path,
            Some(expected_before.to_vec()),
            Some(after),
        )
    }

    /// Create a file that must not exist yet. Folders on the way are created, and removed again
    /// if the transaction is undone.
    pub fn create_file(&mut self, path: &Path, after: Vec<u8>) -> Result<(), TransactionError> {
        self.plan(Kind::Create, path, None, Some(after))
    }

    /// Remove a file that holds `expected_before`.
    pub fn delete_file(
        &mut self,
        path: &Path,
        expected_before: &[u8],
    ) -> Result<(), TransactionError> {
        self.plan(Kind::Delete, path, Some(expected_before.to_vec()), None)
    }

    fn plan(
        &mut self,
        kind: Kind,
        path: &Path,
        before: Option<Vec<u8>>,
        after: Option<Vec<u8>>,
    ) -> Result<(), TransactionError> {
        let relative = paths::relative(self.root, path).map_err(TransactionError::Invalid)?;
        paths::reject_links(self.root, &relative).map_err(TransactionError::Invalid)?;
        if self.planned.iter().any(|planned| planned.path == relative) {
            return Err(TransactionError::Invalid(format!(
                "{relative} is named twice in one operation"
            )));
        }
        self.planned.push(Planned {
            kind,
            path: relative,
            before,
            after,
        });
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.planned.is_empty()
    }

    /// Carry the operation out. The lock proves no other twaco is writing; taking it also ran any
    /// recovery an earlier crash needed.
    pub fn apply(self, _lock: &WorkspaceLock) -> Result<Committed, TransactionError> {
        if self.planned.is_empty() {
            return Ok(Committed {
                operation_id: String::new(),
                steps: 0,
            });
        }
        let mut journal = self.stage()?;
        let operation_id = journal.operation_id.clone();
        let steps = journal.steps.len();
        if let Err(why) = self.install_all(&mut journal) {
            return Err(self.undo(journal, why));
        }
        journal.state = State::Committed;
        journal::write(self.root, &journal).map_err(|why| self.io(&journal, why))?;
        failpoint!("after-commit");
        recover::clean(self.root, &journal, false);
        Ok(Committed {
            operation_id,
            steps,
        })
    }

    fn io(&self, journal: &Journal, why: std::io::Error) -> TransactionError {
        TransactionError::Io {
            path: journal::path_of(self.root, &journal.operation_id),
            why: why.to_string(),
        }
    }

    /// Check every file against the plan, write the journal, then stage the new bytes and the
    /// backups. Nothing a user can see has changed when this returns.
    fn stage(&self) -> Result<Journal, TransactionError> {
        for planned in &self.planned {
            let found =
                recover::observe(&paths::absolute(self.root, &planned.path)).map_err(|why| {
                    TransactionError::Io {
                        path: paths::absolute(self.root, &planned.path),
                        why: why.to_string(),
                    }
                })?;
            let expected = planned.before.as_deref().map(digest);
            if found != expected {
                return Err(TransactionError::Stale(match planned.kind {
                    Kind::Create => format!("{} exists already", planned.path),
                    _ => format!("{} changed since it was read", planned.path),
                }));
            }
        }
        let operation_id = new_operation_id();
        let mut steps = Vec::new();
        for (index, planned) in self.planned.iter().enumerate() {
            let (stage, backup) = (
                planned
                    .after
                    .as_ref()
                    .map(|_| artifact(&planned.path, &operation_id, "twaco-stage")),
                planned
                    .before
                    .as_ref()
                    .map(|_| artifact(&planned.path, &operation_id, "twaco-backup")),
            );
            steps.push(Step {
                id: index + 1,
                kind: planned.kind,
                path: planned.path.clone(),
                before: planned.before.as_deref().map(digest),
                after: planned.after.as_deref().map(digest),
                stage,
                backup,
                new_dirs: if planned.kind == Kind::Create {
                    paths::missing_folders(self.root, &planned.path)
                } else {
                    Vec::new()
                },
                completed: false,
            });
        }
        let mut journal = Journal {
            format: Format {
                major: journal::MAJOR,
                minor: journal::MINOR,
            },
            operation_id,
            command: self.command.to_string(),
            state: State::Staging,
            steps,
        };
        journal::write(self.root, &journal).map_err(|why| self.io(&journal, why))?;
        failpoint!("after-journal");
        if let Err(error) = self.write_artifacts(&journal) {
            recover::clean(self.root, &journal, true);
            return Err(error);
        }
        failpoint!("after-stage");
        journal.state = State::Applying;
        if let Err(why) = journal::write(self.root, &journal) {
            recover::clean(self.root, &journal, true);
            return Err(self.io(&journal, why));
        }
        failpoint!("after-applying");
        Ok(journal)
    }

    fn write_artifacts(&self, journal: &Journal) -> Result<(), TransactionError> {
        for (step, planned) in journal.steps.iter().zip(&self.planned) {
            for folder in &step.new_dirs {
                let path = paths::absolute(self.root, folder);
                std::fs::create_dir(&path).map_err(|why| TransactionError::Io {
                    path,
                    why: why.to_string(),
                })?;
            }
            let destination = paths::absolute(self.root, &step.path);
            if let (Some(stage), Some(bytes)) = (&step.stage, &planned.after) {
                write_artifact(self.root, stage, bytes, Some(&destination))?;
            }
            if let (Some(backup), Some(bytes)) = (&step.backup, &planned.before) {
                write_artifact(self.root, backup, bytes, None)?;
            }
        }
        Ok(())
    }

    fn install_all(&self, journal: &mut Journal) -> Result<(), String> {
        for at in 0..journal.steps.len() {
            let step = journal.steps[at].clone();
            recover::install(self.root, &step)?;
            failpoint!("after-step-{}-visible", step.id);
            journal.steps[at].completed = true;
            journal::write(self.root, journal).map_err(|why| format!("{}: {why}", step.path))?;
            failpoint!("after-step-{}-marked", step.id);
        }
        Ok(())
    }

    /// A step failed: put back what was changed, under the same lock.
    fn undo(&self, journal: Journal, why: String) -> TransactionError {
        match recover::resolve(self.root, &journal, recover::Prefer::Backward) {
            Ok(_) => TransactionError::Failed {
                why,
                rolled_back: true,
                journal: None,
            },
            Err(_) => TransactionError::Failed {
                why,
                rolled_back: false,
                journal: Some(journal::path_of(self.root, &journal.operation_id)),
            },
        }
    }
}

fn new_operation_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ"),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// `<folder>/.<file>.<operation>.<suffix>`: a hidden sibling of the destination, so a rename
/// from it never crosses a filesystem.
fn artifact(path: &str, operation_id: &str, suffix: &str) -> String {
    match path.rsplit_once('/') {
        Some((folder, file)) => format!("{folder}/.{file}.{operation_id}.{suffix}"),
        None => format!(".{path}.{operation_id}.{suffix}"),
    }
}

/// Write `bytes` to a new hidden file, sync it and read it back. A staged file takes the mode of
/// the file it will replace.
fn write_artifact(
    root: &Path,
    relative: &str,
    bytes: &[u8],
    mode_from: Option<&Path>,
) -> Result<(), TransactionError> {
    let path = paths::absolute(root, relative);
    let io = |why: std::io::Error| TransactionError::Io {
        path: path.clone(),
        why: why.to_string(),
    };
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, bytes).and_then(|()| file.sync_all())
        })
        .map_err(io)?;
    if let Some(metadata) = mode_from.and_then(|original| std::fs::metadata(original).ok()) {
        let _ = std::fs::set_permissions(&path, metadata.permissions());
    }
    let written = std::fs::read(&path).map_err(io)?;
    if written != bytes {
        return Err(TransactionError::Io {
            path: path.clone(),
            why: "the file does not hold what was written to it".to_string(),
        });
    }
    Ok(())
}
