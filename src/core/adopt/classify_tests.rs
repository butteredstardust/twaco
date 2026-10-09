use super::*;
use std::path::Path;
use std::process::Command;

fn entity_xml(collection: &str, value: &str, script: Option<&str>) -> String {
    let service = script.map(|body| format!("<ThingShape><ServiceImplementations><ServiceImplementation name=\"S\"><code><![CDATA[{body}]]></code></ServiceImplementation></ServiceImplementations></ThingShape>")).unwrap_or_default();
    format!("<Entities><{collection}><Thing name=\"P.T\" projectName=\"P\"><value>{value}</value>{service}</Thing></{collection}></Entities>")
}

fn case(
    collection: &str,
    base: &str,
    ours: &str,
    theirs: &str,
    scripts: Option<(&str, &str, &str)>,
) -> (
    tempfile::TempDir,
    Solution,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    std::fs::create_dir(dir.path().join(collection)).unwrap();
    std::fs::write(
        dir.path().join(format!("{collection}/P.T.xml")),
        entity_xml(collection, ours, scripts.map(|(_, ours, _)| ours)),
    )
    .unwrap();
    if let Some((_, ours, _)) = scripts {
        let sidecar = dir.path().join("src/P.T/services/S");
        std::fs::create_dir_all(&sidecar).unwrap();
        std::fs::write(sidecar.join("script.js"), ours).unwrap();
    }
    let base_file = dir.path().join("base.xml");
    std::fs::write(
        &base_file,
        entity_xml(collection, base, scripts.map(|(base, _, _)| base)),
    )
    .unwrap();
    let export = dir.path().join("export.xml");
    std::fs::write(
        &export,
        entity_xml(collection, theirs, scripts.map(|(_, _, theirs)| theirs)),
    )
    .unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution, base_file, export)
}

fn report(solution: &Solution, base: &Path, export: &Path) -> Report {
    compare_with(
        solution,
        export,
        &[],
        &CompareOptions {
            base: Some(base.display().to_string()),
            only_kind: None,
        },
    )
    .unwrap()
}

#[test]
fn entity_rows_same_stale_theirs_conflict_added_and_we_removed() {
    for (base, ours, theirs, expected) in [
        ("b", "b", "b", Change::Same),
        ("b", "o", "b", Change::Stale),
        ("b", "b", "t", Change::Theirs),
        ("b", "o", "t", Change::Conflict),
    ] {
        let (_dir, solution, base_file, export) = case("Things", base, ours, theirs, None);
        assert_eq!(
            report(&solution, &base_file, &export).entities[0].change,
            expected
        );
    }
    let (_dir, solution, base_file, export) = case("Things", "b", "o", "t", None);
    std::fs::remove_file(solution.root.join("Things/P.T.xml")).unwrap();
    assert_eq!(
        report(&solution, &base_file, &export).entities[0].change,
        Change::WeRemoved
    );
    let (_dir, solution, _base_file, export) = case("Things", "b", "o", "t", None);
    std::fs::remove_file(solution.root.join("Things/P.T.xml")).unwrap();
    assert_eq!(
        compare(&solution, &export, &[]).unwrap().entities[0].change,
        Change::Added
    );
    let (_dir, solution, _base_file, export) = case("Things", "b", "o", "t", None);
    assert_eq!(
        compare(&solution, &export, &[]).unwrap().entities[0].change,
        Change::Unknown
    );
}

#[test]
fn service_rows_same_stale_theirs_conflict_and_we_removed() {
    // A service that is the same everywhere is not reported at all.
    let (_dir, solution, base_file, export) =
        case("Things", "v", "v", "v", Some(("b();", "b();", "b();")));
    assert!(report(&solution, &base_file, &export).services.is_empty());
    for (base, ours, theirs, expected) in [
        ("b();", "o();", "b();", Change::Stale),
        ("b();", "b();", "t();", Change::Theirs),
        ("b();", "o();", "t();", Change::Conflict),
    ] {
        let (_dir, solution, base_file, export) =
            case("Things", "v", "v", "v", Some((base, ours, theirs)));
        assert_eq!(
            report(&solution, &base_file, &export).services[0].change,
            expected
        );
    }
    let (_dir, solution, base_file, export) =
        case("Things", "v", "v", "v", Some(("b();", "o();", "t();")));
    std::fs::remove_dir_all(solution.root.join("src/P.T/services/S")).unwrap();
    std::fs::write(
        solution.root.join("Things/P.T.xml"),
        entity_xml("Things", "v", None),
    )
    .unwrap();
    assert_eq!(
        report(&solution, &base_file, &export).services[0].change,
        Change::WeRemoved
    );
    let (_dir, solution, _base_file, export) =
        case("Things", "v", "v", "v", Some(("b();", "o();", "t();")));
    std::fs::remove_dir_all(solution.root.join("src/P.T/services/S")).unwrap();
    std::fs::write(
        solution.root.join("Things/P.T.xml"),
        entity_xml("Things", "v", None),
    )
    .unwrap();
    assert_eq!(
        compare(&solution, &export, &[]).unwrap().services[0].change,
        Change::Added
    );
    let (_dir, solution, _base_file, export) =
        case("Things", "v", "v", "v", Some(("b();", "o();", "t();")));
    assert_eq!(
        compare(&solution, &export, &[]).unwrap().services[0].change,
        Change::Unknown
    );
}

#[test]
fn noise_kind_only_kind_and_reverts_follow_the_contract() {
    let (_dir, solution, base, export) = case("Mashups", "b", "b", "b", None);
    let mut xml = std::fs::read_to_string(&export).unwrap();
    xml = xml.replace(
        "<value>b</value>",
        "<value>b</value><Owner>different</Owner>",
    );
    std::fs::write(&export, xml).unwrap();
    let result = report(&solution, &base, &export);
    assert_eq!(result.entities[0].change, Change::Same);
    assert_eq!(result.entities[0].kind, Kind::Ui);
    std::fs::write(
        solution.root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\n[bundle]\nui_collections = [\"Things\"]\n",
    )
    .unwrap();
    let overridden = Solution::load(&solution.root.join("twaco.toml")).unwrap();
    let things_export = solution.root.join("things-export.xml");
    std::fs::write(&things_export, entity_xml("Things", "b", None)).unwrap();
    assert_eq!(
        compare(&overridden, &things_export, &[]).unwrap().entities[0].kind,
        Kind::Ui
    );
    let backend = compare_with(
        &solution,
        &export,
        &[],
        &CompareOptions {
            base: None,
            only_kind: Some(Kind::Backend),
        },
    )
    .unwrap();
    assert!(backend.entities.is_empty() && backend.services.is_empty());
    let (_dir, solution, base, export) =
        case("Things", "v", "o", "t", Some(("b();", "o();", "t();")));
    let mut result = report(&solution, &base, &export);
    assert_eq!(result.reverts().count(), 1);
    result.services[0].generated = true;
    assert_eq!(result.reverts().count(), 0);
}

#[test]
fn history_fallback_marks_committed_values_stale() {
    let (dir, solution, _base, export) = case(
        "Things",
        "old",
        "old",
        "old",
        Some(("old();", "old();", "old();")),
    );
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    };
    run(&["init"]);
    run(&["add", "."]);
    run(&[
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@x",
        "commit",
        "-m",
        "old",
    ]);
    std::fs::write(
        solution.root.join("Things/P.T.xml"),
        entity_xml("Things", "new", Some("new();")),
    )
    .unwrap();
    std::fs::write(solution.root.join("src/P.T/services/S/script.js"), "new();").unwrap();
    let result = compare(&solution, &export, &[]).unwrap();
    assert_eq!(result.entities[0].change, Change::Stale);
    assert_eq!(result.services[0].change, Change::Stale);
}

#[test]
fn only_theirs_and_added_are_not_reverts_and_a_name_filter_keeps_the_service_check() {
    let (_dir, solution, base, export) =
        case("Things", "v", "v", "t", Some(("b();", "b();", "t();")));
    let result = report(&solution, &base, &export);
    assert_eq!(result.services[0].change, Change::Theirs);
    assert_eq!(result.reverts().count(), 0);
    let (_dir, solution, base, export) =
        case("Things", "v", "v", "v", Some(("b();", "o();", "b();")));
    let narrowed = compare_with(
        &solution,
        &export,
        &["Elsewhere".to_string()],
        &CompareOptions {
            base: Some(base.display().to_string()),
            only_kind: None,
        },
    )
    .unwrap();
    assert!(narrowed.entities.is_empty());
    assert_eq!(
        narrowed.services.len(),
        1,
        "the stale service is still reported"
    );
    assert_eq!(narrowed.reverts().count(), 1);
}
