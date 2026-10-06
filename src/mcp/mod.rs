//! `twaco mcp`: the library as a Model Context Protocol server on stdio.
//!
//! The second adapter beside the CLI, over the same `core` functions, so the two cannot drift.
//! Transport is newline-delimited JSON-RPC 2.0: one message per line on stdin, one response per
//! line on stdout, and nothing else on stdout. Logs go to stderr.
//!
//! **Response shape is the point**. Every tool returns a compact summary by
//! default, with `detail: true` for everything. Compact summaries avoid returning unnecessarily
//! large preflight results.
//!
//! **Server writes default to a dry run.** A tool whose blast radius is a server takes `dry_run`,
//! and it is true unless the caller says otherwise. That includes `call`: a service call is
//! opaque, twaco cannot tell whether it writes, and an agent must opt in to running one.

use crate::core::codes::{Coded, ErrorCode};
use crate::core::commands::{self, Mode};
use crate::core::config::Solution;
use crate::core::index::Confidence;
use crate::core::{
    adopt, backup, catalog, check, config_table, db, deploy, docs, entity_carry, entity_delete,
    export, extensions, guide, help, impact, imports, javadoc, logs, newblock, profile, push,
    relocate, rename, repo, retemplate, server, settings, status, types, unused, workspace,
};
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
mod content;
mod data;
mod entity;
mod info;
mod outputs;
mod refactor;
mod registry;
mod requests;
mod schema;
mod source;

#[cfg(test)]
mod tests;

/// Every tool's `tools/list` entry, as the newest protocol revision shows it.
pub fn tool_definitions() -> Vec<Value> {
    registry::definitions(LATEST)
}

#[cfg(test)]
pub(crate) fn legacy_schema(schema: &Value, arguments: &Value) -> Result<(), String> {
    schema::validate_arguments(schema, arguments)
}

/// Protocol revisions this server speaks. A client asking for one of these gets it; any other
/// gets [`LATEST`], and the client decides whether it can continue.
const PROTOCOL_VERSIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
const LATEST: &str = "2025-11-25";

const INSTRUCTIONS: &str = "twaco manages a ThingWorx solution kept in source control: entity XML with \
service scripts as sidecar files. Results are summaries by default; pass detail: true for \
everything. Tools that write to the ThingWorx server default to dry_run: true and report what \
they would do; pass dry_run: false to act. Use `catalog` for the repository's callable services, `types` for editor declarations and TypeScript checks, `logs` to read the server's logs, and \
`help_search`/`help_page` for the ThingWorx Platform help, `javadoc` for the Java API of objects scripts call and Resource service parameters, `guide` for twaco's workflow, the platform's verified-live quirks and the solution's own documents (search it before a live import, a hand-written binding or a configuration-table change), `repo` for file repositories, and \
`extensions` for extension packages. \
Start with `projects`, then `check`, then `status` with `all: true` (or one `entity`).";

/// Serve until stdin closes. `root` is where the solution is looked for, on every call, so an
/// edit to `twaco.toml` takes effect without a restart.
pub fn serve(root: &Path, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    let mut protocol = LATEST.to_string();
    let mut input = input;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if input.read_until(b'\n', &mut bytes)? == 0 {
            break;
        }
        // A line that is not UTF-8 is one bad message, answered as such; the server reads on.
        let response = match std::str::from_utf8(&bytes) {
            Err(error) => Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: not UTF-8: {error}"),
            )),
            Ok(line) if line.trim().is_empty() => None,
            Ok(line) => match serde_json::from_str::<Value>(line) {
                Ok(message) => handle(root, &message, &mut protocol),
                Err(error) => Some(error_response(
                    Value::Null,
                    -32700,
                    &format!("parse error: {error}"),
                )),
            },
        };
        if let Some(response) = response {
            writeln!(
                output,
                "{}",
                serde_json::to_string(&response).expect("JSON values serialise")
            )?;
            output.flush()?;
        }
    }
    Ok(())
}

/// One message in, at most one response out. A notification (no `id`) never gets a response.
fn handle(root: &Path, message: &Value, protocol: &mut String) -> Option<Value> {
    let invalid = |id: Value, why: &str| {
        Some(error_response(
            id,
            -32600,
            &format!("invalid request: {why}"),
        ))
    };
    let Some(object) = message.as_object() else {
        return invalid(
            Value::Null,
            "a message is one JSON object; batches are not supported",
        );
    };
    let id = object.get("id").cloned();
    if id
        .as_ref()
        .is_some_and(|id| !(id.is_string() || id.is_number()))
    {
        return invalid(Value::Null, "an id is a string or a number");
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // This server sends no requests, so a response from the client answers nothing of ours.
        if object.contains_key("result") || object.contains_key("error") {
            return None;
        }
        return invalid(id.unwrap_or(Value::Null), "no method");
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return invalid(id.unwrap_or(Value::Null), "jsonrpc must be \"2.0\"");
    }
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        // notifications/initialized, notifications/cancelled and the rest: nothing to answer.
        return None;
    };
    if !(params.is_null() || params.is_object()) {
        return Some(error_response(id, -32602, "params must be an object"));
    }
    let result = match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(LATEST);
            *protocol = if PROTOCOL_VERSIONS.contains(&asked) {
                asked.to_string()
            } else {
                LATEST.to_string()
            };
            Ok(json!({
                "protocolVersion": protocol,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "twaco", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": registry::definitions(protocol) })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(root, name, &arguments) {
                Some(outcome) => Ok(tool_result(outcome, protocol)),
                None => Err((-32602, format!("unknown tool {name:?}"))),
            }
        }
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A tool's JSON as MCP content. `structuredContent` exists from 2025-06-18; the text block
/// carries the same JSON for clients that predate it.
#[derive(Clone, Debug)]
pub(crate) struct ToolError {
    code: ErrorCode,
    message: String,
}

impl ToolError {
    pub(crate) fn coded<E: Coded + std::fmt::Display>(error: E) -> Self {
        Self {
            code: error.code(),
            message: error.to_string(),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidArguments,
            message: message.into(),
        }
    }

    fn io(error: std::io::Error) -> Self {
        Self::with(ErrorCode::IoError, error.to_string())
    }

    fn with(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self {
            code: ErrorCode::Unclassified,
            message,
        }
    }
}
impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

pub(crate) fn tool_result(outcome: Result<Value, ToolError>, protocol: &str) -> Value {
    let (value, is_error) = match outcome {
        Ok(value) => (value, false),
        Err(error) => (
            json!({ "error": error.message, "code": error.code.as_str() }),
            true,
        ),
    };
    let mut result = json!({
        "content": [{ "type": "text", "text": serde_json::to_string(&value).expect("JSON values serialise") }],
        "isError": is_error,
    });
    if protocol >= "2025-06-18" {
        result["structuredContent"] = value;
    }
    result
}

// ---- tool implementations -------------------------------------------------------------------

fn call_tool(root: &Path, name: &str, arguments: &Value) -> Option<Result<Value, ToolError>> {
    let started = Instant::now();
    let outcome = registry::call(root, name, arguments)?;
    Some(outcome.map(|mut value| {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "duration_ms".to_string(),
                json!(started.elapsed().as_millis() as u64),
            );
        }
        value
    }))
}

/// A text argument that may be left out, but that the tool cannot do without: left out or empty is
/// the same refusal.
pub(crate) fn required_text<'a>(
    value: &'a requests::common::Absent<String>,
    name: &str,
) -> Result<&'a str, ToolError> {
    nonempty(value.as_deref().unwrap_or_default(), name)
}

/// A text argument the tool cannot do without: an empty one is as missing as an absent one.
pub(crate) fn nonempty<'a>(value: &'a str, name: &str) -> Result<&'a str, ToolError> {
    Some(value)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError::invalid(format!("`{name}` is required")))
}

/// A client for the named server profile.
pub(crate) fn client(solution: &Solution, profile: &str) -> Result<server::Client, ToolError> {
    profile::load(&solution.root, profile)
        .map(server::Client::new)
        .map_err(ToolError::coded)
}

pub(crate) fn relative(solution: &Solution, path: &Path) -> String {
    let text = path
        .strip_prefix(&solution.root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/");
    // A project at the solution root is `.`, as twaco.toml spells it, not an empty string.
    let text = text.trim_start_matches("./").to_string();
    if text.is_empty() || text == "." {
        ".".to_string()
    } else {
        text
    }
}

fn projects(solution: &Solution) -> Result<Value, ToolError> {
    let order = solution.deploy_order().map_err(ToolError::coded)?;
    let found = workspace::discover(solution);
    let projects: Vec<Value> = order
        .iter()
        .map(|project| {
            json!({
                "name": project.name,
                "root": relative(solution, &solution.project_root(project)),
                "depends_on": project.depends_on,
                "entities": found.entities.iter().filter(|e| e.found_under == project.name).count(),
            })
        })
        .collect();
    Ok(json!({
        "solution": solution.solution.name,
        "projects": projects,
        "unreadable": found.unreadable,
    }))
}

pub(crate) fn add_notices(result: &mut Value, notices: &commands::Notices) {
    if !notices.is_empty() {
        result["notices"] = json!(notices.lines());
    }
}
