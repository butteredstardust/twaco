use super::*;
use std::path::Path;
use std::process::Command;

fn xml(value: &str, script: &str) -> String {
    format!("<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><value>{value}</value><ThingShape><ServiceImplementations><ServiceImplementation name=\"S\"><code><![CDATA[{script}]]></code></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>")
}

fn solution() -> (tempfile::TempDir, Solution) {
    let temporary = tempfile::tempdir().unwrap();
    std::fs::write(
        temporary.path().join("twaco.toml"),
        "[[project]]\nname = \"P\"\n",
    )
    .unwrap();
    std::fs::create_dir(temporary.path().join("Things")).unwrap();
    std::fs::write(
        temporary.path().join("Things/P.T.xml"),
        xml("one", "base();"),
    )
    .unwrap();
    let solution = Solution::load(&temporary.path().join("twaco.toml")).unwrap();
    (temporary, solution)
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            message,
        ],
    );
}

#[test]
fn from_export_reads_services_and_from_solution_prefers_the_sidecar() {
    let (temporary, solution) = solution();
    let export = temporary.path().join("export.xml");
    std::fs::write(&export, xml("one", "export();")).unwrap();
    assert_eq!(
        Side::from_export(&export)
            .unwrap()
            .services
            .get(&(String::from("P.T"), String::from("S")))
            .unwrap(),
        "export();"
    );
    let sidecar = temporary.path().join("src/P.T/services/S");
    std::fs::create_dir_all(&sidecar).unwrap();
    std::fs::write(sidecar.join("script.js"), "sidecar();").unwrap();
    assert_eq!(
        Side::from_solution(&solution)
            .unwrap()
            .services
            .get(&(String::from("P.T"), String::from("S")))
            .unwrap(),
        "sidecar();"
    );
}

#[test]
fn revision_loads_the_requested_commit_and_unknown_revision_names_it() {
    let (temporary, solution) = solution();
    git(temporary.path(), &["init"]);
    commit(temporary.path(), "one");
    let first = git(temporary.path(), &["rev-parse", "HEAD"]);
    std::fs::write(
        temporary.path().join("Things/P.T.xml"),
        xml("two", "two();"),
    )
    .unwrap();
    commit(temporary.path(), "two");
    let export = Side::from_solution(&solution).unwrap();
    let base = resolve(&solution, Some(&first), &export).unwrap().unwrap();
    assert_eq!(
        base.side
            .services
            .get(&(String::from("P.T"), String::from("S")))
            .unwrap(),
        "base();"
    );
    let error = resolve(&solution, Some("not-a-revision"), &export).unwrap_err();
    assert!(error.to_string().contains("not-a-revision"));
}

#[test]
fn handoffs_record_list_and_refuse_bad_inputs() {
    let (temporary, solution) = solution();
    git(temporary.path(), &["init"]);
    commit(temporary.path(), "one");
    let package = temporary.path().join("package.xml");
    std::fs::write(&package, xml("one", "base();")).unwrap();
    commit(temporary.path(), "package");
    let first = record_handoff(&solution, "first", std::slice::from_ref(&package)).unwrap();
    assert!(first.commit.is_some());
    assert!(!first.dirty);
    assert!(temporary
        .path()
        .join(".twaco/handoffs/first/handoff.json")
        .is_file());
    assert!(matches!(
        record_handoff(&solution, "first", std::slice::from_ref(&package)),
        Err(AdoptError::AlreadyExists { .. })
    ));
    assert!(record_handoff(&solution, "../bad", std::slice::from_ref(&package)).is_err());
    let plain = temporary.path().join("not-entities.xml");
    std::fs::write(&plain, "<notEntities/>").unwrap();
    assert!(record_handoff(&solution, "bad-xml", &[plain]).is_err());
    std::thread::sleep(std::time::Duration::from_millis(2));
    record_handoff(&solution, "newest", std::slice::from_ref(&package)).unwrap();
    assert_eq!(
        handoffs(&solution)
            .unwrap()
            .into_iter()
            .map(|h| h.name)
            .collect::<Vec<_>>(),
        ["newest", "first"]
    );
}

#[test]
fn resolve_selects_matching_handoff_and_each_explicit_source() {
    let (temporary, solution) = solution();
    let first = temporary.path().join("first.xml");
    let second = temporary.path().join("second.xml");
    std::fs::write(&first, xml("one", "base();")).unwrap();
    std::fs::write(&second, xml("two", "two();")).unwrap();
    record_handoff(&solution, "first", std::slice::from_ref(&first)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    record_handoff(&solution, "second", std::slice::from_ref(&second)).unwrap();
    let export = Side::from_export(&first).unwrap();
    assert!(resolve(&solution, None, &export)
        .unwrap()
        .unwrap()
        .label
        .contains("first"));
    assert!(resolve(&solution, Some("second"), &export)
        .unwrap()
        .unwrap()
        .label
        .contains("second"));
    assert!(resolve(&solution, Some(first.to_str().unwrap()), &export)
        .unwrap()
        .unwrap()
        .label
        .contains("file"));
    let empty = tempfile::tempdir().unwrap();
    std::fs::write(empty.path().join("twaco.toml"), "[[project]]\nname=\"P\"\n").unwrap();
    let empty_solution = Solution::load(&empty.path().join("twaco.toml")).unwrap();
    assert!(resolve(&empty_solution, None, &export).unwrap().is_none());
}

#[test]
fn history_versions_are_newest_first_and_ignore_untracked_paths() {
    let (temporary, solution) = solution();
    git(temporary.path(), &["init"]);
    commit(temporary.path(), "one");
    let path = temporary.path().join("Things/P.T.xml");
    std::fs::write(&path, xml("two", "two();")).unwrap();
    commit(temporary.path(), "two");
    let versions = history_versions(&solution, &path, 50);
    assert!(String::from_utf8_lossy(&versions[0]).contains("two();"));
    assert!(String::from_utf8_lossy(&versions[1]).contains("base();"));
    assert!(history_versions(&solution, &temporary.path().join("untracked.xml"), 50).is_empty());
}
