use std::process::Command;

#[test]
fn cli_new_spec_and_build_plan_apply_and_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Page_MU\n  template: blank\n  slots:\n    center:\n      - label: {id: title, text: Acme}\n";
    std::fs::write(root.join("Acme.page.yaml"), spec).unwrap();
    let run = |args: &[&str], success: bool| {
        let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let plan = run(
        &[
            "mashup",
            "new",
            "Acme.Page_MU",
            "--spec",
            "Acme.page.yaml",
            "--json",
        ],
        true,
    );
    assert_eq!(plan["applied"], false);
    assert!(!root.join("src").exists());
    let new = run(
        &[
            "mashup",
            "new",
            "Acme.Page_MU",
            "--spec",
            "Acme.page.yaml",
            "--apply",
            "--json",
        ],
        true,
    );
    assert_eq!(new["applied"], true);
    let page = root.join("src/Acme.Page_MU/page.yaml");
    assert_eq!(std::fs::read_to_string(&page).unwrap(), spec);
    let plan = run(&["mashup", "build", "Acme.Page_MU", "--json"], true);
    assert_eq!(plan["files"], serde_json::json!([]));
    std::fs::write(&page, spec.replace("text: Acme", "text: Acme updated")).unwrap();
    let plan = run(&["mashup", "build", "Acme.Page_MU", "--json"], true);
    assert_eq!(plan["nodes"][0]["status"], "changed");
    let applied = run(
        &["mashup", "build", "Acme.Page_MU", "--apply", "--json"],
        true,
    );
    assert_eq!(applied["applied"], true);
    let content = root.join("src/Acme.Page_MU/mashup/content.json");
    assert!(std::fs::read_to_string(&content)
        .unwrap()
        .contains("Acme updated"));

    let widget = applied["nodes"][0]["widget"].as_str().unwrap();
    let edited = run(
        &[
            "mashup",
            "set",
            "Acme.Page_MU",
            widget,
            "--prop",
            "LabelText=Handmade",
            "--apply",
            "--json",
        ],
        true,
    );
    assert_eq!(edited["applied"], true);
    std::fs::write(&page, spec.replace("text: Acme", "text: Competing")).unwrap();
    let before = std::fs::read(&content).unwrap();
    let conflict = run(
        &["mashup", "build", "Acme.Page_MU", "--apply", "--json"],
        false,
    );
    assert!(!conflict["conflicts"].as_array().unwrap().is_empty());
    assert_eq!(std::fs::read(&content).unwrap(), before);
}
