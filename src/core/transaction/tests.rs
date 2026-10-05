use super::journal::{self, Journal, State};
use super::recover::{self, Action};
use super::*;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::lock;

#[cfg(feature = "test-failpoints")]
mod failpoints;

fn scratch(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "twaco-transaction-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn put(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(root: &Path, relative: &str) -> Option<String> {
    std::fs::read_to_string(root.join(relative)).ok()
}

/// Every file under `root` that belongs to a transaction rather than to the workspace.
fn leftovers(root: &Path) -> Vec<String> {
    fn walk(folder: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                let in_journals = path
                    .parent()
                    .is_some_and(|parent| parent.ends_with("transactions"));
                if name.ends_with(".twaco-stage")
                    || name.ends_with(".twaco-backup")
                    || name.ends_with(".twaco-tmp")
                    || in_journals
                {
                    out.push(path.display().to_string());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// A workspace with `a.txt` and `b.txt`, and a plan that replaces `a.txt`, deletes `b.txt` and
/// creates `new/dir/c.txt`.
fn workspace(label: &str) -> PathBuf {
    let root = scratch(label);
    put(&root, "a.txt", "a before");
    put(&root, "b.txt", "b before");
    root
}

fn plan<'a>(root: &'a Path) -> Transaction<'a> {
    let mut transaction = Transaction::new(root, "test operation");
    transaction
        .replace_file(&root.join("a.txt"), b"a before", b"a after".to_vec())
        .unwrap();
    transaction
        .delete_file(&root.join("b.txt"), b"b before")
        .unwrap();
    transaction
        .create_file(&root.join("new/dir/c.txt"), b"c after".to_vec())
        .unwrap();
    transaction
}

fn assert_before(root: &Path) {
    assert_eq!(read(root, "a.txt").as_deref(), Some("a before"));
    assert_eq!(read(root, "b.txt").as_deref(), Some("b before"));
    assert!(!root.join("new").exists(), "the folders it made are gone");
}

fn assert_after(root: &Path) {
    assert_eq!(read(root, "a.txt").as_deref(), Some("a after"));
    assert_eq!(read(root, "b.txt"), None);
    assert_eq!(read(root, "new/dir/c.txt").as_deref(), Some("c after"));
}

fn journal_in(root: &Path) -> Journal {
    let mut found: Vec<_> = std::fs::read_dir(root.join(journal::DIRECTORY))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    journal::read(&found.remove(0)).unwrap()
}

#[test]
fn an_applied_transaction_changes_every_file_and_leaves_no_trace() {
    let root = workspace("applied");
    let lock = lock::acquire(&root, "test", &[]).unwrap();
    let committed = plan(&root).apply(&lock).unwrap();
    assert_eq!(committed.steps, 3);
    assert_after(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    drop(lock);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_identical_replacement_is_not_a_step() {
    let root = workspace("identical");
    let mut transaction = Transaction::new(&root, "test");
    transaction
        .replace_file(&root.join("a.txt"), b"a before", b"a before".to_vec())
        .unwrap();
    assert!(transaction.is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_file_that_changed_since_it_was_read_changes_nothing() {
    let root = workspace("stale");
    put(&root, "a.txt", "someone edited this");
    let lock = lock::acquire(&root, "test", &[]).unwrap();
    let error = plan(&root).apply(&lock).unwrap_err();
    assert!(matches!(error, TransactionError::Stale(_)), "{error}");
    assert_eq!(error.code(), ErrorCode::StalePlan);
    assert_eq!(read(&root, "a.txt").as_deref(), Some("someone edited this"));
    assert_eq!(read(&root, "b.txt").as_deref(), Some("b before"));
    assert!(!root.join("new").exists());
    assert_eq!(leftovers(&root), Vec::<String>::new());
    drop(lock);
    put(&root, "a.txt", "a before");
    put(&root, "new/dir/c.txt", "already here");
    let lock = lock::acquire(&root, "test", &[]).unwrap();
    let error = plan(&root).apply(&lock).unwrap_err();
    assert!(error.to_string().contains("exists already"), "{error}");
    assert_eq!(read(&root, "a.txt").as_deref(), Some("a before"));
    drop(lock);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_path_that_could_leave_the_workspace_is_refused_when_it_is_named() {
    let root = workspace("paths");
    let mut transaction = Transaction::new(&root, "test");
    for bad in ["../outside.txt", "sub/../../outside.txt", "/etc/passwd", ""] {
        let path = if bad.starts_with('/') {
            std::env::temp_dir().join("elsewhere.txt")
        } else {
            PathBuf::from(bad)
        };
        let error = transaction.create_file(&path, b"x".to_vec()).unwrap_err();
        assert!(
            matches!(error, TransactionError::Invalid(_)),
            "{bad}: {error}"
        );
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
    }
    transaction
        .create_file(&root.join("once.txt"), b"x".to_vec())
        .unwrap();
    let error = transaction
        .create_file(&root.join("once.txt"), b"y".to_vec())
        .unwrap_err();
    assert!(error.to_string().contains("named twice"), "{error}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_link_on_the_way_is_refused() {
    let root = workspace("links");
    let outside = scratch("links-outside");
    std::fs::create_dir_all(root.join("real")).unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, root.join("link")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&outside, root.join("link")).is_ok()
        // A junction needs no privilege, and is a reparse point that is not a symbolic link.
        || std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.join("link"))
            .arg(&outside)
            .output()
            .is_ok_and(|output| output.status.success());
    if !linked {
        eprintln!("skipped: this account may not create links");
        return;
    }
    let mut transaction = Transaction::new(&root, "test");
    let error = transaction
        .create_file(&root.join("link/escape.txt"), b"x".to_vec())
        .unwrap_err();
    assert!(error.to_string().contains("link"), "{error}");
    assert!(!outside.join("escape.txt").exists());
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn a_crash_before_anything_is_visible_leaves_the_workspace_as_it_was() {
    let root = workspace("before-visible");
    let transaction = plan(&root);
    let journal = transaction.stage().unwrap();
    assert_eq!(journal.state, State::Applying);
    assert!(!leftovers(&root).is_empty(), "stages and backups exist");
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].action, Action::Cleaned);
    assert_before(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_crash_while_staging_leaves_the_workspace_as_it_was() {
    let root = workspace("staging");
    let journal = plan(&root).stage().unwrap();
    let mut staging = journal.clone();
    staging.state = State::Staging;
    journal::write(&root, &staging).unwrap();
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::Cleaned);
    assert_before(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_step_installed_just_before_its_mark_is_seen_and_the_operation_finished() {
    let root = workspace("unmarked");
    let journal = plan(&root).stage().unwrap();
    // The first step became visible, then the process died before it wrote the mark: the staged
    // bytes were consumed by the rename.
    recover::install(&root, &journal.steps[0]).unwrap();
    assert!(!journal.steps[0].completed);
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::RolledForward);
    assert_after(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_crash_midway_through_several_steps_is_finished() {
    let root = workspace("midway");
    let mut journal = plan(&root).stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    journal.steps[0].completed = true;
    recover::install(&root, &journal.steps[1]).unwrap();
    journal::write(&root, &journal).unwrap();
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::RolledForward);
    assert_after(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn when_the_new_bytes_are_gone_the_installed_steps_are_undone_from_their_backups() {
    let root = workspace("backward");
    let journal = plan(&root).stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    // The create step's staged bytes have vanished, so the operation cannot be finished.
    std::fs::remove_file(root.join(journal.steps[2].stage.as_ref().unwrap())).unwrap();
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::RolledBack);
    assert_before(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn with_neither_the_new_bytes_nor_a_backup_recovery_refuses_and_keeps_the_journal() {
    let root = workspace("neither");
    let journal = plan(&root).stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    std::fs::remove_file(root.join(journal.steps[2].stage.as_ref().unwrap())).unwrap();
    std::fs::remove_file(root.join(journal.steps[0].backup.as_ref().unwrap())).unwrap();
    let refusal = recover::recover_pending(&root).unwrap_err();
    assert!(refusal.0.contains("recovery refused"), "{refusal}");
    assert_eq!(read(&root, "a.txt").as_deref(), Some("a after"));
    assert_eq!(read(&root, "b.txt").as_deref(), Some("b before"));
    assert_eq!(journal_in(&root).operation_id, journal.operation_id);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_edit_made_after_the_crash_is_never_overwritten_and_every_conflict_is_named() {
    let root = workspace("edited");
    let journal = plan(&root).stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    put(&root, "a.txt", "a edited by a person");
    put(&root, "b.txt", "b edited by a person");
    let refusal = recover::recover_pending(&root).unwrap_err();
    assert!(refusal.0.contains("a.txt: found sha256:"), "{refusal}");
    assert!(refusal.0.contains("b.txt: found sha256:"), "{refusal}");
    assert!(
        refusal.0.contains("Do not run the command again"),
        "{refusal}"
    );
    assert!(refusal.0.contains("original of a.txt"), "{refusal}");
    assert_eq!(
        read(&root, "a.txt").as_deref(),
        Some("a edited by a person")
    );
    assert_eq!(
        read(&root, "b.txt").as_deref(),
        Some("b edited by a person")
    );
    assert!(!root.join("new/dir/c.txt").exists());
    assert_eq!(journal_in(&root).operation_id, journal.operation_id);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_committed_journal_is_only_cleaned_and_a_later_edit_is_left_alone() {
    let root = workspace("committed");
    let mut journal = plan(&root).stage().unwrap();
    for step in journal.steps.clone() {
        recover::install(&root, &step).unwrap();
    }
    journal.state = State::Committed;
    journal::write(&root, &journal).unwrap();
    put(&root, "a.txt", "edited after the commit");
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::Cleaned);
    assert_eq!(
        read(&root, "a.txt").as_deref(),
        Some("edited after the commit")
    );
    assert_eq!(read(&root, "new/dir/c.txt").as_deref(), Some("c after"));
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_journal_from_a_newer_twaco_or_a_damaged_one_is_refused() {
    let root = workspace("versions");
    let journal = plan(&root).stage().unwrap();
    let path = journal::path_of(&root, &journal.operation_id);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["format"]["major"] = 2.into();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let refusal = recover::recover_pending(&root).unwrap_err();
    assert!(refusal.0.contains("newer twaco"), "{refusal}");
    std::fs::write(&path, b"{ not json").unwrap();
    let refusal = recover::recover_pending(&root).unwrap_err();
    assert!(refusal.0.contains("cannot be read"), "{refusal}");
    assert_eq!(read(&root, "a.txt").as_deref(), Some("a before"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn two_interrupted_operations_are_refused_not_ordered() {
    let root = workspace("two");
    let first = plan(&root).stage().unwrap();
    let mut second = first.clone();
    second.operation_id = format!("{}-other", first.operation_id);
    journal::write(&root, &second).unwrap();
    let refusal = recover::recover_pending(&root).unwrap_err();
    assert!(
        refusal.0.contains("2 operations were interrupted"),
        "{refusal}"
    );
    assert!(refusal.0.contains(&first.operation_id), "{refusal}");
    assert!(refusal.0.contains(&second.operation_id), "{refusal}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_journal_that_names_a_path_outside_the_workspace_is_never_acted_on() {
    let root = workspace("escape");
    let outside = scratch("escape-outside");
    put(&outside, "victim.txt", "keep me");
    let mut journal = plan(&root).stage().unwrap();
    journal.steps[1].path = "../victim.txt".to_string();
    journal::write(&root, &journal).unwrap();
    let refusal = recover::recover_pending(&root).unwrap_err();
    assert!(refusal.0.contains("may not touch"), "{refusal}");
    assert_eq!(read(&outside, "victim.txt").as_deref(), Some("keep me"));
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn a_failed_step_undoes_the_ones_before_it() {
    let root = workspace("undo");
    let transaction = plan(&root);
    let mut journal = transaction.stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    journal.steps[0].completed = true;
    journal::write(&root, &journal).unwrap();
    let error = transaction.undo(
        journal,
        recover::Stop::Failed("the disk said no".to_string()),
    );
    assert!(
        matches!(
            error,
            TransactionError::Failed {
                rolled_back: true,
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(error.code(), ErrorCode::IoError);
    assert!(error.to_string().contains("every change was undone"));
    assert_before(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn taking_the_lock_finishes_an_interrupted_operation_and_says_so() {
    let root = workspace("lock-recovers");
    let journal = plan(&root).stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    let lock = lock::acquire(&root, "next command", &[]).unwrap();
    assert_eq!(lock.recovery.len(), 1);
    assert!(
        lock.recovery[0].contains("finished an interrupted test operation"),
        "{:?}",
        lock.recovery
    );
    assert_after(&root);
    drop(lock);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn taking_the_lock_is_refused_while_an_edit_stands_in_the_way() {
    let root = workspace("lock-refuses");
    let journal = plan(&root).stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    put(&root, "a.txt", "edited");
    let error = lock::acquire(&root, "next command", &[]).unwrap_err();
    assert!(matches!(error, lock::LockError::Recovery { .. }), "{error}");
    assert_eq!(error.code(), ErrorCode::RollbackFailed);
    assert!(error.to_string().contains("a.txt"), "{error}");
    assert_eq!(read(&root, "a.txt").as_deref(), Some("edited"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn files_created_in_the_same_new_folder_share_it() {
    let root = workspace("shared-folder");
    let lock = lock::acquire(&root, "test", &[]).unwrap();
    let mut transaction = Transaction::new(&root, "test");
    for name in ["x/y/one.txt", "x/y/two.txt", "x/z/three.txt"] {
        transaction
            .create_file(&root.join(name), name.as_bytes().to_vec())
            .unwrap();
    }
    transaction.apply(&lock).unwrap();
    for name in ["x/y/one.txt", "x/y/two.txt", "x/z/three.txt"] {
        assert_eq!(read(&root, name).as_deref(), Some(name));
    }
    drop(lock);
    // Undone, every folder it made goes and nothing else does.
    let mut transaction = Transaction::new(&root, "test");
    for name in ["p/q/one.txt", "p/q/two.txt"] {
        transaction
            .create_file(&root.join(name), name.as_bytes().to_vec())
            .unwrap();
    }
    let journal = transaction.stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    std::fs::remove_file(root.join(journal.steps[1].stage.as_ref().unwrap())).unwrap();
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::RolledBack);
    assert!(!root.join("p").exists());
    assert!(root.join("x/y/one.txt").exists());
    let _ = std::fs::remove_dir_all(root);
}

/// `dir/x.txt` and `dir/y.txt` in a workspace that also has `a.txt`; a plan that edits
/// `dir/x.txt`, replaces `a.txt` and renames `dir` to `moved/dir2`.
fn folder_workspace(label: &str) -> PathBuf {
    let root = scratch(label);
    put(&root, "dir/x.txt", "x before");
    put(&root, "dir/y.txt", "y before");
    put(&root, "a.txt", "a before");
    root
}

fn folder_plan<'a>(root: &'a Path) -> Transaction<'a> {
    let mut transaction = Transaction::new(root, "test move");
    transaction
        .replace_file(&root.join("dir/x.txt"), b"x before", b"x after".to_vec())
        .unwrap();
    transaction
        .replace_file(&root.join("a.txt"), b"a before", b"a after".to_vec())
        .unwrap();
    transaction
        .move_path(&root.join("dir"), &root.join("moved/dir2"))
        .unwrap();
    transaction
}

fn assert_folder_before(root: &Path) {
    assert_eq!(read(root, "dir/x.txt").as_deref(), Some("x before"));
    assert_eq!(read(root, "dir/y.txt").as_deref(), Some("y before"));
    assert_eq!(read(root, "a.txt").as_deref(), Some("a before"));
    assert!(!root.join("moved").exists());
}

fn assert_folder_after(root: &Path) {
    assert_eq!(read(root, "moved/dir2/x.txt").as_deref(), Some("x after"));
    assert_eq!(read(root, "moved/dir2/y.txt").as_deref(), Some("y before"));
    assert_eq!(read(root, "a.txt").as_deref(), Some("a after"));
    assert!(!root.join("dir").exists());
}

#[test]
fn a_folder_move_with_an_edit_inside_it_applies_and_leaves_no_artifact_behind() {
    let root = folder_workspace("move-applies");
    let lock = lock::acquire(&root, "test", &[]).unwrap();
    folder_plan(&root).apply(&lock).unwrap();
    assert_folder_after(&root);
    // The backup of the edited file travelled with the folder; it is removed all the same.
    assert_eq!(leftovers(&root), Vec::<String>::new());
    drop(lock);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_move_onto_something_that_exists_or_from_something_that_is_gone_changes_nothing() {
    let root = folder_workspace("move-refused");
    put(&root, "moved/dir2/in-the-way.txt", "here already");
    let lock = lock::acquire(&root, "test", &[]).unwrap();
    let error = folder_plan(&root).apply(&lock).unwrap_err();
    assert!(error.to_string().contains("exists already"), "{error}");
    assert_eq!(read(&root, "dir/x.txt").as_deref(), Some("x before"));
    assert_eq!(read(&root, "a.txt").as_deref(), Some("a before"));
    drop(lock);
    let _ = std::fs::remove_dir_all(&root);
    let root = folder_workspace("move-gone");
    let mut transaction = Transaction::new(&root, "test");
    transaction
        .move_path(&root.join("nothing"), &root.join("elsewhere"))
        .unwrap();
    let lock = lock::acquire(&root, "test", &[]).unwrap();
    assert!(matches!(
        transaction.apply(&lock),
        Err(TransactionError::Stale(_))
    ));
    drop(lock);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_crash_after_the_edits_but_before_the_move_finishes_the_move() {
    let root = folder_workspace("move-forward");
    let journal = folder_plan(&root).stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    recover::install(&root, &journal.steps[1]).unwrap();
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::RolledForward);
    assert_folder_after(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_crash_just_after_the_move_is_seen_by_where_the_edited_file_now_is() {
    let root = folder_workspace("move-after");
    let journal = folder_plan(&root).stage().unwrap();
    for step in &journal.steps {
        recover::install(&root, step).unwrap();
    }
    // Nothing was marked: the edited file is found at its new place, not its old one.
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::RolledForward);
    assert_folder_after(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_crash_is_undone_when_it_cannot_be_finished_and_the_move_goes_back() {
    let root = folder_workspace("move-backward");
    let journal = folder_plan(&root).stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    // The staged bytes of the other edit are gone, so the operation cannot be finished.
    std::fs::remove_file(root.join(journal.steps[1].stage.as_ref().unwrap())).unwrap();
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::RolledBack);
    assert_folder_before(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_failure_at_any_step_undoes_the_ones_before_it_and_the_hook_sees_every_step() {
    let steps = {
        let root = folder_workspace("hook-count");
        let lock = lock::acquire(&root, "test", &[]).unwrap();
        let mut seen = Vec::new();
        folder_plan(&root)
            .apply_with(&lock, &mut |at, step| {
                seen.push((at, step.kind));
                Ok(())
            })
            .unwrap();
        drop(lock);
        let _ = std::fs::remove_dir_all(root);
        seen
    };
    assert_eq!(
        steps,
        [(0, Kind::Replace), (1, Kind::Replace), (2, Kind::Move)]
    );
    for fail_at in 0..steps.len() {
        let root = folder_workspace("hook-fail");
        let lock = lock::acquire(&root, "test", &[]).unwrap();
        let error = folder_plan(&root)
            .apply_with(&lock, &mut |at, _| {
                if at == fail_at {
                    Err(std::io::Error::other("injected"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(
            matches!(
                error,
                TransactionError::Failed {
                    rolled_back: true,
                    ..
                }
            ),
            "{fail_at}: {error}"
        );
        assert_folder_before(&root);
        assert_eq!(leftovers(&root), Vec::<String>::new(), "{fail_at}");
        drop(lock);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn a_file_saved_just_before_its_step_is_refused_and_the_rest_is_undone() {
    let root = folder_workspace("saved-before");
    let lock = lock::acquire(&root, "test", &[]).unwrap();
    let error = folder_plan(&root)
        .apply_with(&lock, &mut |at, _| {
            if at == 1 {
                put(&root, "a.txt", "a saved by a person");
            }
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(error, TransactionError::Stale(_)), "{error}");
    assert_eq!(read(&root, "a.txt").as_deref(), Some("a saved by a person"));
    assert_eq!(read(&root, "dir/x.txt").as_deref(), Some("x before"));
    assert_eq!(leftovers(&root), Vec::<String>::new());
    drop(lock);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_save_made_after_the_operation_wrote_a_file_is_named_not_overwritten_by_the_undo() {
    let root = folder_workspace("saved-after");
    let lock = lock::acquire(&root, "test", &[]).unwrap();
    let error = folder_plan(&root)
        .apply_with(&lock, &mut |at, _| {
            if at == 2 {
                // Both edits are in; a person saves the edited file, and then the move fails.
                put(&root, "dir/x.txt", "saved by a person");
                return Err(std::io::Error::other("injected"));
            }
            Ok(())
        })
        .unwrap_err();
    match &error {
        TransactionError::Failed {
            rolled_back: false,
            leftover,
            journal: Some(journal),
            ..
        } => {
            assert_eq!(leftover, &vec![root.join("dir/x.txt")]);
            assert!(journal.exists());
        }
        other => panic!("{other}"),
    }
    assert_eq!(error.code(), ErrorCode::RollbackFailed);
    assert_eq!(
        read(&root, "dir/x.txt").as_deref(),
        Some("saved by a person")
    );
    // The other edit was put back.
    assert_eq!(read(&root, "a.txt").as_deref(), Some("a before"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_journal_still_marked_staging_is_judged_by_the_files_not_the_mark() {
    // After a power cut the older copy of the journal can be the one that survived, while a
    // destination's rename was kept: the files say the operation had begun.
    let root = workspace("stale-staging");
    let mut journal = plan(&root).stage().unwrap();
    recover::install(&root, &journal.steps[0]).unwrap();
    journal.state = State::Staging;
    journal::write(&root, &journal).unwrap();
    let recovered = recover::recover_pending(&root).unwrap();
    assert_eq!(recovered[0].action, Action::RolledForward);
    assert_after(&root);
    assert_eq!(leftovers(&root), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_operation_id_that_is_a_path_is_refused_and_nothing_outside_the_journals_is_removed() {
    let root = workspace("hostile-id");
    let victim = root.join("victim.txt");
    put(&root, "victim.txt", "keep me");
    let mut journal = plan(&root).stage().unwrap();
    journal.state = State::Committed;
    journal.operation_id = "../../victim".to_string();
    let file = root.join(journal::DIRECTORY).join("x.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, serde_json::to_vec(&journal).unwrap()).unwrap();
    // The real journal of `stage` is still there too; remove it so only the hostile one remains.
    for entry in std::fs::read_dir(root.join(journal::DIRECTORY))
        .unwrap()
        .flatten()
    {
        if entry.path() != file {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    let refusal = recover::recover_pending(&root).unwrap_err();
    assert!(refusal.0.contains("cannot be read"), "{refusal}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_lock_for_another_workspace_does_not_allow_a_transaction() {
    let here = workspace("lock-here");
    let there = workspace("lock-there");
    let lock = lock::acquire(&there, "test", &[]).unwrap();
    let error = plan(&here).apply(&lock).unwrap_err();
    assert!(matches!(error, TransactionError::Invalid(_)), "{error}");
    assert_before(&here);
    drop(lock);
    let lock = lock::acquire(&here, "test", &[]).unwrap();
    plan(&here).apply(&lock).unwrap();
    drop(lock);
    let _ = std::fs::remove_dir_all(here);
    let _ = std::fs::remove_dir_all(there);
}

#[cfg(windows)]
#[test]
fn a_name_windows_would_read_as_another_is_refused() {
    let root = workspace("aliases");
    let mut transaction = Transaction::new(&root, "test");
    for bad in ["a.", "a ", "a:stream", "sub/b."] {
        let error = transaction
            .create_file(&root.join(bad), b"x".to_vec())
            .unwrap_err();
        assert!(
            matches!(error, TransactionError::Invalid(_)),
            "{bad}: {error}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}
