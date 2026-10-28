//! Scenarios: several commands in a row on one scratch solution, judged by the files they leave.
//!
//! Each law here holds across commands rather than inside one: a plan lists exactly the files
//! its apply changes, running a command again changes nothing, and round trips (sync, extract,
//! sync; rename there and back; adopting our own export) come back to where they started.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Output};

struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Scratch {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("twaco.toml"),
            "[[project]]\nname = \"Acme\"\n",
        )
        .unwrap();
        Scratch { dir }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_twaco"))
            .args(args)
            .current_dir(self.root())
            .env_clear()
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "twaco {args:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn json(&self, args: &[&str]) -> Value {
        let text = self.ok(args);
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("twaco {args:?}: {e}\n{text}"))
    }

    /// Every file of the solution, by path relative to the root with `/`; twaco's own state in
    /// `.twaco/` (lock, baselines, journals, generated types) is not the solution's content.
    fn tree(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut pending = vec![self.root().to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let relative = path
                    .strip_prefix(self.root())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if relative == ".twaco" || relative == ".git" {
                    continue;
                }
                if path.is_dir() {
                    pending.push(path);
                } else {
                    out.insert(relative, std::fs::read(&path).unwrap());
                }
            }
        }
        out
    }

    fn content(&self, mashup: &str) -> Value {
        let path = self
            .root()
            .join(format!("src/{mashup}/mashup/content.json"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }
}

/// The paths whose bytes differ, or that exist on one side only.
fn changed(
    before: &BTreeMap<String, Vec<u8>>,
    after: &BTreeMap<String, Vec<u8>>,
) -> BTreeSet<String> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

/// Every widget id in a mashup's content.
fn widget_ids(content: &Value) -> BTreeSet<String> {
    fn walk(widget: &Value, out: &mut BTreeSet<String>) {
        if let Some(id) = widget["Properties"]["Id"].as_str() {
            out.insert(id.to_string());
        }
        for child in widget["Widgets"].as_array().into_iter().flatten() {
            walk(child, out);
        }
    }
    let mut out = BTreeSet::new();
    walk(&content["UI"], &mut out);
    out
}

/// The commands that build a small solution, each with a plan that lists its files in `--json`.
fn steps() -> Vec<Vec<&'static str>> {
    vec![
        vec!["new", "project", "Acme"],
        vec![
            "new",
            "data-shape",
            "Acme.Rows_DS",
            "--field",
            "id:STRING:primary",
            "--field",
            "qty:INTEGER",
        ],
        vec!["new", "thing-shape", "Acme.Shape_TS"],
        vec![
            "new",
            "thing-template",
            "Acme.Base_TT",
            "--shape",
            "Acme.Shape_TS",
        ],
        vec!["new", "thing", "Acme.Store_T", "--template", "Acme.Base_TT"],
        vec![
            "thing",
            "service",
            "add",
            "Acme.Store_T",
            "GetRows",
            "--param",
            "limit:INTEGER",
            "--returns",
            "INFOTABLE:Acme.Rows_DS",
        ],
        vec![
            "thing",
            "property",
            "add",
            "Acme.Store_T",
            "Title:STRING",
            "--default",
            "Orders",
        ],
        vec!["mashup", "new", "Acme.Orders_MU"],
        vec!["mashup", "add", "Acme.Orders_MU", "grid"],
        vec![
            "mashup",
            "add",
            "Acme.Orders_MU",
            "button",
            "--above",
            "ptcsgrid-3",
            "--label",
            "Reload",
        ],
        vec![
            "mashup",
            "add",
            "Acme.Orders_MU",
            "label",
            "--right-of",
            "ptcsbutton-4",
            "--label",
            "Total",
        ],
        vec![
            "mashup",
            "param",
            "add",
            "Acme.Orders_MU",
            "Limit",
            "--type",
            "INTEGER",
            "--direction",
            "IN",
        ],
        vec![
            "mashup",
            "bind",
            "Acme.Orders_MU",
            "--from",
            "data:Acme.Store_T.GetRows",
            "--to",
            "ui:ptcsgrid-3.Data",
        ],
        vec![
            "mashup",
            "bind",
            "Acme.Orders_MU",
            "--from",
            "mashup:Limit",
            "--to",
            "data:Acme.Store_T.GetRows.limit",
        ],
        vec![
            "mashup",
            "on",
            "Acme.Orders_MU",
            "--event",
            "mashup:Loaded",
            "--invoke",
            "data:Acme.Store_T.GetRows",
        ],
        vec![
            "mashup",
            "on",
            "Acme.Orders_MU",
            "--event",
            "ui:ptcsbutton-4.Clicked",
            "--invoke",
            "data:Acme.Store_T.GetRows",
        ],
        vec![
            "mashup",
            "recipe",
            "columns",
            "Acme.Orders_MU",
            "ptcsgrid-3",
            "--column",
            "id:ID",
            "--column",
            "qty:Quantity:80",
        ],
    ]
}

fn with(args: &[&'static str], extra: &[&'static str]) -> Vec<&'static str> {
    args.iter().chain(extra).copied().collect()
}

/// Build the solution step by step, checking each plan against what its apply does.
fn build(scratch: &Scratch) {
    for step in steps() {
        let untouched = scratch.tree();
        let plan = scratch.json(&with(&step, &["--json"]));
        assert_eq!(scratch.tree(), untouched, "{step:?}: the plan wrote");
        assert_eq!(plan["applied"], false, "{step:?}: a plan applied");
        let planned: BTreeSet<String> = plan["files"]
            .as_array()
            .unwrap_or_else(|| panic!("{step:?}: no files in {plan}"))
            .iter()
            .map(|file| file.as_str().unwrap().replace('\\', "/"))
            .collect();
        let mashup_before =
            (step[0] == "mashup" && step[1] == "add").then(|| scratch.content(step[2]));
        let before = scratch.tree();
        let applied = scratch.json(&with(&step, &["--apply", "--json"]));
        assert_eq!(applied["applied"], true, "{step:?}");
        let after = scratch.tree();
        assert_eq!(
            changed(&before, &after),
            planned,
            "{step:?}: the plan listed other files than the apply changed"
        );
        if let Some(before) = mashup_before {
            // What mashup add says it placed is what appeared: the widget and its new containers.
            let created: BTreeSet<String> = widget_ids(&scratch.content(step[2]))
                .difference(&widget_ids(&before))
                .cloned()
                .collect();
            let mut reported: BTreeSet<String> = applied["containers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|id| id.as_str().unwrap().to_string())
                .collect();
            reported.insert(applied["id"].as_str().unwrap().to_string());
            assert_eq!(
                created, reported,
                "{step:?}: mashup add reported other widgets than it made"
            );
        }
    }
}

#[test]
fn every_plan_lists_exactly_the_files_its_apply_changes() {
    let scratch = Scratch::new();
    build(&scratch);
    scratch.ok(&["check"]);
}

#[test]
fn running_a_command_again_changes_nothing() {
    let scratch = Scratch::new();
    build(&scratch);
    // Adding a widget is meant to add another each time; every other step makes one thing.
    for step in steps()
        .into_iter()
        .filter(|step| step[..2] != ["mashup", "add"])
    {
        let before = scratch.tree();
        // A second run either refuses (the thing exists, the binding is there) or does nothing.
        let output = scratch.run(&with(&step, &["--apply"]));
        assert_eq!(
            changed(&before, &scratch.tree()),
            BTreeSet::new(),
            "{step:?} changed files when run again (exit {:?})\nstdout={}\nstderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn sync_extract_sync_comes_back_to_the_same_files() {
    let scratch = Scratch::new();
    build(&scratch);
    scratch.ok(&["sync", "--all"]);
    let synced = scratch.tree();
    scratch.ok(&["extract", "--all"]);
    assert_eq!(
        changed(&synced, &scratch.tree()),
        BTreeSet::new(),
        "extract after sync"
    );
    scratch.ok(&["sync", "--all"]);
    assert_eq!(
        changed(&synced, &scratch.tree()),
        BTreeSet::new(),
        "sync after extract"
    );
    scratch.ok(&["sync", "--all", "--check"]);
}

#[test]
fn renaming_there_and_back_restores_every_file() {
    let scratch = Scratch::new();
    build(&scratch);
    let start = scratch.tree();
    scratch.ok(&[
        "rename",
        "entity",
        "Acme.Store_T",
        "Acme.Shop_T",
        "--text",
        "--apply",
    ]);
    assert_ne!(changed(&start, &scratch.tree()), BTreeSet::new());
    scratch.ok(&[
        "rename",
        "entity",
        "Acme.Shop_T",
        "Acme.Store_T",
        "--text",
        "--apply",
    ]);
    assert_eq!(changed(&start, &scratch.tree()), BTreeSet::new());
    scratch.ok(&["check"]);
}

#[test]
fn adopting_our_own_files_finds_nothing_to_adopt() {
    let scratch = Scratch::new();
    build(&scratch);
    scratch.ok(&["sync", "--all"]);
    let start = scratch.tree();
    for entity in [
        "Mashups/Acme.Orders_MU.xml",
        "Things/Acme.Store_T.xml",
        "DataShapes/Acme.Rows_DS.xml",
    ] {
        let report = scratch.json(&["adopt", entity, "--json"]);
        assert_eq!(
            report["changed"],
            serde_json::json!({}),
            "{entity}: {report}"
        );
        assert_eq!(report["identical"], 1, "{entity}: {report}");
        scratch.ok(&["adopt", entity, "--apply"]);
        assert_eq!(
            changed(&start, &scratch.tree()),
            BTreeSet::new(),
            "{entity}"
        );
    }
}

/// Every text file of the solution rewritten with CRLF line endings, as a Windows checkout with
/// `core.autocrlf` leaves it.
fn to_crlf(scratch: &Scratch) {
    for (path, bytes) in scratch.tree() {
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let crlf = text.replace("\r\n", "\n").replace('\n', "\r\n");
        std::fs::write(scratch.root().join(path), crlf).unwrap();
    }
}

/// The files whose lines do not all end the same way.
fn mixed(scratch: &Scratch) -> Vec<String> {
    scratch
        .tree()
        .into_iter()
        .filter(|(_, bytes)| {
            let crlf = bytes.windows(2).filter(|pair| pair == b"\r\n").count();
            let lf = bytes.iter().filter(|&&byte| byte == b'\n').count();
            let cr = bytes.iter().filter(|&&byte| byte == b'\r').count();
            (crlf > 0 && crlf < lf) || cr != crlf
        })
        .map(|(path, _)| path)
        .collect()
}

/// More edits, of every kind of command, after the first build.
fn more_steps() -> Vec<Vec<&'static str>> {
    vec![
        vec![
            "thing",
            "service",
            "add",
            "Acme.Store_T",
            "Count",
            "--returns",
            "INTEGER",
            "--then",
            "result = 1;",
        ],
        vec![
            "thing",
            "property",
            "set",
            "Acme.Store_T",
            "Title:STRING",
            "--default",
            "All orders",
        ],
        vec![
            "thing",
            "property",
            "add",
            "Acme.Store_T",
            "Limit:INTEGER",
            "--default",
            "10",
            "--persistent",
        ],
        vec!["thing", "property", "remove", "Acme.Store_T", "Limit"],
        vec!["thing", "service", "remove", "Acme.Store_T", "Count"],
        vec!["new", "thing-shape", "Acme.Extra_TS"],
        vec!["thing", "shape", "add", "Acme.Store_T", "Acme.Extra_TS"],
        vec!["thing", "shape", "remove", "Acme.Store_T", "Acme.Extra_TS"],
        vec![
            "mashup",
            "param",
            "add",
            "Acme.Orders_MU",
            "Alpha",
            "--type",
            "STRING",
            "--direction",
            "IN",
        ],
        vec![
            "mashup",
            "param",
            "set",
            "Acme.Orders_MU",
            "Alpha",
            "--type",
            "INTEGER",
        ],
        vec!["mashup", "param", "remove", "Acme.Orders_MU", "Alpha"],
        vec![
            "mashup",
            "add",
            "Acme.Orders_MU",
            "dropdown",
            "--below",
            "ptcsgrid-3",
        ],
        vec![
            "mashup",
            "recipe",
            "message",
            "Acme.Orders_MU",
            "--after",
            "data:Acme.Store_T.GetRows",
            "--say",
            "Loaded",
        ],
        vec!["mashup", "new", "Acme.Child_MU"],
        vec![
            "mashup",
            "add",
            "Acme.Child_MU",
            "button",
            "--label",
            "Save",
        ],
        vec![
            "mashup",
            "event",
            "add",
            "Acme.Child_MU",
            "Saved",
            "--when",
            "ui:ptcsbutton-3.Clicked",
        ],
        vec![
            "mashup",
            "add-contained",
            "Acme.Orders_MU",
            "Acme.Child_MU",
            "--below",
            "ptcsgrid-3",
        ],
        vec![
            "mashup",
            "recipe",
            "popup",
            "Acme.Orders_MU",
            "ptcsbutton-4",
            "--mashup",
            "Acme.Child_MU",
            "--window",
            "modal-popup",
        ],
        vec!["localization", "set", "Acme.Hello", "--value", "Hello"],
        vec![
            "new",
            "data-table",
            "Acme.Rows_DT",
            "--data-shape",
            "Acme.Rows_DS",
        ],
    ]
}

#[test]
fn a_crlf_solution_stays_whole_through_every_kind_of_edit() {
    let scratch = Scratch::new();
    build(&scratch);
    scratch.ok(&["sync", "--all"]);
    to_crlf(&scratch);
    assert_eq!(mixed(&scratch), Vec::<String>::new());
    scratch.ok(&["sync", "--all"]);
    assert_eq!(
        mixed(&scratch),
        Vec::<String>::new(),
        "sync of a CRLF checkout"
    );
    for step in more_steps() {
        let output = scratch.run(&with(&step, &["--apply"]));
        assert!(
            output.status.success(),
            "twaco {step:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(mixed(&scratch), Vec::<String>::new(), "after {step:?}");
    }
    scratch.ok(&["sync", "--all"]);
    assert_eq!(mixed(&scratch), Vec::<String>::new(), "final sync");
    let check = scratch.run(&["check", "--detail"]);
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}

#[test]
fn every_kind_of_edit_plans_what_it_applies() {
    let scratch = Scratch::new();
    build(&scratch);
    for step in more_steps() {
        let before = scratch.tree();
        let plan = scratch.json(&with(&step, &["--json"]));
        assert_eq!(scratch.tree(), before, "{step:?}: the plan wrote");
        let planned: BTreeSet<String> = plan["files"]
            .as_array()
            .unwrap_or_else(|| panic!("{step:?}: the --json plan lists no files: {plan}"))
            .iter()
            // A path, or an object with one (localization's files carry counts too).
            .filter_map(|file| file.as_str().or_else(|| file["path"].as_str()))
            .map(|f| f.replace('\\', "/"))
            .collect();
        let output = scratch.run(&with(&step, &["--apply"]));
        assert!(
            output.status.success(),
            "twaco {step:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(changed(&before, &scratch.tree()), planned, "{step:?}");
    }
}

/// The `--json` forms of sync and bundle keep the exit codes of their text forms: 1 when the
/// files are behind (drift), 2 for an error.
#[test]
fn json_reports_keep_the_drift_and_error_exit_codes() {
    let scratch = Scratch::new();
    build(&scratch);
    scratch.ok(&["sync", "--all"]);
    let script = scratch
        .root()
        .join("src/Acme.Store_T/services/GetRows/script.js");
    std::fs::write(&script, "result = 1;\n").unwrap();
    let drift = scratch.run(&["sync", "--all", "--check", "--json"]);
    assert_eq!(drift.status.code(), Some(1), "{drift:?}");
    let report: Value = serde_json::from_slice(&drift.stdout).unwrap();
    assert_eq!(report["changed"], 1, "{report}");
    let missing = scratch.run(&["bundle", "--check", "--json"]);
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    let report: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(report["state"], "missing", "{report}");
    let error = scratch.run(&["sync", "Acme.Nope", "--json"]);
    assert_eq!(error.status.code(), Some(2), "{error:?}");
}

/// A command that accepts `--json` prints JSON: an agent asked for it and parses what comes.
#[test]
fn every_command_that_accepts_json_prints_json() {
    let scratch = Scratch::new();
    build(&scratch);
    scratch.ok(&["sync", "--all"]);
    let commands: Vec<Vec<&str>> = vec![
        vec!["projects"],
        vec!["catalog"],
        vec!["catalog", "Acme.Store_T.GetRows"],
        vec!["impact", "Things/Acme.Store_T"],
        vec!["trace", "Acme.Store_T.GetRows"],
        vec!["unused"],
        vec!["unused", "--fields"],
        vec!["docs"],
        vec!["widgets", "grid"],
        vec!["mashup", "templates"],
        vec!["mashup", "validate", "--all"],
        vec!["mashup", "describe", "Acme.Orders_MU"],
        vec!["handoff", "list"],
        vec!["block", "list"],
        vec!["search", "Acme"],
        vec!["check"],
        vec!["sync", "--all", "--check"],
        vec!["fmt", "--check"],
        vec!["types", "--check"],
        vec!["localization", "status"],
        vec!["localization", "set", "Acme.Hello", "--value", "Hi"],
        vec!["localization", "new", "de"],
        vec!["localization", "remove", "Acme.Hello"],
        vec!["rename", "entity", "Acme.Store_T", "Acme.Shop_T"],
        vec!["rename", "service", "Acme.Store_T", "GetRows", "Rows"],
        vec!["rename", "field", "Acme.Rows_DS", "qty", "quantity"],
        vec!["copy", "service", "Acme.Store_T", "Acme.Base_TT", "GetRows"],
        vec!["retemplate", "Acme.Store_T", "--to", "GenericThing"],
        vec!["reproject", "Things/Acme.Store_T", "--to", "Acme"],
        vec!["adopt", "Mashups/Acme.Orders_MU.xml"],
        vec!["bundle"],
        vec!["package"],
        vec!["requirements", "init"],
        vec!["permissions", "init"],
    ];
    let mut wrong = Vec::new();
    for command in commands {
        let output = scratch.run(&with_json(&command));
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("unexpected argument '--json'") {
            continue;
        }
        // An error is text on stderr, `--json` or not; what succeeds must be JSON.
        if output.status.success() && serde_json::from_slice::<Value>(&output.stdout).is_err() {
            wrong.push(format!(
                "{command:?} (exit {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or_else(|| stderr.lines().next().unwrap_or(""))
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "accepted --json but printed no JSON:\n{}",
        wrong.join("\n")
    );
}

fn with_json<'a>(command: &[&'a str]) -> Vec<&'a str> {
    command.iter().copied().chain(["--json"]).collect()
}
