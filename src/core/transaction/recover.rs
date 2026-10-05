//! Finishing or undoing an operation that a crash interrupted.
//!
//! Where an operation stands is read from the files themselves: each destination holds its
//! before state, its after state, or something else, and the journal's progress marks are only a
//! hint. A rename that completed just before the process died but before the mark was written is
//! therefore seen for what it is.

use super::journal::{self, Journal, Kind, ReadError, State, Step};
use super::paths;
use std::fmt;
use std::path::Path;

/// Why a journal was not recovered. The text is the repair plan: it names every path involved.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal(pub String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing was visible, or the operation had committed: only artifacts were removed.
    Cleaned,
    RolledForward,
    RolledBack,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovered {
    pub operation_id: String,
    pub command: String,
    pub action: Action,
}

impl Recovered {
    /// One line for a person: what was found and what was done.
    pub fn describe(&self) -> String {
        let what = match self.action {
            Action::Cleaned => "cleaned up after",
            Action::RolledForward => "finished",
            Action::RolledBack => "undid",
        };
        format!(
            "{what} an interrupted {} (operation {})",
            self.command, self.operation_id
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefer {
    /// A crash: complete the operation when the bytes to install are still staged.
    Forward,
    /// A step failed: undo what was installed.
    Backward,
}

enum Standing {
    Before,
    After,
    Other(Option<String>),
}

/// The digest of what is at `path`, or none when nothing is. Anything that is not a regular
/// file reads as a digest no operation could have produced.
pub(super) fn observe(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(why) => Err(why),
        Ok(metadata) if !metadata.is_file() => Ok(Some("not-a-regular-file".to_string())),
        Ok(_) => Ok(Some(journal::digest(&std::fs::read(path)?))),
    }
}

fn standing(root: &Path, step: &Step) -> std::io::Result<Standing> {
    let found = observe(&paths::absolute(root, &step.path))?;
    Ok(if found == step.before {
        Standing::Before
    } else if found == step.after {
        Standing::After
    } else {
        Standing::Other(found)
    })
}

fn show(digest: &Option<String>) -> String {
    digest.clone().unwrap_or_else(|| "absent".to_string())
}

/// Recover whatever a crash left, before anything else touches the workspace. Called with the
/// workspace lock held, ahead of the sweep for stale temporaries.
pub fn recover_pending(root: &Path) -> Result<Vec<Recovered>, Refusal> {
    let folder = root.join(journal::DIRECTORY);
    if paths::is_link(&root.join(".twaco")) || paths::is_link(&folder) {
        return Err(Refusal(format!(
            "recovery refused: {} is a link, so what is behind it is not this workspace's",
            folder.display()
        )));
    }
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return Ok(Vec::new());
    };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && path
                    .file_name()
                    .is_some_and(|name| !name.to_string_lossy().starts_with('.'))
        })
        .collect();
    files.sort();
    let mut journals = Vec::new();
    for file in files {
        match journal::read(&file) {
            Ok(journal) => journals.push(journal),
            Err(ReadError::Unsupported { major, minor }) => {
                return Err(Refusal(format!(
                    "recovery refused: {} was written by a newer twaco (journal format \
                     {major}.{minor}; this one reads {}.{}). Run the newer twaco in this \
                     workspace to finish it.",
                    file.display(),
                    journal::MAJOR,
                    journal::MINOR
                )))
            }
            Err(ReadError::Malformed(why)) => {
                return Err(Refusal(format!(
                    "recovery refused: {} cannot be read ({why}). Nothing was changed; its \
                     staged files sit beside the files it names. Do not delete it unread.",
                    file.display()
                )))
            }
            Err(ReadError::Io(why)) => {
                return Err(Refusal(format!(
                    "recovery refused: {}: {why}",
                    file.display()
                )))
            }
        }
    }
    let unfinished: Vec<&Journal> = journals
        .iter()
        .filter(|journal| journal.state == State::Applying)
        .collect();
    if unfinished.len() > 1 {
        let names: Vec<String> = unfinished
            .iter()
            .map(|journal| {
                format!(
                    "  {}",
                    journal::path_of(root, &journal.operation_id).display()
                )
            })
            .collect();
        return Err(Refusal(format!(
            "recovery refused: {} operations were interrupted and no order between them is \
             safe to guess:\n{}\nA person must decide which to finish; leave the files as they \
             are.",
            unfinished.len(),
            names.join("\n")
        )));
    }
    let mut recovered = Vec::new();
    for journal in &journals {
        let action = resolve(root, journal, Prefer::Forward)?;
        recovered.push(Recovered {
            operation_id: journal.operation_id.clone(),
            command: journal.command.clone(),
            action,
        });
    }
    Ok(recovered)
}

/// Bring one journal's operation to an end: committed, or undone.
pub(super) fn resolve(root: &Path, journal: &Journal, prefer: Prefer) -> Result<Action, Refusal> {
    match journal.state {
        State::Committed => {
            clean(root, journal, false);
            return Ok(Action::Cleaned);
        }
        State::Staging => {
            clean(root, journal, true);
            return Ok(Action::Cleaned);
        }
        State::Applying => {}
    }
    check_paths(root, journal)?;
    let io = |why: std::io::Error| {
        Refusal(format!(
            "recovery refused for operation {}: {why}",
            journal.operation_id
        ))
    };
    let mut standings = Vec::new();
    for step in &journal.steps {
        standings.push(standing(root, step).map_err(io)?);
    }
    let conflicts: Vec<String> = journal
        .steps
        .iter()
        .zip(&standings)
        .filter_map(|(step, standing)| match standing {
            Standing::Other(found) => Some(format!(
                "  {}: found {}; expected {} before the operation or {} after it",
                step.path,
                show(found),
                show(&step.before),
                show(&step.after)
            )),
            _ => None,
        })
        .collect();
    if !conflicts.is_empty() {
        return Err(Refusal(refusal_text(journal, &conflicts)));
    }
    let installed: Vec<&Step> = journal
        .steps
        .iter()
        .zip(&standings)
        .filter(|(_, standing)| matches!(standing, Standing::After))
        .map(|(step, _)| step)
        .collect();
    let waiting: Vec<&Step> = journal
        .steps
        .iter()
        .zip(&standings)
        .filter(|(_, standing)| matches!(standing, Standing::Before))
        .map(|(step, _)| step)
        .collect();
    if installed.is_empty() {
        clean(root, journal, true);
        return Ok(Action::Cleaned);
    }
    let forward = waiting.iter().all(|step| can_install(root, step));
    let backward = installed.iter().all(|step| can_restore(root, step));
    let go_forward = match prefer {
        Prefer::Forward => forward,
        Prefer::Backward => false,
    };
    if go_forward {
        for step in waiting {
            install(root, step).map_err(|why| {
                Refusal(format!(
                    "recovery stopped for operation {}: {why}\n{}",
                    journal.operation_id,
                    refusal_text(journal, &[])
                ))
            })?;
        }
        let mut done = journal.clone();
        done.state = State::Committed;
        journal::write(root, &done).map_err(io)?;
        clean(root, &done, false);
        return Ok(Action::RolledForward);
    }
    if !backward {
        return Err(Refusal(refusal_text(
            journal,
            &[
                "  the bytes needed to finish and the copies needed to undo are not both \
                 available"
                    .to_string(),
            ],
        )));
    }
    for step in installed.iter().rev() {
        restore(root, step).map_err(|why| {
            Refusal(format!(
                "recovery stopped for operation {}: {why}\n{}",
                journal.operation_id,
                refusal_text(journal, &[])
            ))
        })?;
    }
    clean(root, journal, true);
    Ok(Action::RolledBack)
}

fn check_paths(root: &Path, journal: &Journal) -> Result<(), Refusal> {
    let mut problems = Vec::new();
    for step in &journal.steps {
        let named = std::iter::once(&step.path)
            .chain(step.stage.iter())
            .chain(step.backup.iter())
            .chain(step.new_dirs.iter());
        for path in named {
            if let Err(why) = paths::stored(path).and_then(|path| paths::reject_links(root, &path))
            {
                problems.push(format!("  {why}"));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Refusal(format!(
            "recovery refused for operation {}: it names a path it may not touch:\n{}\n\
             Nothing was opened, replaced or removed.",
            journal.operation_id,
            problems.join("\n")
        )))
    }
}

fn refusal_text(journal: &Journal, conflicts: &[String]) -> String {
    let mut text = format!(
        "recovery refused for operation {} ({}):\n",
        journal.operation_id, journal.command
    );
    for conflict in conflicts {
        text.push_str(conflict);
        text.push('\n');
    }
    text.push_str(
        "\nDo not run the command again. Keep the files as they are, merge or restore them \
         deliberately, then make every file the journal names match either its state before the \
         operation (to undo it) or after it (to finish it). The copies that remain:\n",
    );
    for step in &journal.steps {
        if let Some(backup) = &step.backup {
            text.push_str(&format!("  original of {}: {backup}\n", step.path));
        }
        if let Some(stage) = &step.stage {
            text.push_str(&format!("  new bytes of {}: {stage}\n", step.path));
        }
    }
    text.push_str("After that, run twaco again; it finishes the recovery.");
    text
}

/// The staged bytes of a step that has not been installed are present and are the planned ones.
fn can_install(root: &Path, step: &Step) -> bool {
    match step.kind {
        Kind::Delete => true,
        Kind::Replace | Kind::Create => holds(root, step.stage.as_deref(), &step.after),
    }
}

/// What it takes to put an installed step back is present.
fn can_restore(root: &Path, step: &Step) -> bool {
    match step.kind {
        Kind::Create => true,
        Kind::Replace | Kind::Delete => holds(root, step.backup.as_deref(), &step.before),
    }
}

fn holds(root: &Path, artifact: Option<&str>, digest: &Option<String>) -> bool {
    artifact.is_some_and(|artifact| {
        std::fs::read(paths::absolute(root, artifact))
            .is_ok_and(|bytes| Some(journal::digest(&bytes)) == *digest)
    })
}

/// Make one step visible: rename its staged bytes over the destination, or remove the
/// destination.
pub(super) fn install(root: &Path, step: &Step) -> Result<(), String> {
    let destination = paths::absolute(root, &step.path);
    let fail = |why: std::io::Error| format!("{}: {why}", step.path);
    match step.kind {
        Kind::Delete => retry(|| std::fs::remove_file(&destination)).map_err(fail),
        Kind::Replace | Kind::Create => {
            let stage = step
                .stage
                .as_deref()
                .ok_or_else(|| format!("{} has no staged bytes", step.path))?;
            if step.kind == Kind::Create && destination.exists() {
                return Err(format!("{} exists already", step.path));
            }
            let stage = paths::absolute(root, stage);
            retry(|| std::fs::rename(&stage, &destination)).map_err(fail)
        }
    }
}

/// Put one installed step back from its backup, or remove what it created.
fn restore(root: &Path, step: &Step) -> Result<(), String> {
    let destination = paths::absolute(root, &step.path);
    let fail = |why: std::io::Error| format!("{}: {why}", step.path);
    match step.kind {
        Kind::Create => retry(|| std::fs::remove_file(&destination)).map_err(fail),
        Kind::Replace | Kind::Delete => {
            let backup = step
                .backup
                .as_deref()
                .ok_or_else(|| format!("{} has no backup", step.path))?;
            let bytes = std::fs::read(paths::absolute(root, backup)).map_err(fail)?;
            retry(|| crate::core::workspace::atomic_replace(&destination, &bytes)).map_err(fail)
        }
    }
}

/// An editor or a virus scanner can hold a file for a moment without sharing it, which Windows
/// reports as a permission error. A few short retries; anything else fails at once.
fn retry(mut action: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut wait = 25;
    for attempt in 0..6 {
        match action() {
            Err(why) if why.kind() == std::io::ErrorKind::PermissionDenied && attempt < 5 => {
                std::thread::sleep(std::time::Duration::from_millis(wait));
                wait *= 2;
            }
            other => return other,
        }
    }
    unreachable!("the last attempt returns")
}

/// Remove an operation's stages, backups and journal. `undone` also removes the folders it
/// created, when nothing else has been put in them.
pub(super) fn clean(root: &Path, journal: &Journal, undone: bool) {
    for step in &journal.steps {
        for artifact in step.stage.iter().chain(step.backup.iter()) {
            if paths::stored(artifact)
                .is_ok_and(|artifact| paths::reject_links(root, &artifact).is_ok())
            {
                let _ = std::fs::remove_file(paths::absolute(root, artifact));
            }
        }
    }
    if undone {
        for step in journal.steps.iter().rev() {
            for folder in step.new_dirs.iter().rev() {
                if paths::stored(folder)
                    .is_ok_and(|folder| paths::reject_links(root, &folder).is_ok())
                {
                    let _ = std::fs::remove_dir(paths::absolute(root, folder));
                }
            }
        }
    }
    let _ = std::fs::remove_file(journal::path_of(root, &journal.operation_id));
    journal::sync_directory(&root.join(journal::DIRECTORY));
}
