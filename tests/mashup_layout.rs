//! Mashup layout output, held to files.
//!
//! Every embedded template is instantiated with `mashup new --template`, and a set of authoring
//! sequences (placement, toolbar band, panel fit, card recipe, label pairs, tabs, filter panel with
//! footer) runs in a scratch solution. The mashup files they leave (entity XML, content.json,
//! custom.css) are compared with `tests/fixtures/mashup/layout/<case>.txt`, so a change to layout
//! decisions is a decision of its own. Random UUIDs are written `<UUID-n>` by first appearance.
//!
//! ```sh
//! TWACO_BLESS=1 cargo test --test mashup_layout
//! git diff tests/fixtures/mashup/layout
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn twaco(root: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(args)
        .current_dir(root)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "twaco {args:?}\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A scratch solution with a Thing whose `GetRows` service returns a DataShape.
fn solution() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"Acme\"\n").unwrap();
    twaco(root, &["new", "project", "Acme", "--apply"]);
    twaco(
        root,
        &[
            "new",
            "data-shape",
            "Acme.Rows_DS",
            "--field",
            "id:STRING:primary",
            "--field",
            "qty:INTEGER",
            "--apply",
        ],
    );
    twaco(
        root,
        &[
            "new",
            "thing",
            "Acme.Store_T",
            "--template",
            "GenericThing",
            "--apply",
        ],
    );
    twaco(
        root,
        &[
            "thing",
            "service",
            "add",
            "Acme.Store_T",
            "GetRows",
            "--param",
            "limit:INTEGER",
            "--returns",
            "INFOTABLE:Acme.Rows_DS",
            "--apply",
        ],
    );
    dir
}

fn files(dir: &Path, relative: &str, out: &mut BTreeMap<String, String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        let name = format!("{relative}{}", path.file_name().unwrap().to_string_lossy());
        if path.is_dir() {
            files(&path, &format!("{name}/"), out);
        } else {
            let text =
                String::from_utf8_lossy(&std::fs::read(&path).unwrap()).replace("\r\n", "\n");
            out.insert(name, text);
        }
    }
}

/// Replace each distinct UUID by `<UUID-n>`, numbered by first appearance.
fn stable_uuids(text: &str) -> String {
    let bytes = text.as_bytes();
    let shape = [8, 4, 4, 4, 12];
    let mut seen: Vec<String> = Vec::new();
    let mut out = String::new();
    let mut at = 0;
    while at < bytes.len() {
        if let Some(length) = uuid_at(&bytes[at..], &shape) {
            let id = text[at..at + length].to_ascii_lowercase();
            let index = seen
                .iter()
                .position(|known| *known == id)
                .unwrap_or_else(|| {
                    seen.push(id);
                    seen.len() - 1
                });
            out.push_str(&format!("<UUID-{}>", index + 1));
            at += length;
        } else {
            let character = text[at..].chars().next().unwrap();
            out.push(character);
            at += character.len_utf8();
        }
    }
    out
}

fn uuid_at(bytes: &[u8], shape: &[usize]) -> Option<usize> {
    let mut at = 0;
    for (index, group) in shape.iter().enumerate() {
        if index > 0 {
            if bytes.get(at) != Some(&b'-') {
                return None;
            }
            at += 1;
        }
        for _ in 0..*group {
            if !bytes.get(at)?.is_ascii_hexdigit() {
                return None;
            }
            at += 1;
        }
    }
    Some(at)
}

/// Every mashup file in the solution, in path order.
fn dump(root: &Path) -> String {
    let mut all = BTreeMap::new();
    files(&root.join("Mashups"), "Mashups/", &mut all);
    let mut sources = BTreeMap::new();
    files(&root.join("src"), "src/", &mut sources);
    all.extend(
        sources
            .into_iter()
            .filter(|(path, _)| path.contains("_MU/")),
    );
    let mut text = String::new();
    for (path, body) in all {
        // The entity's copy of content.json is `mashup::sync`'s output; the sidecar is the golden.
        let body = match (body.find("<mashupContent>"), body.find("</mashupContent>")) {
            (Some(start), Some(end)) if path.ends_with(".xml") => format!(
                "{}<mashupContent>(content.json)</mashupContent>{}",
                &body[..start],
                &body[end + "</mashupContent>".len()..]
            ),
            _ => body,
        };
        text.push_str(&format!("=== {path}\n{body}\n"));
    }
    stable_uuids(&text)
}

/// One case: its name and the commands after `twaco`; `$M` stands for the case's mashup name.
type Steps = Vec<Vec<&'static str>>;

fn run_steps(root: &Path, mashup: &str, steps: &Steps) {
    for step in steps {
        let mut args: Vec<String> = step.iter().map(|word| word.replace("$M", mashup)).collect();
        args.push("--apply".into());
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        twaco(root, &borrowed);
    }
}

fn template_case(template: &str) -> (String, Steps) {
    let mut step = vec![
        "mashup",
        "new",
        "$M",
        "--template",
        Box::leak(template.to_string().into_boxed_str()) as &'static str,
        "--rules",
        "legacy",
    ];
    if template.contains("sidebar") {
        step.extend(["--primary-menu", "data:Acme.Store_T.GetRows"]);
    }
    (format!("template-{template}"), vec![step])
}

fn sequences() -> Vec<(String, Steps)> {
    let new = |template: &'static str| {
        vec![
            "mashup",
            "new",
            "$M",
            "--template",
            template,
            "--rules",
            "legacy",
        ]
    };
    vec![
        (
            "seq-placement".into(),
            vec![
                new("list-page"),
                vec!["mashup", "add", "$M", "grid", "--in", "body"],
                vec![
                    "mashup", "add", "$M", "button", "--in", "toolbar", "--label", "Go",
                ],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "label",
                    "--right-of",
                    "ptcsbutton-7",
                    "--label",
                    "Total",
                ],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "button",
                    "--below",
                    "ptcsgrid-6",
                    "--label",
                    "More",
                ],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "label",
                    "--above",
                    "ptcsgrid-6",
                    "--label",
                    "Heading",
                ],
                vec!["mashup", "add", "$M", "separator", "--below", "ptcsgrid-6"],
            ],
        ),
        (
            "seq-blank-placement".into(),
            vec![
                new("blank"),
                vec!["mashup", "add", "$M", "label", "--label", "One"],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "label",
                    "--right-of",
                    "ptcslabel-3",
                    "--label",
                    "Two",
                ],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "label",
                    "--below",
                    "ptcslabel-3",
                    "--label",
                    "Three",
                ],
            ],
        ),
        (
            "seq-toolbar-band".into(),
            vec![
                new("dashboard"),
                vec![
                    "mashup",
                    "recipe",
                    "toolbar",
                    "$M",
                    "flexcontainer-3",
                    "--band",
                ],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "button",
                    "--in",
                    "flexcontainer-3",
                    "--label",
                    "Refresh",
                ],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "datepicker",
                    "--left-of",
                    "ptcsbutton-12",
                ],
            ],
        ),
        (
            "seq-panel-fit".into(),
            vec![
                new("dashboard"),
                vec![
                    "mashup",
                    "recipe",
                    "panel",
                    "$M",
                    "flexcontainer-10",
                    "--fit",
                    "fill",
                ],
                vec![
                    "mashup",
                    "recipe",
                    "panel",
                    "$M",
                    "flexcontainer-11",
                    "--fit",
                    "content",
                    "--style",
                    "card",
                ],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "label",
                    "--in",
                    "flexcontainer-10",
                    "--label",
                    "Side",
                ],
            ],
        ),
        (
            "seq-card-recipe".into(),
            vec![
                new("dashboard"),
                vec![
                    "mashup",
                    "recipe",
                    "card",
                    "$M",
                    "flexcontainer-5",
                    "--title",
                    "Alpha",
                    "--say",
                    "12",
                    "--status",
                    "OK",
                    "--kind",
                    "ok",
                ],
                vec![
                    "mashup",
                    "recipe",
                    "card",
                    "$M",
                    "flexcontainer-6",
                    "--title",
                    "Beta",
                    "--say",
                    "7",
                    "--status",
                    "Late",
                    "--kind",
                    "warning",
                ],
                vec![
                    "mashup",
                    "recipe",
                    "card",
                    "$M",
                    "flexcontainer-7",
                    "--title",
                    "Gamma",
                ],
            ],
        ),
        (
            "seq-label-pairs".into(),
            vec![
                new("dashboard"),
                vec![
                    "mashup",
                    "source",
                    "add",
                    "$M",
                    "Acme.Store_T",
                    "GetRows",
                    "--on-load",
                ],
                vec![
                    "mashup",
                    "recipe",
                    "label-pairs",
                    "$M",
                    "flexcontainer-11",
                    "--data",
                    "data:Acme.Store_T.GetRows",
                    "--row",
                    "first",
                    "--title",
                    "Row",
                ],
            ],
        ),
        (
            "seq-filter-footer".into(),
            vec![
                new("filter-panel"),
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "textfield",
                    "--in",
                    "filters",
                    "--label",
                    "Search",
                ],
                vec![
                    "mashup", "add", "$M", "dropdown", "--in", "filters", "--label", "Category",
                ],
                vec![
                    "mashup", "add", "$M", "button", "--in", "filters", "--bottom", "--label",
                    "Apply",
                ],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "button",
                    "--right-of",
                    "ptcsbutton-13",
                    "--label",
                    "Clear",
                ],
                vec!["mashup", "add", "$M", "grid", "--in", "content"],
            ],
        ),
        (
            "seq-form-2col".into(),
            vec![
                new("form-2col"),
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "textfield",
                    "--in",
                    "left",
                    "--label",
                    "Name",
                ],
                vec![
                    "mashup",
                    "add",
                    "$M",
                    "textfield",
                    "--in",
                    "right",
                    "--label",
                    "Code",
                ],
                vec![
                    "mashup", "add", "$M", "button", "--in", "actions", "--label", "Save",
                ],
            ],
        ),
    ]
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mashup/layout")
        .join(format!("{name}.txt"))
}

fn template_names() -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let text = twaco(dir.path(), &["mashup", "templates", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|template| template["name"].as_str().unwrap().to_string())
        .collect()
}

/// What `mashup templates` prints for the list and for every template, plain and as JSON.
fn templates_view() -> String {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut text = String::new();
    let mut commands: Vec<Vec<String>> = vec![
        vec!["mashup".into(), "templates".into()],
        vec!["mashup".into(), "templates".into(), "--json".into()],
    ];
    for name in template_names() {
        commands.push(vec!["mashup".into(), "templates".into(), name.clone()]);
        commands.push(vec![
            "mashup".into(),
            "templates".into(),
            name,
            "--json".into(),
        ]);
    }
    for command in commands {
        let borrowed: Vec<&str> = command.iter().map(String::as_str).collect();
        text.push_str(&format!("=== twaco {}\n", command.join(" ")));
        text.push_str(&twaco(root, &borrowed).replace("\r\n", "\n"));
    }
    text
}

fn check(name: &str, actual: &str, bless: bool, differing: &mut Vec<String>) {
    let path = fixture_path(name);
    if bless {
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

fn cases() -> Vec<(String, Steps)> {
    let mut cases: Vec<(String, Steps)> = template_names()
        .iter()
        .map(|name| template_case(name))
        .collect();
    cases.extend(sequences());
    cases
}

#[test]
fn mashup_layout_output_matches_the_fixtures() {
    let bless = std::env::var("TWACO_BLESS").as_deref() == Ok("1");
    let mut differing = Vec::new();
    check("templates-view", &templates_view(), bless, &mut differing);
    for (name, steps) in cases() {
        let dir = solution();
        run_steps(dir.path(), "Acme.Case_MU", &steps);
        check(&name, &dump(dir.path()), bless, &mut differing);
    }
    // Tabs hold child mashups, so they are built here with their children beside them.
    let dir = solution();
    let root = dir.path();
    for step in [
        vec![
            "mashup",
            "new",
            "Acme.One_MU",
            "--template",
            "tab-content",
            "--rules",
            "legacy",
        ],
        vec![
            "mashup",
            "new",
            "Acme.Two_MU",
            "--template",
            "tab-content",
            "--rules",
            "legacy",
        ],
        vec![
            "mashup",
            "new",
            "Acme.Tabs_MU",
            "--rules",
            "legacy",
            "--template",
            "tabs",
            "--tab",
            "One=Acme.One_MU",
            "--tab",
            "Two=Acme.Two_MU",
        ],
        vec![
            "mashup",
            "add",
            "Acme.One_MU",
            "label",
            "--in",
            "body",
            "--label",
            "Inside",
        ],
    ] {
        let mut args = step.clone();
        args.push("--apply");
        twaco(root, &args);
    }
    check("seq-tabs", &dump(root), bless, &mut differing);
    assert!(
        differing.is_empty(),
        "{} fixture(s) differ; if the change is meant, run `TWACO_BLESS=1 cargo test --test \
         mashup_layout` and review the diff:\n{}",
        differing.len(),
        differing.join("\n")
    );
}

/// A fixture for a case that no longer exists is a stale promise nobody checks.
#[test]
fn every_fixture_belongs_to_a_case() {
    let mut known: Vec<String> = cases().into_iter().map(|(name, _)| name).collect();
    known.push("templates-view".into());
    known.push("seq-tabs".into());
    let Ok(entries) = std::fs::read_dir(fixture_path("x").parent().unwrap()) else {
        return;
    };
    for entry in entries {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        let stem = name.trim_end_matches(".txt").to_string();
        assert!(known.contains(&stem), "{name} belongs to no case");
    }
}
