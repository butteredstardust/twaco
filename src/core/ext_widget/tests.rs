use super::*;

fn options(name: &str) -> NewOptions<'_> {
    NewOptions {
        name,
        package: None,
        vendor: Some("Acme \"Labs\""),
        version: None,
        min_thingworx: None,
        description: None,
        sdk: None,
    }
}

fn written(root: &Path, name: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let _ = root;
    new_widget(&dir.path().join(name), &options(name))
        .unwrap()
        .apply()
        .unwrap();
    dir
}

#[test]
fn a_new_widget_project_is_read_as_one_and_checks_clean() {
    let dir = written(Path::new("."), "AcmeDial");
    let root = dir.path().join("AcmeDial");
    let project = read(&root).unwrap().unwrap();
    assert_eq!(
        project,
        NpmProject {
            package: "AcmeDial".into(),
            version: "1.0.0".into(),
            runner: "npm",
        }
    );
    let json: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("package.json")).unwrap()).unwrap();
    assert_eq!(json["name"], "acmedial");
    assert_eq!(
        json["author"], "Acme \"Labs\"",
        "a vendor is written as JSON"
    );
    let ide = std::fs::read_to_string(root.join("src/ide/index.ts")).unwrap();
    assert!(
        ide.contains("class AcmeDial extends TWComposerWidget"),
        "{ide}"
    );
    assert!(std::fs::read_to_string(root.join("src/runtime/index.ts"))
        .unwrap()
        .contains("class AcmeDial extends TWRuntimeWidget"));
    let report = check(&root);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert_eq!(report.package.as_deref(), Some("AcmeDial"));
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("node_modules")),
        "{:?}",
        report.notes
    );
}

#[test]
fn names_and_versions_a_widget_cannot_have_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let error = new_widget(&dir.path().join("x"), &options("9Dial")).unwrap_err();
    assert!(error.contains("not a widget package name"), "{error}");
    let error = new_widget(
        &dir.path().join("y"),
        &NewOptions {
            version: Some("1.0"),
            ..options("Dial")
        },
    )
    .unwrap_err();
    assert!(error.contains("--package-version"), "{error}");
    std::fs::create_dir_all(dir.path().join("full")).unwrap();
    std::fs::write(dir.path().join("full/x"), "").unwrap();
    assert!(new_widget(&dir.path().join("full"), &options("Dial"))
        .unwrap_err()
        .contains("not empty"));
}

#[test]
fn a_template_project_is_read_by_its_package_json_rules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // The template's own metadata.xml leaves the name empty; packageName is optional.
    std::fs::write(
        root.join("package.json"),
        r#"{"name": "demo", "version": "2.1.0", "scripts": {"build": "webpack"}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("metadata.xml"),
        r#"<Entities><ExtensionPackages><ExtensionPackage name="" packageVersion=""/></ExtensionPackages></Entities>"#,
    )
    .unwrap();
    std::fs::write(root.join("yarn.lock"), "").unwrap();
    let project = read(root).unwrap().unwrap();
    assert_eq!(project.package, "demo_ExtensionPackage");
    assert_eq!(project.runner, "yarn");

    // A Java project with a package.json for its tooling stays a Java project, even while its
    // metadata.xml is broken.
    std::fs::write(root.join("build.gradle"), "").unwrap();
    assert!(read(root).is_none());
}

#[test]
fn the_version_is_written_in_place_and_nothing_else_moves() {
    let dir = tempfile::tempdir().unwrap();
    let text = "{\r\n  \"name\": \"x\",\r\n  \"config\": {\"version\": \"9.9.9\"},\r\n  \"description\": \"says \\\"version\\\"\",\r\n  \"version\": \"1.2.3\"\r\n}\r\n";
    std::fs::write(dir.path().join("package.json"), text).unwrap();
    set_version(dir.path(), "1.2.4").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
        text.replace("\"1.2.3\"", "\"1.2.4\"")
    );
    std::fs::write(dir.path().join("package.json"), "{\"name\": \"x\"}").unwrap();
    assert!(set_version(dir.path(), "1.0.0").is_err());
    std::fs::write(
        dir.path().join("package.json"),
        "{\"\\u0076ersion\": \"1.0.0\"}",
    )
    .unwrap();
    set_version(dir.path(), "1.0.1").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
        "{\"\\u0076ersion\": \"1.0.1\"}"
    );
}

#[test]
fn a_gulp_project_keeping_its_metadata_at_the_root_is_one_and_its_versions_must_agree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("package.json"),
        r#"{"packageName": "heat", "version": "1.1.4", "scripts": {"build": "gulp"}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("metadata.xml"),
        r#"<Entities><ExtensionPackages><ExtensionPackage name="heat" packageVersion="1.0.0"/></ExtensionPackages></Entities>"#,
    )
    .unwrap();
    assert_eq!(read(root).unwrap().unwrap().package, "heat");
    let findings = check(root).findings;
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].rule, "version-mismatch");

    // Without a build script, a package.json beside a real metadata.xml is only tooling.
    std::fs::write(
        root.join("package.json"),
        r#"{"name": "lint", "version": "0.0.1"}"#,
    )
    .unwrap();
    assert!(read(root).is_none());
}

#[test]
fn check_names_what_a_build_needs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("package.json"),
        r#"{"packageName": "Bad-Name", "version": "1.0", "scripts": {}}"#,
    )
    .unwrap();
    std::fs::write(root.join("build.mjs"), "").unwrap();
    let rules: Vec<&str> = check(root).findings.iter().map(|f| f.rule).collect();
    assert_eq!(
        rules,
        [
            "package-name",
            "version",
            "no-build-script",
            "missing-entry",
            "missing-entry"
        ]
    );
}
