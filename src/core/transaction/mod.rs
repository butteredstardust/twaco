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
    /// Where a move puts what is at `path`.
    to: Option<String>,
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
        /// Paths that could not be put back because something else is there now.
        leftover: Vec<PathBuf>,
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
                leftover,
            } => {
                write!(f, "{why}; some changes could not be undone")?;
                if !leftover.is_empty() {
                    let names: Vec<String> = leftover
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect();
                    write!(f, ": {}", names.join(", "))?;
                }
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

    /// Make a file hold `after`: a replacement of what it holds now (read here, and checked again
    /// when the transaction is applied), or a new file when there is none.
    pub fn write_file(&mut self, path: &Path, after: Vec<u8>) -> Result<(), TransactionError> {
        match std::fs::read(path) {
            Ok(before) => self.replace_file(path, &before, after),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.create_file(path, after)
            }
            Err(error) => Err(TransactionError::Io {
                path: path.to_path_buf(),
                why: error.to_string(),
            }),
        }
    }

    /// Remove a file that holds `expected_before`.
    pub fn delete_file(
        &mut self,
        path: &Path,
        expected_before: &[u8],
    ) -> Result<(), TransactionError> {
        self.plan(Kind::Delete, path, Some(expected_before.to_vec()), None)
    }

    /// Rename a file or a whole folder. Anything the operation also rewrites inside it is named
    /// at its old path and rewritten first; the rename comes after.
    pub fn move_path(&mut self, from: &Path, to: &Path) -> Result<(), TransactionError> {
        let destination = paths::relative(self.root, to).map_err(TransactionError::Invalid)?;
        paths::reject_links(self.root, &destination).map_err(TransactionError::Invalid)?;
        self.plan_to(Kind::Move, from, Some(destination), None, None)
    }

    fn plan(
        &mut self,
        kind: Kind,
        path: &Path,
        before: Option<Vec<u8>>,
        after: Option<Vec<u8>>,
    ) -> Result<(), TransactionError> {
        self.plan_to(kind, path, None, before, after)
    }

    fn plan_to(
        &mut self,
        kind: Kind,
        path: &Path,
        to: Option<String>,
        before: Option<Vec<u8>>,
        after: Option<Vec<u8>>,
    ) -> Result<(), TransactionError> {
        #[cfg(test)]
        if let Some(after) = &after {
            crate::xml_oracle::check_write(path, after);
        }
        let relative = paths::relative(self.root, path).map_err(TransactionError::Invalid)?;
        paths::reject_links(self.root, &relative).map_err(TransactionError::Invalid)?;
        // A file may be rewritten and also renamed (with the folder it is in); nothing else is
        // named twice.
        if self.planned.iter().any(|planned| {
            planned.path == relative && (planned.kind == Kind::Move) == (kind == Kind::Move)
        }) {
            return Err(TransactionError::Invalid(format!(
                "{relative} is named twice in one operation"
            )));
        }
        self.planned.push(Planned {
            kind,
            path: relative,
            to,
            before,
            after,
        });
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.planned.is_empty()
    }

    /// How many steps are planned.
    pub fn len(&self) -> usize {
        self.planned.len()
    }

    /// Carry the operation out. The lock proves no other twaco is writing; taking it also ran any
    /// recovery an earlier crash needed.
    pub fn apply(self, lock: &WorkspaceLock) -> Result<Committed, TransactionError> {
        self.apply_with(lock, &mut |_, _| Ok(()))
    }

    /// As `apply`, calling `before_step` with each step's position and the step just before it is
    /// made. An error from it stops the operation and undoes what was done: how a caller makes
    /// a failure happen at an exact point.
    pub fn apply_with(
        self,
        lock: &WorkspaceLock,
        before_step: &mut dyn FnMut(usize, &Step) -> std::io::Result<()>,
    ) -> Result<Committed, TransactionError> {
        if !lock.covers(self.root) {
            return Err(TransactionError::Invalid(
                "the workspace lock held is not the one for this workspace".to_string(),
            ));
        }
        if self.planned.is_empty() {
            return Ok(Committed {
                operation_id: String::new(),
                steps: 0,
            });
        }
        let mut journal = self.stage()?;
        let operation_id = journal.operation_id.clone();
        let steps = journal.steps.len();
        if let Err(stop) = self.install_all(&mut journal, before_step) {
            return Err(self.undo(journal, stop));
        }
        journal.state = State::Committed;
        journal::write(self.root, &journal).map_err(|why| self.io(&journal, why))?;
        tracing::debug!(operation = %operation_id, steps, "transaction committed");
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
        let io = |path: &str, why: std::io::Error| TransactionError::Io {
            path: paths::absolute(self.root, path),
            why: why.to_string(),
        };
        for planned in &self.planned {
            if planned.kind == Kind::Move {
                continue;
            }
            let found = recover::observe(&paths::absolute(self.root, &planned.path))
                .map_err(|why| io(&planned.path, why))?;
            let expected = planned.before.as_deref().map(digest);
            if found != expected {
                return Err(TransactionError::Stale(match planned.kind {
                    Kind::Create => format!("{} exists already", planned.path),
                    _ => format!("{} changed since it was read", planned.path),
                }));
            }
        }
        // What the files a move carries will hold by the time it is made.
        let rewritten: std::collections::BTreeMap<String, Option<Vec<u8>>> = self
            .planned
            .iter()
            .filter_map(|planned| match planned.kind {
                Kind::Replace => Some((planned.path.clone(), planned.after.clone())),
                Kind::Delete => Some((planned.path.clone(), None)),
                _ => None,
            })
            .collect();
        let mut moves = Vec::new();
        for planned in self.planned.iter().filter(|p| p.kind == Kind::Move) {
            let to = planned.to.as_deref().expect("a move has a destination");
            if std::fs::symlink_metadata(paths::absolute(self.root, &planned.path)).is_err() {
                return Err(TransactionError::Stale(format!(
                    "{} changed since it was read",
                    planned.path
                )));
            }
            if std::fs::symlink_metadata(paths::absolute(self.root, to)).is_ok() {
                return Err(TransactionError::Stale(format!("{to} exists already")));
            }
            let tree = journal::tree_digest(self.root, &planned.path, &rewritten)
                .map_err(|why| io(&planned.path, why))?;
            moves.push((planned.path.clone(), to.to_string(), tree));
        }
        let inside = |path: &str, folder: &str| {
            path == folder
                || path
                    .strip_prefix(folder)
                    .is_some_and(|rest| rest.starts_with('/'))
        };
        let operation_id = new_operation_id();
        let mut steps = Vec::new();
        let mut claimed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (index, planned) in self.planned.iter().enumerate() {
            let carried = moves
                .iter()
                .find(|(from, _, _)| planned.kind != Kind::Move && inside(&planned.path, from));
            if planned.kind == Kind::Create && carried.is_some() {
                return Err(TransactionError::Invalid(format!(
                    "{} would be created inside a folder the same operation moves",
                    planned.path
                )));
            }
            let tree = moves
                .iter()
                .find(|(from, _, _)| planned.kind == Kind::Move && *from == planned.path)
                .map(|(_, _, tree)| tree.clone());
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
                before: tree
                    .clone()
                    .or_else(|| planned.before.as_deref().map(digest)),
                after: tree.or_else(|| planned.after.as_deref().map(digest)),
                stage,
                backup,
                to: planned.to.clone(),
                then_at: carried.map(|(from, to, _)| {
                    format!(
                        "{to}{}",
                        planned.path.strip_prefix(from.as_str()).unwrap_or("")
                    )
                }),
                // A folder two files need is made, and later removed, by the first of them.
                new_dirs: match (planned.kind, planned.to.as_deref()) {
                    (Kind::Create, _) => paths::missing_folders(self.root, &planned.path),
                    (Kind::Move, Some(to)) => paths::missing_folders(self.root, to),
                    _ => Vec::new(),
                }
                .into_iter()
                .filter(|folder| claimed.insert(folder.clone()))
                .collect(),
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
        tracing::debug!(
            operation = %journal.operation_id,
            command = self.command,
            steps = journal.steps.len(),
            "transaction journal written"
        );
        failpoint!("after-journal");
        if let Err(error) = self.write_artifacts(&journal) {
            recover::clean(self.root, &journal, true);
            return Err(error);
        }
        tracing::debug!(operation = %journal.operation_id, "transaction staged");
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

    fn install_all(
        &self,
        journal: &mut Journal,
        before_step: &mut dyn FnMut(usize, &Step) -> std::io::Result<()>,
    ) -> Result<(), recover::Stop> {
        for at in 0..journal.steps.len() {
            let step = journal.steps[at].clone();
            before_step(at, &step)
                .map_err(|why| recover::Stop::Failed(format!("{}: {why}", step.path)))?;
            // Looked at again right before the change: an editor can save the file after the
            // plan was checked.
            match recover::standing(self.root, &step) {
                Ok(recover::Standing::Before) => {}
                Ok(_) => return Err(recover::Stop::Stale(recover::stale_text(&step))),
                Err(why) => return Err(recover::Stop::Failed(format!("{}: {why}", step.path))),
            }
            recover::install(self.root, &step)?;
            tracing::debug!(
                operation = %journal.operation_id,
                step = step.id,
                kind = ?step.kind,
                path = %step.path,
                "transaction step installed"
            );
            failpoint!("after-step-{}-visible", step.id);
            journal.steps[at].completed = true;
            journal::write(self.root, journal)
                .map_err(|why| recover::Stop::Failed(format!("{}: {why}", step.path)))?;
            failpoint!("after-step-{}-marked", step.id);
        }
        Ok(())
    }

    /// A step failed: put back what was changed, under the same lock. A file somebody saved since
    /// the operation wrote it is never overwritten: it is named instead, and the journal stays.
    fn undo(&self, journal: Journal, stop: recover::Stop) -> TransactionError {
        let leftover = recover::undo_installed(self.root, &journal);
        tracing::warn!(
            operation = %journal.operation_id,
            rolled_back = leftover.is_empty(),
            leftover = leftover.len(),
            "transaction step failed"
        );
        if leftover.is_empty() {
            recover::clean(self.root, &journal, true);
            return match stop {
                recover::Stop::Stale(why) => TransactionError::Stale(why),
                recover::Stop::Failed(why) => TransactionError::Failed {
                    why,
                    rolled_back: true,
                    journal: None,
                    leftover,
                },
            };
        }
        let (recover::Stop::Stale(why) | recover::Stop::Failed(why)) = stop;
        TransactionError::Failed {
            why,
            rolled_back: false,
            journal: Some(journal::path_of(self.root, &journal.operation_id)),
            leftover,
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
