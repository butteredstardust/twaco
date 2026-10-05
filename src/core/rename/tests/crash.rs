//! A prefix rename (edits, moved files and folders, the baseline and the ledger) written by a
//! process that is killed at every point of the journaled write, then recovered by the next
//! command to take the lock.

use super::*;

const ROOT_VARIABLE: &str = "TWACO_TEST_CHILD_ROOT";

fn the_rename(solution: &Solution) -> Plan {
    plan(solution, &spec(Kind::Prefix, "Acme.App", "Acme.New")).unwrap()
}

/// Runs in the child. Does nothing when the test binary runs it as an ordinary test.
#[test]
fn child_renames_the_prefix() {
    let Ok(root) = std::env::var(ROOT_VARIABLE) else {
        return;
    };
    let solution = Solution::load(&Path::new(&root).join(CONFIG_FILE)).unwrap();
    let planned = the_rename(&solution);
    let lock = crate::core::lock::acquire(&solution.root, "child", &[]).unwrap();
    let _ = apply(&solution, &planned, &options(true), &lock);
}

fn seeded() -> Fixture {
    let fixture = fixture();
    seed_baseline(&fixture, &the_rename(&fixture.solution));
    fixture
}

#[test]
fn a_rename_is_wholly_done_or_not_at_all_whenever_the_process_dies() {
    let before = snapshot(&seeded().root);
    let (after, steps) = {
        let done = seeded();
        let mut steps = 0usize;
        apply_with(
            &done.solution,
            &the_rename(&done.solution),
            &options(true),
            &locked(&done),
            &mut |_| {
                steps += 1;
                Ok(())
            },
        )
        .unwrap();
        (snapshot(&done.root), steps)
    };
    assert_ne!(before, after);
    assert!(steps > 3, "{steps}");
    let mut points: Vec<(String, bool)> = vec![
        ("after-journal".to_string(), false),
        ("after-stage".to_string(), false),
        ("after-applying".to_string(), false),
    ];
    for step in 1..=steps {
        points.push((format!("after-step-{step}-visible"), true));
        points.push((format!("after-step-{step}-marked"), true));
    }
    points.push(("after-commit".to_string(), true));
    for (point, finished) in points {
        let fixture = seeded();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "core::rename::tests::crash::child_renames_the_prefix",
                "--nocapture",
            ])
            .env(ROOT_VARIABLE, &fixture.root)
            .env("TWACO_TEST_FAILPOINT", &point)
            .output()
            .unwrap()
            .status;
        assert!(!status.success(), "{point}: the child was not stopped");
        drop(locked(&fixture));
        let now = snapshot(&fixture.root);
        let hidden: Vec<&PathBuf> = now
            .keys()
            .filter(|path| {
                let name = path.file_name().unwrap().to_string_lossy();
                name.ends_with(".twaco-stage")
                    || name.ends_with(".twaco-backup")
                    || path.starts_with(".twaco/transactions")
            })
            .collect();
        assert!(hidden.is_empty(), "{point}: {hidden:?}");
        let expected = if finished { &after } else { &before };
        assert_eq!(
            now.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "{point}"
        );
        for (path, bytes) in expected {
            assert!(&now[path] == bytes, "{point}: {} differs", path.display());
        }
    }
}
