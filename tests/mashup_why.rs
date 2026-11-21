//! `twaco mashup why`, held to files.
//!
//! Mashups are built from templates and authoring commands in a scratch solution, and what `why`
//! prints for a widget (text and JSON) is compared with `tests/fixtures/mashup/why/<case>.txt`:
//! the decision, the rule that made it, where it came from, what the mashup holds now, and how to
//! override it.
//!
//! ```sh
//! TWACO_BLESS=1 cargo test --test mashup_why
//! git diff tests/fixtures/mashup/why
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(args)
        .current_dir(root)
        .env_clear()
        .output()
        .unwrap()
}

fn twaco(root: &Path, args: &[&str]) -> String {
    let output = run(root, args);
    assert!(
        output.status.success(),
        "twaco {args:?}\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn solution() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("twaco.toml"),
        "[[project]]\nname = \"Acme\"\n",
    )
    .unwrap();
    twaco(dir.path(), &["new", "project", "Acme", "--apply"]);
    dir
}

fn apply(root: &Path, args: &[&str]) {
    let mut all = args.to_vec();
    all.push("--apply");
    twaco(root, &all);
}

fn template(root: &Path, name: &str, template: &str) {
    apply(root, &["mashup", "new", name, "--template", template]);
}

/// Text, then JSON, for one widget.
fn why(root: &Path, mashup: &str, widget: &str) -> String {
    format!(
        "=== twaco mashup why {mashup} {widget}\n{}=== twaco mashup why {mashup} {widget} --json\n{}",
        twaco(root, &["mashup", "why", mashup, widget]),
        twaco(root, &["mashup", "why", mashup, widget, "--json"]),
    )
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mashup/why")
        .join(format!("{name}.txt"))
}

fn check(name: &str, actual: &str, differing: &mut Vec<String>) {
    let path = fixture(name);
    if std::env::var("TWACO_BLESS").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    match std::fs::read_to_string(&path) {
        Ok(expected) if expected.replace("\r\n", "\n") == actual => {}
        Ok(_) => differing.push(format!("{name}: differs from {}", path.display())),
        Err(_) => differing.push(format!("{name}: no fixture at {}", path.display())),
    }
}

#[test]
fn why_output_matches_the_fixtures() {
    let mut differing = Vec::new();

    let dir = solution();
    template(dir.path(), "Acme.Case_MU", "list-page");
    check(
        "toolbar-band",
        &why(dir.path(), "Acme.Case_MU", "toolbar"),
        &mut differing,
    );

    let dir = solution();
    let root = dir.path();
    template(root, "Acme.Case_MU", "list-page");
    apply(
        root,
        &[
            "mashup",
            "add",
            "Acme.Case_MU",
            "label",
            "--in",
            "body",
            "--label",
            "Head",
        ],
    );
    apply(
        root,
        &[
            "mashup",
            "add",
            "Acme.Case_MU",
            "container",
            "--below",
            "ptcslabel-6",
        ],
    );
    for _ in 0..2 {
        apply(
            root,
            &[
                "mashup",
                "add",
                "Acme.Case_MU",
                "container",
                "--right-of",
                "flexcontainer-7",
            ],
        );
    }
    check(
        "card-row",
        &why(root, "Acme.Case_MU", "container-10"),
        &mut differing,
    );
    check(
        "card-row-leaf",
        &why(root, "Acme.Case_MU", "ptcslabel-6"),
        &mut differing,
    );
    apply(
        root,
        &[
            "mashup",
            "set",
            "Acme.Case_MU",
            "container-10",
            "--prop",
            "flex-wrap=wrap",
        ],
    );
    check(
        "card-row-authored",
        &why(root, "Acme.Case_MU", "container-10"),
        &mut differing,
    );
    // An unequal row is a validation advisory that names the rule, never an error.
    apply(
        root,
        &[
            "mashup",
            "set",
            "Acme.Case_MU",
            "flexcontainer-11",
            "--prop",
            "flex-grow=3",
        ],
    );
    let report = twaco(root, &["mashup", "validate", "Acme.Case_MU"]);
    assert!(
        !report.contains("layout-siblings"),
        "authored rows are not judged: {report}"
    );

    let dir = solution();
    template(dir.path(), "Acme.Case_MU", "tab-content");
    check(
        "tab-content",
        &why(dir.path(), "Acme.Case_MU", "toolbar"),
        &mut differing,
    );

    let dir = solution();
    template(dir.path(), "Acme.Case_MU", "filter-panel");
    check(
        "filter-footer",
        &why(dir.path(), "Acme.Case_MU", "filter-footer"),
        &mut differing,
    );

    assert!(differing.is_empty(), "{}", differing.join("\n"));
}

#[test]
fn a_hand_changed_template_slot_is_an_advisory_naming_its_rule() {
    let dir = solution();
    let root = dir.path();
    template(root, "Acme.Case_MU", "list-page");
    let clean = twaco(root, &["mashup", "validate", "Acme.Case_MU"]);
    assert!(!clean.contains("layout-"), "{clean}");
    // The sidecar and the entity both hold the content; change both, as an editor and a sync would.
    for file in [
        "src/Acme.Case_MU/mashup/content.json",
        "Mashups/Acme.Case_MU.xml",
    ] {
        let path = root.join(file);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("50px"), "{file}");
        std::fs::write(&path, content.replace("50px", "64px")).unwrap();
    }
    let output = run(root, &["mashup", "validate", "Acme.Case_MU", "--json"]);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let found: serde_json::Value = serde_json::from_str(&text).unwrap();
    let problems = found["mashups"][0]["problems"].as_array().unwrap();
    let advisory = problems
        .iter()
        .find(|problem| problem["rule"] == "layout-slot-size")
        .unwrap_or_else(|| panic!("{text}"));
    assert_eq!(advisory["severity"], "advisory");
    assert!(
        advisory["message"]
            .as_str()
            .unwrap()
            .contains("size.toolbar-50"),
        "{advisory}"
    );
    assert!(
        output.status.success(),
        "an advisory does not fail the check"
    );
}

#[test]
fn an_unknown_widget_is_refused_and_why_needs_both_operands() {
    let dir = solution();
    let root = dir.path();
    template(root, "Acme.Case_MU", "list-page");
    let output = run(root, &["mashup", "why", "Acme.Case_MU", "nothing-here"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no widget in the mashup is nothing-here")
    );
    let output = run(root, &["mashup", "why", "Acme.Case_MU"]);
    assert!(!output.status.success());
    let output = run(root, &["mashup", "why", "Acme.Nope_MU", "toolbar"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("no mashup Acme.Nope_MU"));
}
