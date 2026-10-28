//! End-to-end laws for the destructive mashup authoring commands.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Output};

struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("twaco.toml"),
            "[[project]]\nname = \"Acme\"\n",
        )
        .unwrap();
        Self { dir }
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
        serde_json::from_str(&self.ok(args)).unwrap()
    }
    fn tree(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut pending = vec![self.root().to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let relative = path
                    .strip_prefix(self.root())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if relative == ".twaco" {
                    continue;
                }
                if path.is_dir() {
                    pending.push(path);
                } else {
                    out.insert(relative, std::fs::read(path).unwrap());
                }
            }
        }
        out
    }
}

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

#[test]
fn remove_leaf_plans_exact_files_applies_and_keeps_crlf() {
    let scratch = Scratch::new();
    scratch.ok(&["mashup", "new", "Acme.Orders_MU", "--apply"]);
    scratch.ok(&["mashup", "add", "Acme.Orders_MU", "label", "--apply"]);
    let entity = scratch.root().join("Mashups/Acme.Orders_MU.xml");
    if entity.exists() {
        let crlf = String::from_utf8(std::fs::read(&entity).unwrap())
            .unwrap()
            .replace("\r\n", "\n")
            .replace('\n', "\r\n");
        std::fs::write(&entity, crlf).unwrap();
    }
    let before = scratch.tree();
    let plan = scratch.json(&[
        "mashup",
        "remove",
        "Acme.Orders_MU",
        "ptcslabel-3",
        "--json",
    ]);
    let listed = plan["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect::<BTreeSet<_>>();
    scratch.ok(&[
        "mashup",
        "remove",
        "Acme.Orders_MU",
        "ptcslabel-3",
        "--apply",
    ]);
    let after = scratch.tree();
    assert_eq!(listed, changed(&before, &after));
    if entity.exists() {
        assert!(std::fs::read(&entity)
            .unwrap()
            .windows(2)
            .filter(|bytes| bytes[1] == b'\n')
            .all(|bytes| bytes[0] == b'\r'));
    }
}

#[test]
fn remove_unbind_and_off_are_safe_over_a_wired_mashup() {
    let scratch = Scratch::new();
    for command in [
        vec!["new", "project", "Acme"],
        vec![
            "new",
            "data-shape",
            "Acme.Rows_DS",
            "--field",
            "id:STRING:primary",
        ],
        vec!["new", "thing", "Acme.Store_T"],
        vec![
            "thing",
            "service",
            "add",
            "Acme.Store_T",
            "GetRows",
            "--returns",
            "INFOTABLE:Acme.Rows_DS",
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
            "on",
            "Acme.Orders_MU",
            "--event",
            "ui:ptcsbutton-4.Clicked",
            "--invoke",
            "data:Acme.Store_T.GetRows",
        ],
    ] {
        let mut applied = command;
        applied.push("--apply");
        scratch.ok(&applied);
    }
    let refusal = scratch.run(&["mashup", "remove", "Acme.Orders_MU", "ptcsgrid-3"]);
    assert!(!refusal.status.success());
    let unbind = scratch.json(&[
        "mashup",
        "unbind",
        "Acme.Orders_MU",
        "--from",
        "data:Acme.Store_T.GetRows",
        "--to",
        "ui:ptcsgrid-3.Data",
        "--apply",
        "--json",
    ]);
    assert!(unbind["applied"].as_bool().unwrap());
    scratch.ok(&[
        "mashup",
        "bind",
        "Acme.Orders_MU",
        "--from",
        "data:Acme.Store_T.GetRows",
        "--to",
        "ui:ptcsgrid-3.Data",
        "--apply",
    ]);
    let off = scratch.json(&[
        "mashup",
        "off",
        "Acme.Orders_MU",
        "--event",
        "ui:ptcsbutton-4.Clicked",
        "--invoke",
        "data:Acme.Store_T.GetRows",
        "--apply",
        "--json",
    ]);
    assert!(off["applied"].as_bool().unwrap());
    scratch.ok(&[
        "mashup",
        "on",
        "Acme.Orders_MU",
        "--event",
        "ui:ptcsbutton-4.Clicked",
        "--invoke",
        "data:Acme.Store_T.GetRows",
        "--apply",
    ]);
    let cascade = scratch.json(&[
        "mashup",
        "remove",
        "Acme.Orders_MU",
        "ptcsbutton-4",
        "--cascade",
        "--apply",
        "--json",
    ]);
    assert_eq!(cascade["events_removed"], 1);
    let second = scratch.json(&[
        "mashup",
        "remove",
        "Acme.Orders_MU",
        "ptcsbutton-4",
        "--cascade",
        "--apply",
        "--json",
    ]);
    assert!(second["removed"].as_array().unwrap().is_empty());
    let grid = scratch.json(&[
        "mashup",
        "remove",
        "Acme.Orders_MU",
        "ptcsgrid-3",
        "--cascade",
        "--apply",
        "--json",
    ]);
    assert_eq!(grid["bindings_removed"], 1);
}
