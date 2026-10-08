//! A transaction killed at every point of its protocol, recovered by the next process.
//!
//! The parent test starts the test binary itself as a child, with `TWACO_TEST_FAILPOINT` naming
//! where to abort. The child dies there for real: the operating system releases its lock, nothing
//! is flushed that was not synced, and the parent then takes the lock the way any later command
//! would and finds out what is left.

use super::*;
use std::process::Command;

const ROOT_VARIABLE: &str = "TWACO_TEST_CHILD_ROOT";

/// The same plan the other tests use, with a fourth step, so there are several points to die at.
fn plan_of<'a>(root: &'a Path) -> Transaction<'a> {
    let mut transaction = plan(root);
    transaction
        .create_file(&root.join("d.txt"), b"d after".to_vec())
        .unwrap();
    transaction
}

/// Runs in the child. Does nothing when the test binary runs it as an ordinary test.
#[test]
fn child_applies_the_plan() {
    let Ok(root) = std::env::var(ROOT_VARIABLE) else {
        return;
    };
    let root = PathBuf::from(root);
    let lock = lock::acquire(&root, "child", &[]).unwrap();
    let _ = plan_of(&root).apply(&lock);
}

fn die_at(root: &Path, point: &str) {
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "core::transaction::tests::failpoints::child_applies_the_plan",
            "--nocapture",
        ])
        .env(ROOT_VARIABLE, root)
        .env("TWACO_TEST_FAILPOINT", point)
        .output()
        .unwrap()
        .status;
    assert!(!status.success(), "{point}: the child was not stopped");
}

fn assert_whole(root: &Path, point: &str, expect_after: bool) {
    let lock = lock::acquire(root, "recovering command", &[]).unwrap();
    assert_eq!(lock.recovery.len(), 1, "{point}: {:?}", lock.recovery);
    if expect_after {
        assert_after(root);
        assert_eq!(read(root, "d.txt").as_deref(), Some("d after"), "{point}");
    } else {
        assert_before(root);
        assert_eq!(read(root, "d.txt"), None, "{point}");
    }
    assert_eq!(leftovers(root), Vec::<String>::new(), "{point}");
    drop(lock);
}

#[test]
fn every_point_of_the_protocol_recovers_to_wholly_before_or_wholly_after() {
    let mut points: Vec<(String, bool)> = vec![
        ("after-journal".to_string(), false),
        ("after-stage".to_string(), false),
        ("after-applying".to_string(), false),
    ];
    for step in 1..=4 {
        points.push((format!("after-step-{step}-visible"), true));
        points.push((format!("after-step-{step}-marked"), true));
    }
    points.push(("after-commit".to_string(), true));
    for (point, expect_after) in points {
        let (_dir, root) = workspace("failpoint");
        die_at(&root, &point);
        assert_whole(&root, &point, expect_after);
    }
}

#[test]
fn a_person_s_edit_after_the_crash_stops_recovery_and_is_left_untouched() {
    let (_dir, root) = workspace("failpoint-edit");
    die_at(&root, "after-step-1-visible");
    put(
        &root,
        "a.txt",
        "edited between the crash and the next command",
    );
    let error = lock::acquire(&root, "next", &[]).unwrap_err();
    assert!(matches!(error, lock::LockError::Recovery { .. }), "{error}");
    assert!(
        error.to_string().contains("a.txt: found sha256:"),
        "{error}"
    );
    assert_eq!(
        read(&root, "a.txt").as_deref(),
        Some("edited between the crash and the next command")
    );
    assert_eq!(read(&root, "b.txt").as_deref(), Some("b before"));
    assert_eq!(read(&root, "new/dir/c.txt"), None);
}

/// Runs in the child. Does nothing when the test binary runs it as an ordinary test.
#[test]
fn child_applies_the_folder_plan() {
    let Ok(root) = std::env::var(ROOT_VARIABLE) else {
        return;
    };
    let root = PathBuf::from(root);
    let lock = lock::acquire(&root, "child", &[]).unwrap();
    let _ = folder_plan(&root).apply(&lock);
}

#[test]
fn a_folder_move_with_edits_inside_it_recovers_whenever_the_process_dies() {
    let mut points: Vec<(String, bool)> = vec![
        ("after-journal".to_string(), false),
        ("after-stage".to_string(), false),
        ("after-applying".to_string(), false),
    ];
    for step in 1..=3 {
        points.push((format!("after-step-{step}-visible"), true));
        points.push((format!("after-step-{step}-marked"), true));
    }
    points.push(("after-commit".to_string(), true));
    for (point, expect_after) in points {
        let (_dir, root) = folder_workspace("failpoint-folder");
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "core::transaction::tests::failpoints::child_applies_the_folder_plan",
                "--nocapture",
            ])
            .env(ROOT_VARIABLE, &root)
            .env("TWACO_TEST_FAILPOINT", &point)
            .output()
            .unwrap()
            .status;
        assert!(!status.success(), "{point}: the child was not stopped");
        let lock = lock::acquire(&root, "recovering command", &[]).unwrap();
        assert_eq!(lock.recovery.len(), 1, "{point}: {:?}", lock.recovery);
        drop(lock);
        if expect_after {
            assert_folder_after(&root);
        } else {
            assert_folder_before(&root);
        }
        assert_eq!(leftovers(&root), Vec::<String>::new(), "{point}");
    }
}
