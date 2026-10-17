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
use crate::core::entity::{carry, delete};
use crate::core::index::Confidence;
use crate::core::progress::{self as core_progress, Progress};
use crate::core::{
    adopt, backup, catalog, check, config_table, db, deploy, docs, export, extensions, guide, help,
    impact, imports, javadoc, logs, newblock, profile, push, relocate, rename, repo, retemplate,
    server, settings, status, types, unused, workspace,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
mod content;
pub use content::localization_status_json;
mod data;
mod entity;
mod info;
mod outputs;
mod progress;
mod refactor;
mod registry;
pub use registry::Selection;
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

/// A protocol revision this server speaks, oldest first, so a feature's arrival is a comparison.
/// A client asking for one of these gets it; any other gets [`LATEST`], and the client decides
/// whether it can continue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Protocol {
    V2024_11_05,
    V2025_03_26,
    V2025_06_18,
    V2025_11_25,
}

const LATEST: Protocol = Protocol::V2025_11_25;

impl Protocol {
    const ALL: [Self; 4] = [
        Self::V2024_11_05,
        Self::V2025_03_26,
        Self::V2025_06_18,
        Self::V2025_11_25,
    ];

    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|protocol| protocol.as_str() == text)
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::V2024_11_05 => "2024-11-05",
            Self::V2025_03_26 => "2025-03-26",
            Self::V2025_06_18 => "2025-06-18",
            Self::V2025_11_25 => "2025-11-25",
        }
    }

    /// `structuredContent` in results and `outputSchema` in definitions arrived in 2025-06-18.
    pub(crate) fn has_structured_output(self) -> bool {
        self >= Self::V2025_06_18
    }
}

const INSTRUCTIONS: &str = "twaco manages a ThingWorx solution kept in source control: entity XML with \
service scripts as sidecar files. Results are summaries by default; pass detail: true for \
everything. Tools that write to the ThingWorx server default to dry_run: true and report what \
they would do; pass dry_run: false to act. Use `catalog` for the repository's callable services, `types` for editor declarations and TypeScript checks, `logs` to read the server's logs, and \
`help_search`/`help_page` for the ThingWorx Platform help, `javadoc` for the Java API of objects scripts call and Resource service parameters, `guide` for twaco's workflow, the platform's verified-live quirks and the solution's own documents (search it before a live import, a hand-written binding or a configuration-table change), `repo` for file repositories, and \
`extensions` for extension packages. \
Start with `projects`, then `check`, then `status` with `all: true` (or one `entity`).";

/// Serve every tool until stdin closes. See [`serve_tools`].
#[cfg(test)]
pub(crate) fn serve(
    root: &Path,
    input: impl BufRead,
    output: impl Write + Send,
) -> std::io::Result<()> {
    serve_tools(root, &Selection::ALL, input, output)
}

/// Serve the selected tools until stdin closes. `root` is where the solution is looked for, on
/// every call, so an edit to `twaco.toml` takes effect without a restart.
///
/// Tool calls run one at a time on a worker, in the order they arrived, as they always have: two
/// at once would only meet at the workspace lock, which refuses rather than waits. This thread
/// keeps reading meanwhile, so `ping`, `tools/list` and `notifications/cancelled` are answered
/// while a long call runs. A cancelled call that has not started never starts; one that has
/// runs to its end, because stopping a server write halfway leaves the server partly changed,
/// but sends no further progress and no response, as the protocol asks.
///
/// Progress notifications and responses share `output` under one lock, so each is a whole line.
pub fn serve_tools(
    root: &Path,
    selection: &Selection,
    input: impl BufRead,
    output: impl Write + Send,
) -> std::io::Result<()> {
    let output = Mutex::new(output);
    let sink: &progress::Sink<'_> = &output;
    let pending = Pending::default();
    std::thread::scope(|scope| {
        let (queue, calls) = std::sync::mpsc::channel::<Call>();
        let worker = scope.spawn(|| -> std::io::Result<()> {
            for call in calls {
                if let Some(response) = run_call(root, selection, &call, sink) {
                    write_line(sink, &response)?;
                }
                pending.finish(&call.id);
            }
            Ok(())
        });
        let read = read_messages(selection, input, sink, &pending, &queue);
        // Closing the queue lets the worker finish what is queued, then stop.
        drop(queue);
        let worked = worker
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        read.and(worked)
    })
}

fn read_messages(
    selection: &Selection,
    mut input: impl BufRead,
    sink: &progress::Sink<'_>,
    pending: &Pending,
    queue: &std::sync::mpsc::Sender<Call>,
) -> std::io::Result<()> {
    let mut protocol = LATEST;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if input.read_until(b'\n', &mut bytes)? == 0 {
            return Ok(());
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
                Ok(message) => match handle(selection, &message, &mut protocol, pending) {
                    Handled::Respond(response) => response,
                    Handled::Queue(call) => {
                        // The worker stops early only when it cannot write; joining it reports why.
                        if queue.send(call).is_err() {
                            return Ok(());
                        }
                        None
                    }
                },
                Err(error) => Some(error_response(
                    Value::Null,
                    -32700,
                    &format!("parse error: {error}"),
                )),
            },
        };
        if let Some(response) = response {
            write_line(sink, &response)?;
        }
    }
}

fn write_line(sink: &progress::Sink<'_>, message: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_string(message).expect("JSON values serialise");
    line.push('\n');
    let mut output = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    output.write_all(line.as_bytes())?;
    output.flush()
}

/// What the reader does with one message: answer it now, or queue a tool call for the worker.
enum Handled {
    Respond(Option<Value>),
    Queue(Call),
}

/// A `tools/call` waiting for, or running on, the worker.
struct Call {
    id: Value,
    params: Value,
    protocol: Protocol,
    cancelled: Arc<AtomicBool>,
    /// Log fields the reader already made safe: see [`handle`].
    logged_id: String,
    tool: &'static str,
    dry_run: Option<bool>,
}

/// The calls queued or running, by request id, so a cancellation finds its call.
#[derive(Default)]
struct Pending(Mutex<HashMap<String, Arc<AtomicBool>>>);

impl Pending {
    fn calls(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<AtomicBool>>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn start(&self, id: &Value) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.calls().insert(id.to_string(), Arc::clone(&flag));
        flag
    }

    /// Mark the call with this id cancelled. An id that is not pending (finished, unknown, or
    /// never a call) is ignored, as the protocol allows.
    fn cancel(&self, id: &Value) -> bool {
        match self.calls().get(&id.to_string()) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    fn finish(&self, id: &Value) {
        self.calls().remove(&id.to_string());
    }
}

/// Run one queued call on the worker. `None` when it was cancelled: no response is sent.
fn run_call(
    root: &Path,
    selection: &Selection,
    call: &Call,
    sink: &progress::Sink<'_>,
) -> Option<Value> {
    let span = tracing::info_span!(
        "mcp",
        method = "tools/call",
        id = %call.logged_id,
        tool = call.tool,
        dry_run = call.dry_run
    );
    let _entered = span.enter();
    if call.cancelled.load(Ordering::SeqCst) {
        tracing::info!("cancelled before it started");
        return None;
    }
    let name = call
        .params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let arguments = call
        .params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    // Only a call that carries a token reports progress.
    let notifier = progress::token_of(&call.params)
        .map(|token| progress::Notifier::new(sink, token).until(Arc::clone(&call.cancelled)));
    let reporter: &dyn Progress = match &notifier {
        Some(notifier) => notifier,
        None => &core_progress::NONE,
    };
    let response = match call_tool(root, selection, name, &arguments, reporter) {
        Some(outcome) => {
            if let Err(error) = &outcome {
                tracing::info!(code = error.code.as_str(), "tool failed");
            }
            json!({ "jsonrpc": "2.0", "id": call.id, "result": tool_result(outcome, call.protocol) })
        }
        None => error_response(call.id.clone(), -32602, &format!("unknown tool {name:?}")),
    };
    if call.cancelled.load(Ordering::SeqCst) {
        tracing::info!("cancelled while it ran; its response is dropped");
        return None;
    }
    Some(response)
}

/// One message in: at most one response now, or a call for the worker. A notification (no `id`)
/// never gets a response.
fn handle(
    selection: &Selection,
    message: &Value,
    protocol: &mut Protocol,
    pending: &Pending,
) -> Handled {
    let invalid = |id: Value, why: &str| {
        Handled::Respond(Some(error_response(
            id,
            -32600,
            &format!("invalid request: {why}"),
        )))
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
            return Handled::Respond(None);
        }
        return invalid(id.unwrap_or(Value::Null), "no method");
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return invalid(id.unwrap_or(Value::Null), "jsonrpc must be \"2.0\"");
    }
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        if method == "notifications/cancelled" {
            if let Some(request) = params.get("requestId") {
                let found = pending.cancel(request);
                tracing::info!(found, "cancellation received");
            }
        }
        // notifications/initialized and the rest: nothing to answer.
        return Handled::Respond(None);
    };
    if !(params.is_null() || params.is_object()) {
        return Handled::Respond(Some(error_response(id, -32602, "params must be an object")));
    }
    // Fields never include `arguments`: a tool's parameters can hold anything, secrets included.
    // The method and the tool name come from the caller too, so only names this server knows
    // reach a log.
    let logged_method = match method {
        "initialize" | "ping" | "tools/list" | "tools/call" => method,
        _ => "<unknown>",
    };
    let tool = match params.get("name").and_then(Value::as_str) {
        None => "",
        Some(name) => registry::registered_name(name).unwrap_or("<unknown>"),
    };
    let dry_run = params
        .get("arguments")
        .and_then(|arguments| arguments.get("dry_run"))
        .and_then(Value::as_bool);
    // A string id comes from the caller and can hold anything. Log numbers only.
    let logged_id = match &id {
        Value::Number(number) => number.to_string(),
        _ => "<string>".to_string(),
    };
    let span = tracing::info_span!("mcp", method = logged_method, id = %logged_id, tool, dry_run);
    let _entered = span.enter();
    tracing::info!("message received");
    let result = match method {
        "initialize" => {
            *protocol = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .and_then(Protocol::parse)
                .unwrap_or(LATEST);
            Ok(json!({
                "protocolVersion": protocol.as_str(),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "twaco", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": registry::selected_definitions(*protocol, selection)
        })),
        "tools/call" => {
            let cancelled = pending.start(&id);
            return Handled::Queue(Call {
                id,
                params,
                protocol: *protocol,
                cancelled,
                logged_id,
                tool,
                dry_run,
            });
        }
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Handled::Respond(Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    }))
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    tracing::info!(code, "error response");
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A tool's failure. Every conversion names its code: there is no `From<String>`, so a `?` on
/// a string error does not compile rather than silently reporting `unclassified`.
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

/// A tool's JSON as MCP content. `structuredContent` exists from 2025-06-18; the text block
/// carries the same JSON for clients that predate it.
pub(crate) fn tool_result(outcome: Result<Value, ToolError>, protocol: Protocol) -> Value {
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
    if protocol.has_structured_output() {
        result["structuredContent"] = value;
    }
    result
}

// ---- tool implementations -------------------------------------------------------------------

fn call_tool(
    root: &Path,
    selection: &Selection,
    name: &str,
    arguments: &Value,
    progress: &dyn Progress,
) -> Option<Result<Value, ToolError>> {
    let started = Instant::now();
    let outcome = registry::call(root, selection, name, arguments, progress)?;
    Some(outcome.map(|mut value| {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "duration_ms".to_string(),
                json!(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
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

/// A file to write, given relative to the solution: a plain path whose nearest existing folder
/// is really inside the solution, and not an existing file unless overwriting.
pub(crate) fn out_path(
    solution: &Solution,
    relative: &str,
    overwrite: bool,
) -> Result<std::path::PathBuf, ToolError> {
    // Only plain names: `\\x` or `C:x` is not absolute to Rust on Windows, yet joins outside.
    let plain = std::path::Path::new(relative)
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !plain || relative.trim().is_empty() {
        return Err(ToolError::invalid(format!(
            "{relative} must be a plain path inside the solution"
        )));
    }
    let out = solution.root.join(relative);
    // And no link along the way may lead out: the nearest folder that exists must be inside.
    let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
    let existing = out
        .ancestors()
        .skip(1)
        .find(|a| a.exists())
        .unwrap_or(&solution.root);
    let real = std::fs::canonicalize(existing)
        .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{}: {e}", existing.display())))?;
    if !real.starts_with(&root) {
        return Err(ToolError::invalid(format!(
            "{relative} leads outside the solution"
        )));
    }
    if out.exists() && !overwrite {
        return Err(ToolError::with(
            ErrorCode::AlreadyExists,
            format!("{relative} exists; pass overwrite: true to replace it"),
        ));
    }
    Ok(out)
}

/// A file to read, given relative to the solution: it must exist and really be inside it, links
/// resolved, so an absolute path or `..` cannot reach a file elsewhere on the machine.
pub(crate) fn in_path(solution: &Solution, relative: &str) -> Result<PathBuf, ToolError> {
    let real = std::fs::canonicalize(solution.root.join(relative))
        .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{relative}: {e}")))?;
    let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
    if !real.starts_with(&root) {
        return Err(ToolError::invalid(format!(
            "{relative} is outside the solution"
        )));
    }
    Ok(real)
}

/// A client for the named server profile.
pub(crate) fn client(solution: &Solution, profile: &str) -> Result<server::Client, ToolError> {
    profile::load(&solution.root, profile)
        .map(server::Client::new)
        .map_err(ToolError::coded)
}

/// The key a writing tool sets to `true` in its result: `plan` for a dry run, `applied` once
/// the change is written.
pub(crate) fn outcome_key(dry_run: bool) -> &'static str {
    if dry_run {
        "plan"
    } else {
        "applied"
    }
}

pub(crate) fn relative(solution: &Solution, path: &Path) -> String {
    let text = solution.relative(path);
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
