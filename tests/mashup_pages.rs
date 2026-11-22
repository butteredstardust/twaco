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

fn copy_fixture(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.path().is_dir() {
            copy_fixture(&entry.path(), &destination);
        } else {
            std::fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn cli(root: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .current_dir(root)
        .env_clear()
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn mcp(root: &std::path::Path, tool: &str, arguments: serde_json::Value) -> serde_json::Value {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .current_dir(root)
        .env_clear()
        .env("TWACO_ROOT", root)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})).unwrap();
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":arguments}})).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|v| v["id"] == 1)
        .unwrap();
    assert_ne!(response["result"]["isError"], true, "{response}");
    let mut value = response["result"]["structuredContent"].clone();
    value.as_object_mut().unwrap().remove("duration_ms");
    value
}

fn benchmark_lifecycle(id: &str) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    copy_fixture(&fixtures.join("bench_solution"), root);
    let text =
        std::fs::read_to_string(fixtures.join(format!("bench_{}/page.yaml", id.to_lowercase())))
            .unwrap();
    std::fs::write(root.join("input.page.yaml"), &text).unwrap();
    let name = format!("Acme.BenchSpec{id}_MU");
    let new_args = [
        "mashup",
        "new",
        &name,
        "--project",
        "Acme.Bench",
        "--spec",
        "input.page.yaml",
        "--json",
    ];
    let plan = cli(root, &new_args);
    let remote = mcp(
        root,
        "mashup_new",
        serde_json::json!({"name":name,"project":"Acme.Bench","spec":"input.page.yaml"}),
    );
    assert_eq!(plan, remote);
    assert_eq!(plan["applied"], false);
    assert!(!root.join("Mashups").exists());
    let mut apply_args = new_args.to_vec();
    apply_args.push("--apply");
    cli(root, &apply_args);
    let artifacts = root.join(format!("src/{name}"));
    let content = artifacts.join("mashup/content.json");
    let before = std::fs::read(&content).unwrap();
    let rebuild = mcp(root, "mashup_build", serde_json::json!({"mashup":name}));
    assert_eq!(rebuild["files"], serde_json::json!([]));
    assert_eq!(std::fs::read(&content).unwrap(), before);
    let baseline: serde_json::Value =
        serde_json::from_slice(&std::fs::read(artifacts.join(".page-baseline.json")).unwrap())
            .unwrap();
    let (node, prop, old) = if id == "N" {
        ("preview", "Label", "Preview")
    } else {
        (
            "header--title",
            "LabelText",
            match id {
                "A" => "Asset KPI",
                "H" => "Asset List Top Filter",
                "J" => "Alerts & Notifications",
                "O" => "Gauges & KPI",
                "Q1" => "Operations Overview",
                "Q2" => "Press 04",
                "Q3" => "Plant Dashboard",
                "Q4" => "Fleet",
                "L" => "Compared Assets",
                "M" => "Metrics List Side Filter",
                _ => "Admin Metrics",
            },
        )
    };
    let widget = baseline["ids"][node].as_str().unwrap();
    cli(
        root,
        &[
            "mashup",
            "set",
            &name,
            widget,
            "--prop",
            &format!("{prop}=Handmade"),
            "--apply",
            "--json",
        ],
    );
    let handmade = std::fs::read(&content).unwrap();
    let retained = mcp(
        root,
        "mashup_build",
        serde_json::json!({"mashup":name,"dry_run":false}),
    );
    assert_eq!(retained["files"], serde_json::json!([]));
    assert_eq!(std::fs::read(&content).unwrap(), handmade);
    std::fs::write(
        artifacts.join("page.yaml"),
        text.replace(old, "Changed by spec"),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .current_dir(root)
        .env_clear()
        .args(["mashup", "build", &name, "--apply", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let conflict: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!conflict["conflicts"].as_array().unwrap().is_empty());
    assert_eq!(std::fs::read(&content).unwrap(), handmade);
}

#[test]
fn benchmark_a_frontends_and_rebuilds() {
    benchmark_lifecycle("A");
}
#[test]
fn benchmark_h_frontends_and_rebuilds() {
    benchmark_lifecycle("H");
}
#[test]
fn benchmark_j_frontends_and_rebuilds() {
    benchmark_lifecycle("J");
}
#[test]
fn benchmark_k_frontends_and_rebuilds() {
    benchmark_lifecycle("K");
}
#[test]
fn benchmark_n_frontends_and_rebuilds() {
    benchmark_lifecycle("N");
}
#[test]
fn benchmark_o_frontends_and_rebuilds() {
    benchmark_lifecycle("O");
}

#[test]
fn benchmark_q1_frontends_and_rebuilds() {
    benchmark_lifecycle("Q1");
}
#[test]
fn benchmark_q2_frontends_and_rebuilds() {
    benchmark_lifecycle("Q2");
}
#[test]
fn benchmark_q3_frontends_and_rebuilds() {
    benchmark_lifecycle("Q3");
}
#[test]
fn benchmark_q4_frontends_and_rebuilds() {
    benchmark_lifecycle("Q4");
}

#[test]
fn benchmark_l_frontends_and_rebuilds() {
    benchmark_lifecycle("L");
}
#[test]
fn benchmark_m_frontends_and_rebuilds() {
    benchmark_lifecycle("M");
}

#[test]
fn a_generated_cell_is_created_once_and_never_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    copy_fixture(&fixtures.join("bench_solution"), root);
    std::fs::copy(
        fixtures.join("bench_l/page.yaml"),
        root.join("input.page.yaml"),
    )
    .unwrap();
    let args = [
        "mashup",
        "new",
        "Acme.BenchSpecL_MU",
        "--project",
        "Acme.Bench",
        "--spec",
        "input.page.yaml",
        "--apply",
        "--json",
    ];
    let applied = cli(root, &args);
    assert_eq!(
        applied["cells"],
        serde_json::json!(["Acme.BenchSpecLCell_MU"])
    );
    let cell = root.join("src/Acme.BenchSpecLCell_MU");
    assert!(cell.join("page.yaml").exists());
    assert!(cell.join(".page-baseline.json").exists());
    assert!(root.join("Mashups/Acme.BenchSpecLCell_MU.xml").exists());
    // The cell is a page of its own: rebuilding it is a no-op, and a hand edit survives the
    // page's rebuild, which does not touch it.
    let rebuilt = cli(
        root,
        &["mashup", "build", "Acme.BenchSpecLCell_MU", "--json"],
    );
    assert_eq!(rebuilt["files"], serde_json::json!([]));
    let content = cell.join("mashup/content.json");
    let baseline: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cell.join(".page-baseline.json")).unwrap()).unwrap();
    let title = baseline["ids"]["title"].as_str().unwrap();
    let original = std::fs::read_to_string(&content).unwrap();
    cli(
        root,
        &[
            "mashup",
            "set",
            "Acme.BenchSpecLCell_MU",
            title,
            "--prop",
            "LabelType=title",
            "--apply",
            "--json",
        ],
    );
    let handmade = std::fs::read_to_string(&content).unwrap();
    assert_ne!(handmade, original);
    let again = cli(root, &["mashup", "build", "Acme.BenchSpecL_MU", "--json"]);
    assert_eq!(again["files"], serde_json::json!([]));
    assert_eq!(again["cells"], serde_json::json!([]));
    assert_eq!(std::fs::read_to_string(&content).unwrap(), handmade);
}
