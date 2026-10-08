//! One writer at a time per workspace.
//!
//! Two syncs, or a sync and a deploy, can interleave the moment twaco runs from more than one
//! terminal, and a long-lived MCP server makes it routine: each one reads a file, decides, and
//! writes, and the second writer silently undoes the first. So every command that writes takes
//! `.twaco/lock` for its whole run, before it reads anything, and a second one refuses rather
//! than waits (beyond a fifth of a second, to let a diagnostic glance pass).
//!
//! The lock is the operating system's, taken on an open file with `File::try_lock`. It is
//! released when the process exits, however it exits. So a crash never leaves a stale lock for a
//! person to find and delete by hand, which is what makes an exclusive lock tolerable.
//!
//! Who holds it is written to a separate `lock.holder` file. It cannot go in the lock file:
//! Windows locks are mandatory, so no other handle can read a locked file, not even one opened
//! by the same process. The holder file is informational only; the lock is the handle.
//!
//! While the lock is held nobody else can be mid-write, so leftover `*.twaco-tmp` files, from a
//! write interrupted between creating its temporary and renaming it into place, are safe to
//! remove. That is the stale-temp recovery half of 8.8.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const RELATIVE_PATH: &str = ".twaco/lock";
pub const HOLDER_PATH: &str = ".twaco/lock.holder";
const TEMPORARY_SUFFIX: &str = ".twaco-tmp";
const LEGACY_SUFFIXES: [&str; 2] = [".twaco-rename-tmp", ".twaco-delete-tmp"];

/// Held for as long as it lives. Dropping it closes the file, which releases the lock.
#[derive(Debug)]
pub struct WorkspaceLock {
    _file: File,
    /// Leftover temporaries removed when the lock was taken.
    pub recovered: Vec<PathBuf>,
    /// Interrupted operations finished or undone when the lock was taken, one line each.
    pub recovery: Vec<String>,
    root: PathBuf,
    command: String,
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        tracing::debug!(command = %self.command, "workspace lock released");
    }
}

impl WorkspaceLock {
    /// Whether this lock is the one for the workspace at `root`.
    pub fn covers(&self, root: &Path) -> bool {
        match (
            std::fs::canonicalize(&self.root),
            std::fs::canonicalize(root),
        ) {
            (Ok(held), Ok(asked)) => held == asked,
            _ => self.root == root,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another process holds it. `holder` is what that process wrote, when it could be read.
    #[error(
        "another twaco command is changing this workspace ({holder}); run this one when \
                 it finishes"
    )]
    Held { holder: String },
    #[error("{}: {why}", .path.display())]
    Io { path: PathBuf, why: String },
    /// An interrupted operation could not be finished or undone safely. The text says why and
    /// what a person must do.
    #[error("{message}")]
    Recovery { message: String },
}

/// Take the workspace lock for `command`, then sweep `sweep` for stale temporaries.
///
/// `sweep` is where twaco writes through a temporary: `.twaco`, the entity folders and the
/// sidecar tree. Only twaco's own temporaries are removed, and only once the lock is held.
pub fn acquire(root: &Path, command: &str, sweep: &[PathBuf]) -> Result<WorkspaceLock, LockError> {
    acquire_then_sweep(root, command, || sweep.to_vec())
}

/// How often, and how far apart, a contended lock is tried again before refusing. `doctor`
/// takes the lock for an instant to see whether it is free, and a writer starting in that
/// instant must not be refused for it. A real holder outlasts the retries by far.
const ATTEMPTS: u32 = 5;
const PAUSE: std::time::Duration = std::time::Duration::from_millis(40);

/// `sweep` is called only once the lock is held, so what it finds cannot change under it.
fn acquire_then_sweep(
    root: &Path,
    command: &str,
    sweep: impl FnOnce() -> Vec<PathBuf>,
) -> Result<WorkspaceLock, LockError> {
    let path = root.join(RELATIVE_PATH);
    let io = |why: std::io::Error| LockError::Io {
        path: path.clone(),
        why: why.to_string(),
    };
    std::fs::create_dir_all(path.parent().expect("the lock has a parent")).map_err(io)?;
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(io)?;
    let holder_path = root.join(HOLDER_PATH);
    let mut attempt = 1;
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if attempt < ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(PAUSE);
            }
            Err(TryLockError::WouldBlock) => {
                let holder = std::fs::read_to_string(&holder_path).unwrap_or_default();
                let holder = holder.trim();
                tracing::debug!(command, holder, "workspace lock refused");
                return Err(LockError::Held {
                    holder: if holder.is_empty() {
                        "holder unknown".to_string()
                    } else {
                        holder.to_string()
                    },
                });
            }
            Err(TryLockError::Error(error)) => return Err(io(error)),
        }
    }
    tracing::debug!(command, attempts = attempt, "workspace lock taken");
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let record = format!(
        "twaco {command}, pid {}, started at unix time {started}\n",
        std::process::id()
    );
    // Written beside it and renamed over it, so a contender never reads half a record.
    // Informational; a failure to say who holds the lock must not fail the command holding it.
    let staged = root.join(format!(".twaco/.lock.holder{TEMPORARY_SUFFIX}"));
    if std::fs::write(&staged, record.as_bytes())
        .and_then(|()| std::fs::rename(&staged, &holder_path))
        .is_err()
    {
        // Windows refuses the rename while a reader holds the record open without sharing
        // deletion. Writing in place can be read half-done, but never names the last holder.
        let _ = std::fs::remove_file(&staged);
        if std::fs::write(&holder_path, record.as_bytes()).is_err() {
            // Neither way worked: better no record ("holder unknown") than the last holder's.
            let _ = std::fs::remove_file(&holder_path);
        }
    }

    // An operation a crash interrupted is finished or undone before anything else is read or
    // swept: its stages and backups are not stale temporaries.
    let recovery = super::transaction::recover_pending(root)
        .map_err(|refusal| LockError::Recovery { message: refusal.0 })?
        .iter()
        .map(super::transaction::Recovered::describe)
        .collect::<Vec<String>>();

    // Only folders inside the workspace are swept. A configured folder that is a link to
    // somewhere else is left alone: what lies there is not this lock's to recover.
    let mut recovered = Vec::new();
    if let Ok(workspace) = std::fs::canonicalize(root) {
        for directory in std::iter::once(root.join(".twaco")).chain(sweep()) {
            if std::fs::canonicalize(&directory).is_ok_and(|real| real.starts_with(&workspace)) {
                remove_temporaries(&directory, &mut recovered);
            }
        }
    }
    Ok(WorkspaceLock {
        _file: file,
        recovered,
        recovery,
        root: root.to_path_buf(),
        command: command.to_string(),
    })
}

/// Who holds the workspace lock, if anyone, without taking it or sweeping anything.
///
/// For diagnosis only: the answer can be stale the moment it is returned. `Ok(None)` means
/// nobody held it at the instant of asking.
pub fn holder(root: &Path) -> Result<Option<String>, LockError> {
    let path = root.join(RELATIVE_PATH);
    if !path.exists() {
        return Ok(None);
    }
    let io = |why: std::io::Error| LockError::Io {
        path: path.clone(),
        why: why.to_string(),
    };
    let file = OpenOptions::new().write(true).open(&path).map_err(io)?;
    match file.try_lock() {
        // Released as `file` drops at the end of this function.
        Ok(()) => Ok(None),
        Err(TryLockError::WouldBlock) => {
            let holder = std::fs::read_to_string(root.join(HOLDER_PATH)).unwrap_or_default();
            Ok(Some(if holder.trim().is_empty() {
                "holder unknown".to_string()
            } else {
                holder.trim().to_string()
            }))
        }
        Err(TryLockError::Error(error)) => Err(io(error)),
    }
}

/// Take the lock for a solution, sweeping everywhere twaco writes through a temporary: the entity
/// folders and the sidecar tree (and `.twaco`, always). Used by the CLI and the MCP server alike.
pub fn acquire_for(
    solution: &super::config::Solution,
    command: &str,
) -> Result<WorkspaceLock, LockError> {
    acquire_then_sweep(&solution.root, command, || {
        let mut sweep: Vec<PathBuf> = super::workspace::discover(solution)
            .entities
            .iter()
            .filter_map(|entity| entity.path.parent().map(Path::to_path_buf))
            .collect();
        sweep.sort();
        sweep.dedup();
        sweep.push(solution.src_root());
        sweep
    })
}

/// twaco's own temporaries are hidden siblings of their target: `.<name>[.<pid>...].twaco-tmp`.
/// Older versions also left `.twaco-rename-tmp` and `.twaco-delete-tmp`; those are recognised too.
fn is_temporary(name: &str) -> bool {
    name.starts_with('.')
        && LEGACY_SUFFIXES
            .iter()
            .chain([&TEMPORARY_SUFFIX])
            .any(|suffix| name.ends_with(suffix))
}

fn remove_temporaries(directory: &Path, recovered: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            remove_temporaries(&path, recovered);
        } else if kind.is_file()
            && path
                .file_name()
                .is_some_and(|name| is_temporary(&name.to_string_lossy()))
            && std::fs::remove_file(&path).is_ok()
        {
            tracing::warn!(path = %path.display(), "removed a stale temporary left by an interrupted write");
            recovered.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> (tempfile::TempDir, PathBuf) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-lock-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        (root_guard, root)
    }

    #[test]
    fn a_second_writer_is_refused_and_told_who_holds_it() {
        let (_dir, root) = temp();
        let first = acquire(&root, "sync", &[]).unwrap();
        let error = acquire(&root, "deploy", &[]).unwrap_err();
        match error {
            LockError::Held { holder } => {
                assert!(holder.contains("twaco sync"), "{holder}");
                assert!(
                    holder.contains(&format!("pid {}", std::process::id())),
                    "{holder}"
                );
            }
            other => panic!("expected Held, got {other}"),
        }
        drop(first);
    }

    #[test]
    fn the_lock_is_free_again_once_its_holder_is_dropped() {
        let (_dir, root) = temp();
        drop(acquire(&root, "sync", &[]).unwrap());
        let again = acquire(&root, "fmt", &[]).unwrap();
        let text = std::fs::read_to_string(root.join(HOLDER_PATH)).unwrap();
        assert!(
            text.starts_with("twaco fmt,"),
            "the new holder replaces the old record: {text}"
        );
        drop(again);
    }

    #[test]
    fn a_transactions_stages_and_backups_are_not_stale_temporaries() {
        assert!(!is_temporary(".a.txt.20261005T093015Z-1-0.twaco-stage"));
        assert!(!is_temporary(".a.txt.20261005T093015Z-1-0.twaco-backup"));
        assert!(is_temporary(".a.txt.42-7-0.twaco-tmp"));
    }

    #[test]
    fn stale_temporaries_are_removed_and_nothing_else() {
        let (_dir, root) = temp();
        let things = root.join("Things");
        let nested = root.join("src/T/services/S");
        std::fs::create_dir_all(&things).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        let stale = [
            things.join(".T.xml.twaco-tmp"),
            nested.join(".script.js.twaco-tmp"),
            root.join(".twaco/.baseline.json.42.twaco-tmp"),
            root.join(".twaco/.renames.json.42.twaco-rename-tmp"),
            root.join(".twaco/.renames.json.42.twaco-delete-tmp"),
        ];
        // Outside the workspace, nothing is twaco's to recover, even with twaco's own name.
        let (_dir, outside) = temp();
        let kept = [
            things.join("T.xml"),
            nested.join("script.js"),
            root.join(".twaco/baseline.json"),
            things.join("notes.twaco-tmp"),
            outside.join(".T.xml.twaco-tmp"),
        ];
        for path in stale.iter().chain(&kept) {
            std::fs::write(path, b"x").unwrap();
        }
        let lock = acquire(&root, "sync", &[things, root.join("src"), outside.clone()]).unwrap();
        assert_eq!(lock.recovered.len(), 5);
        assert!(stale.iter().all(|path| !path.exists()));
        assert!(kept.iter().all(|path| path.exists()));
        drop(lock);
    }

    #[test]
    fn the_lock_logs_each_step_and_warns_about_a_stale_temporary() {
        let (_dir, root) = temp();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join(".twaco/.old.json.7.twaco-tmp"), b"x").unwrap();
        let (_, logs) = crate::core::diagnostics::captured(|| {
            let span = tracing::info_span!("capture", mine = "lock-logs-steps");
            let _entered = span.enter();
            let lock = acquire(&root, "sync", &[]).unwrap();
            assert!(matches!(
                acquire(&root, "fmt", &[]),
                Err(LockError::Held { .. })
            ));
            drop(lock);
        });
        let line = |needle: &str| {
            logs.lines()
                .find(|line| line.contains(needle) && line.contains("lock-logs-steps"))
        };
        assert!(
            line("workspace lock taken").unwrap().contains("DEBUG"),
            "{logs}"
        );
        let mine = root.display().to_string();
        let warning = logs
            .lines()
            .find(|line| line.contains("removed a stale temporary") && line.contains(&mine))
            .unwrap_or_else(|| panic!("no warning for {mine}:\n{logs}"));
        assert!(
            warning.contains("WARN") && warning.contains(".old.json.7.twaco-tmp"),
            "{logs}"
        );
        assert!(
            line("workspace lock refused").unwrap().contains("pid"),
            "{logs}"
        );
        assert!(line("workspace lock released").is_some(), "{logs}");
    }

    #[test]
    fn the_record_names_the_current_holder_while_someone_reads_it() {
        let (_dir, root) = temp();
        drop(acquire(&root, "sync", &[]).unwrap());
        // A reader that shares reading and writing only, as most programs open a file. On
        // Windows that refuses the rename, so the fallback must still replace the record.
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0x1 | 0x2); // FILE_SHARE_READ | FILE_SHARE_WRITE
        }
        let reader = options.open(root.join(HOLDER_PATH)).unwrap();
        let lock = acquire(&root, "fmt", &[]).unwrap();
        let text = std::fs::read_to_string(root.join(HOLDER_PATH)).unwrap();
        assert!(
            text.starts_with("twaco fmt,") && text.ends_with('\n'),
            "{text}"
        );
        assert!(!root.join(".twaco/.lock.holder.twaco-tmp").exists());
        drop((reader, lock));
    }

    #[test]
    fn a_lock_held_for_an_instant_is_waited_out() {
        // As `doctor` holds it while asking whether it is free.
        let (_dir, root) = temp();
        drop(acquire(&root, "sync", &[]).unwrap());
        let glance = OpenOptions::new()
            .write(true)
            .open(root.join(RELATIVE_PATH))
            .unwrap();
        glance.try_lock().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(60));
            drop(glance);
        });
        let lock = acquire(&root, "deploy", &[]).expect("the glance ends before the retries do");
        release.join().unwrap();
        drop(lock);
    }

    #[test]
    fn the_holder_can_be_asked_without_taking_the_lock() {
        let (_dir, root) = temp();
        assert_eq!(holder(&root).unwrap(), None, "no lock file yet");
        let held = acquire(&root, "deploy", &[]).unwrap();
        assert!(holder(&root).unwrap().unwrap().contains("twaco deploy"));
        drop(held);
        assert_eq!(holder(&root).unwrap(), None);
        // Asking did not take it: a writer can still acquire.
        drop(acquire(&root, "sync", &[]).unwrap());
    }

    #[test]
    fn a_held_lock_does_not_sweep() {
        let (_dir, root) = temp();
        let things = root.join("Things");
        std::fs::create_dir_all(&things).unwrap();
        let first = acquire(&root, "deploy", &[]).unwrap();
        // The holder is mid-write: its temporary must survive a refused second command.
        let in_flight = things.join(".T.xml.twaco-tmp");
        std::fs::write(&in_flight, b"x").unwrap();
        assert!(acquire(&root, "sync", &[things]).is_err());
        assert!(in_flight.exists());
        drop(first);
    }
}
