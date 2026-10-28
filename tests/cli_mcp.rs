//! CLI/MCP differential tests for local command facades.
//!
//! Each case starts with identical solutions, then compares the public CLI JSON report with the
//! JSON carried by an MCP `tools/call` response.  `.twaco/` is deliberately excluded: locks and
//! other local bookkeeping are not solution content.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

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

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_twaco"));
        command
            .args(args)
            .current_dir(self.root())
            .env_clear()
            .env("TWACO_NO_UPDATE_CHECK", "1");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "twaco {args:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    fn cli_json(&self, args: &[&str]) -> Invocation {
        let output = self.run(args);
        let success = output.status.success();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let value = serde_json::from_str(&stdout).unwrap_or_else(|_| {
            json!({
                "stdout": stdout,
                "stderr": String::from_utf8_lossy(&output.stderr),
            })
        });
        Invocation { success, value }
    }

    fn mcp(&self, tool: &str, arguments: Value) -> Invocation {
        let mut child = self
            .command(&["mcp"])
            .env("TWACO_ROOT", self.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let requests = [
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":arguments}}),
        ];
        let mut stdin = child.stdin.take().unwrap();
        for request in requests {
            writeln!(stdin, "{request}").unwrap();
        }
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "twaco mcp {tool}\nstderr={}",
            String::from_utf8_lossy(&output.stderr),
        );
        let response = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|response| response["id"] == 1)
            .unwrap_or_else(|| {
                panic!(
                    "twaco mcp {tool} gave no tools/call response\nstdout={}\nstderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                )
            });
        let result = response["result"].clone();
        let success = result["isError"] != true;
        let value = result["content"]
            .get(0)
            .and_then(|content| content["text"].as_str())
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or(result);
        Invocation { success, value }
    }

    /// Every file of the solution, with `/` paths. twaco's local state is not solution content.
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
                if relative == ".twaco" || relative == ".git" {
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

struct Invocation {
    success: bool,
    value: Value,
}

fn prepared_mashup() -> Scratch {
    let scratch = Scratch::new();
    scratch.ok(&["new", "project", "Acme", "--apply"]);
    scratch.ok(&["mashup", "new", "Acme.Orders_MU", "--apply"]);
    scratch
}

fn prepared_bundle() -> Scratch {
    let scratch = prepared_mashup();
    scratch.ok(&["bundle"]);
    scratch
}

fn normalise(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.replace('-', "_").to_ascii_lowercase(), normalise(value)))
                .collect::<Map<String, Value>>(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(normalise).collect()),
        Value::String(text) => Value::String(text.replace('\\', "/")),
        value => value.clone(),
    }
}

fn semantic(value: &Value, fields: &[&str]) -> Value {
    let mut normalised = normalise(value);
    if normalised.get("dry_run").is_none() {
        if let Some(applied) = normalised.get("applied").and_then(Value::as_bool) {
            normalised["dry_run"] = json!(!applied);
        }
    }
    Value::Object(
        fields
            .iter()
            .map(|field| {
                (
                    (*field).to_string(),
                    normalised.get(*field).cloned().unwrap_or(Value::Null),
                )
            })
            .collect(),
    )
}

fn assert_agree(
    label: &str,
    cli: Invocation,
    mcp: Invocation,
    fields: &[&str],
    cli_tree: BTreeMap<String, Vec<u8>>,
    mcp_tree: BTreeMap<String, Vec<u8>>,
) {
    assert_eq!(
        cli.success, mcp.success,
        "{label}: CLI={:?}, MCP={:?}",
        cli.value, mcp.value
    );
    assert_eq!(
        semantic(&cli.value, fields),
        semantic(&mcp.value, fields),
        "{label}: semantic result differs\nCLI={}\nMCP={}",
        cli.value,
        mcp.value,
    );
    assert_eq!(cli_tree, mcp_tree, "{label}: solution trees differ");
}

#[test]
fn mashup_add_plan_and_apply_match_mcp() {
    for (label, cli_args, mcp_arguments) in [
        (
            "plan",
            vec!["mashup", "add", "Acme.Orders_MU", "grid", "--json"],
            json!({"mashup":"Acme.Orders_MU","widget":"grid","dry_run":true}),
        ),
        (
            "apply",
            vec![
                "mashup",
                "add",
                "Acme.Orders_MU",
                "grid",
                "--apply",
                "--json",
            ],
            json!({"mashup":"Acme.Orders_MU","widget":"grid","dry_run":false}),
        ),
    ] {
        let cli_scratch = prepared_mashup();
        let mcp_scratch = prepared_mashup();
        let cli = cli_scratch.cli_json(&cli_args);
        let mcp = mcp_scratch.mcp("mashup_add", mcp_arguments);
        assert_agree(
            &format!("mashup add {label}"),
            cli,
            mcp,
            &[
                "applied",
                "dry_run",
                "mashup",
                "widget",
                "id",
                "containers",
                "wrapped",
                "files",
            ],
            cli_scratch.tree(),
            mcp_scratch.tree(),
        );
    }
}

/// BUG: `sync` has no CLI `--json` flag, so it cannot provide the structured counterpart of MCP.
#[test]
fn sync_check_matches_mcp() {
    sync_case(
        "check",
        &["sync", "--all", "--check", "--json"],
        json!({"all":true,"check":true}),
    );
}

/// BUG: `sync` has no CLI `--json` flag, so it cannot provide the structured counterpart of MCP.
#[test]
fn sync_apply_matches_mcp() {
    sync_case(
        "apply",
        &["sync", "--all", "--json"],
        json!({"all":true,"check":false}),
    );
}

fn sync_case(label: &str, cli_args: &[&str], mcp_arguments: Value) {
    let cli_scratch = prepared_mashup();
    let mcp_scratch = prepared_mashup();
    let cli = cli_scratch.cli_json(cli_args);
    let mcp = mcp_scratch.mcp("sync", mcp_arguments);
    assert_agree(
        &format!("sync {label}"),
        cli,
        mcp,
        &[
            "ok", "check", "checked", "changed", "failed", "changes", "errors",
        ],
        cli_scratch.tree(),
        mcp_scratch.tree(),
    );
}

/// BUG: `bundle` has no CLI `--json` flag, so its offline plan cannot be compared structurally.
#[test]
fn bundle_plan_matches_mcp() {
    let cli_scratch = prepared_bundle();
    let mcp_scratch = prepared_bundle();
    let cli = cli_scratch.cli_json(&["bundle", "--check", "--json"]);
    let mcp = mcp_scratch.mcp("bundle", json!({"dry_run":true}));
    assert_agree(
        "bundle plan",
        cli,
        mcp,
        &["ok", "state", "bundle", "entities", "files"],
        cli_scratch.tree(),
        mcp_scratch.tree(),
    );
}
