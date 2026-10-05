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
    adopt, backup, baseline, catalog, check, config_table, datatable_copy, db, deploy, docs,
    entity_carry, entity_delete, export, extensions, guide, help, impact, imports, javadoc, lock,
    logs, newblock, profile, push, relocate, rename, repo, retemplate, server, settings, status,
    types, unused, workspace,
};
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match tool_definitions().into_iter().find(|t| t["name"] == name) {
                None => Err((-32602, format!("unknown tool {name:?}"))),
                Some(definition) => {
                    match validate_arguments(&definition["inputSchema"], &arguments) {
                        // A tool error, not a protocol one, so the agent sees it and can correct it.
                        Err(why) => Ok(tool_result(
                            Err(ToolError::invalid(format!("{why}; nothing was done"))),
                            protocol,
                        )),
                        Ok(()) => match call_tool(root, name, &arguments) {
                            Some(outcome) => Ok(tool_result(outcome, protocol)),
                            None => Err((-32602, format!("unknown tool {name:?}"))),
                        },
                    }
                }
            }
        }
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    })
}

/// Hold the arguments to the tool's own schema. The tools read an argument of the wrong type as
/// absent, and absent means the default, which for `check`, `entity` or `only` is the wider
/// action: `check: "true"` would write, `only: "X"` would deploy everything. So a name the
/// schema does not declare, a value of the wrong type or outside its enum, or a missing
/// required argument is refused before the tool runs.
fn validate_arguments(schema: &Value, arguments: &Value) -> Result<(), String> {
    if !arguments.is_object() {
        return Err("`arguments` must be a JSON object".to_string());
    }
    validate_value(schema, arguments, "")
}

/// Validate one value against the small JSON Schema subset advertised by MCP tools.
fn validate_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let fits = match schema["type"].as_str() {
        Some("string") => value.is_string(),
        Some("boolean") => value.is_boolean(),
        Some("integer") => value
            .as_u64()
            .is_some_and(|n| schema["minimum"].as_u64().is_none_or(|min| n >= min)),
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        _ => true,
    };
    if !fits {
        let wanted = match schema["type"].as_str() {
            Some("array") if array_of_strings(schema) => "an array of strings".to_string(),
            Some("array") => "an array".to_string(),
            Some("integer") => format!(
                "an integer of at least {}",
                schema["minimum"].as_u64().unwrap_or(0)
            ),
            Some(other) => format!("a {other}"),
            None => "something else".to_string(),
        };
        return Err(format!("`{path}` must be {wanted}, not {value}"));
    }
    if let Some(allowed) = schema["enum"].as_array() {
        if !allowed.contains(value) {
            return Err(format!(
                "`{path}` must be one of {}, not {value}",
                Value::Array(allowed.clone())
            ));
        }
    }
    if let Some(given) = value.as_object() {
        let empty = Map::new();
        let properties = schema["properties"].as_object().unwrap_or(&empty);
        for name in schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !given.contains_key(name) {
                let path = property_path(path, name);
                return Err(format!("`{path}` is required"));
            }
        }
        for (name, value) in given {
            let name_path = property_path(path, name);
            match properties.get(name) {
                Some(property) => validate_value(property, value, &name_path)?,
                None if schema["additionalProperties"] == false => {
                    if path.is_empty() {
                        let mut known: Vec<&String> = properties.keys().collect();
                        known.sort();
                        let known: Vec<&str> = known.into_iter().map(String::as_str).collect();
                        return Err(format!(
                            "this tool takes no argument `{name}` (it takes: {})",
                            known.join(", ")
                        ));
                    }
                    return Err(format!("`{name_path}` is not allowed"));
                }
                None if schema["additionalProperties"].is_object() => {
                    validate_value(&schema["additionalProperties"], value, &name_path)?
                }
                None => {}
            }
        }
    }
    if let Some(items) = value.as_array() {
        if schema["items"].is_object() {
            for (index, value) in items.iter().enumerate() {
                validate_value(&schema["items"], value, &format!("{path}[{index}]"))?;
            }
        }
    }
    Ok(())
}

fn property_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}

fn array_of_strings(schema: &Value) -> bool {
    schema["items"].as_object().is_some_and(|items| {
        items.len() == 1 && items.get("type").and_then(Value::as_str) == Some("string")
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A tool's JSON as MCP content. `structuredContent` exists from 2025-06-18; the text block
/// carries the same JSON for clients that predate it.
#[derive(Clone, Debug)]
struct ToolError {
    code: ErrorCode,
    message: String,
}

impl ToolError {
    fn coded<E: Coded + std::fmt::Display>(error: E) -> Self {
        Self {
            code: error.code(),
            message: error.to_string(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
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

fn tool_result(outcome: Result<Value, ToolError>, protocol: &str) -> Value {
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

// ---- tool definitions -----------------------------------------------------------------------

fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false },
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": !read_only,
            "openWorldHint": false,
        },
    })
}

fn detail() -> Value {
    json!({ "type": "boolean", "description": "Return everything instead of the summary.", "default": false })
}

fn profile_arg() -> Value {
    json!({ "type": "string", "description": "Server profile name.", "default": "default" })
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "projects",
            "The solution's ThingWorx projects, their roots, their deploy order and how many entity documents each holds.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "types",
            "Generate editor declarations, type-check every service, or fetch and cache platform declarations. All actions take the workspace lock.",
            json!({
                "action": { "type": "string", "enum": ["generate", "check", "platform"], "default": "generate" },
                "profile": profile_arg(),
                "detail": detail(),
            }),
            &[],
            false,
        ),
        tool(
            "check",
            "Run every gate of the solution (line endings, sidecars in sync, formatting, script traps, code order, project validation, declared hooks). With live: true, every service script is also parsed by the ThingWorx server, and an unreachable server fails the check. live defaults to the solution's [gates] live.",
            json!({ "live": { "type": "boolean", "description": "Default: the solution's [gates] live." }, "profile": profile_arg(), "detail": detail() }),
            &[],
            true,
        ),
        tool(
            "status",
            "Compare entities with the server and the recorded baseline: in-sync, local-changed, server-changed, both-changed, not-on-server, or no baseline yet. Lists every entity that needs attention.",
            json!({
                "entity": { "type": "string", "description": "One entity name, full or its last dotted segment. Name one, or pass all: true." },
                "all": { "type": "boolean", "default": false, "description": "Every entity (of the project, if one is named)." },
                "project": { "type": "string", "description": "Narrow to one project of the solution." },
                "record": { "type": "boolean", "default": false, "description": "Record a baseline for every entity whose two sides agree. Writes .twaco/baseline.json under the workspace lock." },
                "profile": profile_arg(),
                "detail": detail(),
            }),
            &[],
            false,
        ),
        tool(
            "sync",
            "Write sidecars back into their entity XML: service scripts, DataShape fields, mashup content, DataTable configuration. This is how an edit to a script.js takes effect. check: true reports what would change and writes nothing. Takes the workspace lock while it writes.",
            json!({
                "entity": { "type": "string", "description": "One entity, full name or its last dotted segment. Name one, or pass all: true." },
                "all": { "type": "boolean", "default": false, "description": "Every entity (of the project, if one is named)." },
                "project": { "type": "string", "description": "Narrow to one project of the solution." },
                "check": { "type": "boolean", "default": false },
                "allow_add_remove": { "type": "boolean", "default": false, "description": "Permit a service or field to appear or disappear." },
                "relayout": { "type": "boolean", "default": false, "description": "Rewrite every script payload in the configured layout." },
            }),
            &[],
            false,
        ),
        tool(
            "extract",
            "Entity XML to sidecars: service scripts, DataShape fields, mashup content, DataTable configuration. Overwrites the sidecars of the entities chosen; takes the workspace lock.",
            json!({
                "entity": { "type": "string", "description": "One entity, full name or its last dotted segment. Name one, or pass all: true." },
                "all": { "type": "boolean", "default": false, "description": "Every entity (of the project, if one is named)." },
                "project": { "type": "string" },
            }),
            &[],
            false,
        ),
        tool(
            "fmt",
            "Format every service script sidecar with the built-in formatter. check: true reports which would change and writes nothing.",
            json!({ "check": { "type": "boolean", "default": false } }),
            &[],
            false,
        ),
        tool(
            "push",
            "Import one entity's file to the server. Refuses when the server changed since the last sync, was deleted there, or has no baseline, unless force is true. Reads the entity back and records a baseline only for what the server kept. A dry run unless dry_run is false.",
            json!({
                "entity": { "type": "string" },
                "dry_run": { "type": "boolean", "default": true },
                "force": { "type": "boolean", "default": false },
                "backup": { "type": "boolean", "default": true, "description": "With force, save the server's copy under .twaco/backups first (default true); false skips it." },
                "profile": profile_arg(),
            }),
            &["entity"],
            false,
        ),
        tool(
            "entity_delete",
            "Delete server entities in dependency-safe order. Accepts Collection/Name or a bare name resolved on the server; renamed adds undeleted entity/prefix entries from .twaco/renames.json. allow_repository_defined accepts a repository definition that deployment would recreate; allow_outside_dependents accepts structural dependents outside the delete set; allow_file_repository_data_loss accepts deleting a FileRepository Thing and all its files. A FileRepository delete needs its own acknowledgement. force is deprecated: it means the first two acknowledgements and never FileRepository data loss. A dry run unless dry_run is false; every applied delete is confirmed absent.",
            json!({
                "entities": { "type": "array", "items": { "type": "string" }, "description": "Entities to delete; may be empty when renamed is true." },
                "renamed": { "type": "boolean", "default": false },
                "allow_repository_defined": { "type": "boolean", "default": false },
                "allow_outside_dependents": { "type": "boolean", "default": false },
                "allow_file_repository_data_loss": { "type": "boolean", "default": false },
                "force": { "type": "boolean", "default": false, "description": "Deprecated: means allow_repository_defined and allow_outside_dependents; never accepts FileRepository data loss." },
                "dry_run": { "type": "boolean", "default": true },
                "backup": { "type": "boolean", "default": true, "description": "Save the server's copy of every entity under .twaco/backups before deleting (default true); a failed backup deletes nothing." },
                "profile": profile_arg(),
            }),
            &[],
            false,
        ),
        tool(
            "entity_restore",
            "List the backup sets taken before deletes and forced overwrites (no set given), or import one back: all its entities, or the named ones. A dry run unless dry_run is false; each import is confirmed on the server.",
            json!({
                "set": { "type": "string", "description": "A set id from the listing; omit to list sets." },
                "entities": { "type": "array", "items": { "type": "string" }, "description": "Only these (Collection/Name or a bare name)." },
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
            }),
            &[],
            false,
        ),
        tool(
            "entity_carry",
            "Copy run-time, design-time and visibility permissions from renamed (old) entities to the ones that replaced them, mapping principals through the rename ledger. pairs is a flat list [old, new, old, new, ...] of Collection/Name; renamed adds every pending ledger entity. A dry run unless dry_run is false; every write is read back and the ledger marked carried.",
            json!({
                "pairs": { "type": "array", "items": { "type": "string" }, "description": "Collection/Old, Collection/New, in pairs; may be empty when renamed is true." },
                "renamed": { "type": "boolean", "default": false },
                "dry_run": { "type": "boolean", "default": true },
                "detail": { "type": "boolean", "default": false, "description": "Also ask the platform for its own difference count." },
                "profile": profile_arg(),
            }),
            &[],
            false,
        ),
        tool(
            "db_run",
            "Run one SQL script as an atomic SQLCommand through a throwaway Database Thing. Plans by default and shows the SQL and sanitized connection target; pass dry_run: false to execute.",
            json!({
                "file": { "type": "string", "description": "A UTF-8 SQL file relative to the solution; give file or sql." },
                "sql": { "type": "string", "description": "Inline SQL; give sql or file." },
                "thing": { "type": "string", "description": "Database Thing; otherwise resolve the one in the solution." },
                "no_transaction": { "type": "boolean", "default": false, "description": "Prefix COMMIT; for statements such as CREATE DATABASE." },
                "timeout": { "type": "integer", "minimum": 1, "default": 120 },
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
            }),
            &[],
            false,
        ),
        tool(
            "db_query",
            "Run read-only SQLQuery through a throwaway Database Thing. Returns columns and the first 20 rows unless detail is true.",
            json!({
                "file": { "type": "string", "description": "A UTF-8 SQL file relative to the solution; give file or sql." },
                "sql": { "type": "string", "description": "Inline SQL; give sql or file." },
                "thing": { "type": "string", "description": "Database Thing; otherwise resolve the one in the solution." },
                "max_rows": { "type": "integer", "minimum": 1, "default": 500 },
                "timeout": { "type": "integer", "minimum": 1, "default": 120 },
                "profile": profile_arg(),
                "detail": detail(),
            }),
            &[],
            true,
        ),
        tool(
            "datatable_copy",
            "Copy the rows of one DataTable into the DataTable that replaced it, mapping fields by name, by the rename ledger, or by map ({old: new}). Refuses unmapped fields (unless drop_unmapped), type changes, and a non-empty target (unless append). A dry run unless dry_run is false; the target is read back and compared. Each row's source, tags and timestamp are not carried.",
            json!({
                "old": { "type": "string", "description": "The source DataTable's name." },
                "new": { "type": "string", "description": "The target DataTable's name." },
                "map": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Source field to target field." },
                "drop_unmapped": { "type": "boolean", "default": false },
                "append": { "type": "boolean", "default": false },
                "max_rows": { "type": "integer", "minimum": 1, "default": 100000 },
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
            }),
            &["old", "new"],
            false,
        ),
        tool(
            "db_clean",
            "Find, and with dry_run false delete, the temporary ZZ.Twaco.Sql.* Database Things an interrupted db_run or db_query left on the server. Only names twaco generates, on Database Things, are touched.",
            json!({
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
            }),
            &[],
            false,
        ),
        tool(
            "deploy",
            "Deploy the solution: offline gates, one bundle per project in dependency order, every script parsed by the server (fails closed), a conflict check per entity, then import, read-back, and the project's deploy and post-import services. A dry run (a plan) unless dry_run is false.",
            json!({
                "dry_run": { "type": "boolean", "default": true },
                "force": { "type": "boolean", "default": false, "description": "Overwrite entities the server changed since the last sync." },
                "backup": { "type": "boolean", "default": true, "description": "With force, save the server's copy of each overwritten entity under .twaco/backups first (default true)." },
                "only": { "type": "array", "items": { "type": "string" }, "description": "Only these entities; post-import services are skipped." },
                "only_projects": { "type": "array", "items": { "type": "string" } },
                "backend_only": { "type": "boolean", "default": false },
                "skip_checks": { "type": "boolean", "default": false, "description": "Skip the offline gates. The server's script parse still runs." },
                "profile": profile_arg(),
                "detail": detail(),
            }),
            &[],
            false,
        ),
        tool(
            "adopt_report",
            "Compare a designer's Composer <Entities> export with the repository: which services it would revert, which entities it adds or changes (node by node with detail), and which it lacks. Writes nothing.",
            json!({
                "export": { "type": "string", "description": "Path to the export, absolute or relative to the solution root." },
                "entity": { "type": "array", "items": { "type": "string" }, "description": "Only entities whose name contains one of these." },
                "detail": detail(),
            }),
            &["export"],
            true,
        ),
        tool(
            "adopt_apply",
            "Adopt the mechanical half of a designer's export: mashup content into sidecars, a new mashup's entity file, changed media. Never writes a service or configuration table; run sync afterwards. Takes the workspace lock.",
            json!({
                "export": { "type": "string", "description": "Path to the export, absolute or relative to the solution root." },
                "entity": { "type": "array", "items": { "type": "string" } },
            }),
            &["export"],
            false,
        ),
        tool(
            "rename",
            "Rename one entity, a dotted project/building-block prefix, a DataShape field, a declared service, one service parameter, or a configuration table. A dry run unless dry_run is false; an apply takes the workspace lock. The result carries a plan_digest: pass it back with dry_run false to apply exactly the plan that was reviewed.",
            json!({
                "kind": { "type": "string", "enum": ["entity", "prefix", "field", "service", "param", "table", "property"] },
                "scope": { "type": "string", "description": "Entity full name; required for kind field, service or table." },
                "service": { "type": "string", "description": "Service name; required for kind param and refused otherwise." },
                "old": { "type": "string" },
                "new": { "type": "string" },
                "dry_run": { "type": "boolean", "default": true },
                "include_outside": { "type": "boolean", "default": false },
                "sql": { "type": "boolean", "default": false, "description": "Write the database migration script when the rename touches DBConnection tables (into sql/ or sql_dir). Such a rename is refused unless sql or no_sql is given." },
                "sql_dir": { "type": "string", "description": "Folder for the migration script, relative to the solution; implies sql." },
                "no_sql": { "type": "boolean", "default": false, "description": "The DBConnection tables are not in use: write no script." },
                "skip_checks": { "type": "boolean", "default": false },
                "plan_digest": { "type": "string", "description": "The plan_digest a dry run returned. With dry_run false, the rename is refused (nothing written) unless the plan is still exactly that one." },
            }),
            &["kind", "old", "new"],
            false,
        ),
        tool(
            "move_member",
            "Move or copy a service or a property from one Thing, template or shape to another in the repository: the definition (and a service's implementation and sidecar) is lifted out byte for byte and re-indented where it lands. Refuses a name the target, its ancestors or its descendants already use; reports the callers that stop resolving when the target is not something the source inherits; leave_delegate keeps a service on the source that calls the moved one (a Thing target). A dry run unless dry_run is false; an apply takes the workspace lock.",
            json!({
                "action": { "type": "string", "enum": ["move", "copy"] },
                "kind": { "type": "string", "enum": ["service", "property"] },
                "from": { "type": "string", "description": "The entity the member is on." },
                "to": { "type": "string", "description": "The entity it goes to." },
                "name": { "type": "string" },
                "new_name": { "type": "string", "description": "A new name on the target." },
                "leave_delegate": { "type": "boolean", "default": false },
                "dry_run": { "type": "boolean", "default": true },
            }),
            &["action", "kind", "from", "to", "name"],
            false,
        ),
        tool(
            "new_building_block",
            "Create a new building block in the repository, as the PTC Solution Framework's Create New Building Block does on a server: its project, EntryPoint template and Thing, Management shape, Manager template and Thing (not for an abstract block), default and admin groups and organization, as files, plus the project in twaco.toml. The files match what the framework produced on a server. The permission helper and the ui and test types are not created. A dry run unless dry_run is false; an apply takes the workspace lock.",
            json!({
                "name": { "type": "string", "description": "The block's name with its namespace, such as Acme.Orders." },
                "type": { "type": "string", "enum": ["standard", "abstract", "implementation"], "default": "standard" },
                "display_name": { "type": "string" },
                "description": { "type": "string" },
                "parent": { "type": "string", "description": "The abstract block an implementation implements." },
                "model_logic": { "type": "boolean", "default": false },
                "management_shape": { "type": "boolean", "default": true, "description": "An implementation may leave out its Management_TS." },
                "root": { "type": "string", "description": "The project's folder, relative to the solution; default the block's name." },
                "base_extension": { "type": "string", "description": "PTC.Base:<version>; default what another project of the solution declares." },
                "dry_run": { "type": "boolean", "default": true },
            }),
            &["name"],
            false,
        ),
        tool(
            "retemplate",
            "Change a Thing's template (or a template's base template) and/or the shapes it implements, in the repository. The plan lists what the entity and everything inheriting it gains and loses, the stored property values and configuration-table rows left with no definition, and the references to a lost member; such a loss is refused unless accept_loss is true. A dry run unless dry_run is false; an apply takes the workspace lock.",
            json!({
                "entity": { "type": "string", "description": "The Thing or template to change." },
                "template": { "type": "string", "description": "The new template (a Thing) or base template (a template)." },
                "add_shapes": { "type": "array", "items": { "type": "string" } },
                "remove_shapes": { "type": "array", "items": { "type": "string" } },
                "accept_loss": { "type": "boolean", "default": false },
                "dry_run": { "type": "boolean", "default": true },
            }),
            &["entity"],
            false,
        ),
        tool(
            "config_table",
            "Read one Thing's configuration table on the server, diff it against the entity XML in the repository, or restore it from a backup file. restore is a dry run unless dry_run is false; it refuses a backup of another Thing or table, and reads the table back.",
            json!({
                "thing": { "type": "string" },
                "table": { "type": "string" },
                "action": { "type": "string", "enum": ["read", "diff", "restore"], "default": "read" },
                "backup": { "type": "string", "description": "restore only: the backup file, as `twaco config-table --backup` writes it." },
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
                "detail": detail(),
            }),
            &["thing", "table"],
            false,
        ),
        tool(
            "logs",
            "Read a server log: ApplicationLog, ScriptLog, CommunicationLog, ConfigurationLog or SecurityLog. Newest first. A summary first (counts by level, top origins, repeated messages, the newest entries); detail: true lists every entry. truncated: true means the limit was reached.",
            json!({
                "log": { "type": "string", "description": "ApplicationLog, ScriptLog, CommunicationLog, ConfigurationLog or SecurityLog." },
                "since": { "type": "string", "default": "1h", "description": "How far back, ending now: a number and s, m, h or d. Not with from/to." },
                "from": { "type": "string", "description": "ISO-8601 with Z or an offset (2026-10-01T06:37:00Z); without one it is in the local time zone of the machine running twaco. Or `now`." },
                "to": { "type": "string", "description": "ISO-8601 or `now` (the default)." },
                "level": { "type": "string", "enum": ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"], "description": "This level and above." },
                "grep": { "type": "string", "description": "Entries whose message contains this text." },
                "regex": { "type": "string", "description": "A Java regex that must match the whole message. Not with grep." },
                "user": { "type": "string" },
                "thread": { "type": "string" },
                "origin": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "default": 200 },
                "oldest_first": { "type": "boolean", "default": false },
                "profile": profile_arg(),
                "detail": detail(),
            }),
            &["log"],
            true,
        ),
        tool(
            "log_level",
            "Read a server log's level and its subloggers' levels, or change one. ScriptLog at WARN records no logger.info or logger.debug from scripts; lower it while debugging, then put it back. A change is the whole server's and a dry run unless dry_run is false; the result says how to undo it.",
            json!({
                "log": { "type": "string", "description": "ApplicationLog, ScriptLog, CommunicationLog, ConfigurationLog or SecurityLog." },
                "level": { "type": "string", "enum": ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"], "description": "Set this level. Omit, with reset false, to only read." },
                "sublogger": { "type": "string", "description": "A class or package within the log, such as com.thingworx." },
                "reset": { "type": "boolean", "default": false, "description": "Put the sublogger (or, without one, every sublogger) back to the log's level." },
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
            }),
            &["log"],
            false,
        ),
        tool(
            "repo",
            "Read the server's file repositories: list them, list a folder (recursive: true for everything below), get a text file's content, or compare the tree kept in source control (filerepository/<repo>/) with the server's (same, differs, local-only, remote-only; equal sizes are compared by SHA-256). Read-only.",
            json!({
                "action": { "type": "string", "enum": ["list", "ls", "get", "status"], "default": "list" },
                "repository": { "type": "string", "description": "A FileRepository Thing, such as SystemRepository. Not for list." },
                "path": { "type": "string", "description": "ls: a folder (default /); get: a file, such as Thumbnails/a.png." },
                "recursive": { "type": "boolean", "default": false },
                "max_chars": { "type": "integer", "minimum": 1000, "default": 100000, "description": "get: the most text returned." },
                "profile": profile_arg(),
                "detail": detail(),
            }),
            &[],
            true,
        ),
        tool(
            "repo_write",
            "Change a file repository: put (upload text, or a file of the solution), mkdir, rm (a file, or a folder; recursive: true to delete one that holds anything), mv (a file), or push/pull the tree kept in source control. A dry run unless dry_run is false. Nothing existing is replaced without overwrite: true; applied changes are read back.",
            json!({
                "action": { "type": "string", "enum": ["put", "mkdir", "rm", "mv", "push", "pull"], "description": "push/pull: the whole tree filerepository/<repo>/ to or from the server; neither deletes." },
                "repository": { "type": "string" },
                "path": { "type": "string", "description": "put/mkdir/rm: the repository path; mv: the source." },
                "to": { "type": "string", "description": "mv: the file's new path." },
                "text": { "type": "string", "description": "put: the content, as UTF-8 text." },
                "local": { "type": "string", "description": "put: a file of the solution to upload instead, relative to the solution root." },
                "overwrite": { "type": "boolean", "default": false },
                "recursive": { "type": "boolean", "default": false },
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
            }),
            &["action", "repository"],
            false,
        ),
        tool(
            "extensions",
            "The server's extension packages: list them, or show one (its extensions, and which are in use). Read-only.",
            json!({
                "action": { "type": "string", "enum": ["list", "show"], "default": "list" },
                "package": { "type": "string", "description": "show: the package name." },
                "profile": profile_arg(),
            }),
            &[],
            true,
        ),
        tool(
            "extension_write",
            "Import an extension package zip of the solution, or remove an installed package. A dry run unless dry_run is false: an import is then only validated by the server, which installs nothing. A removal is refused while the package is in use.",
            json!({
                "action": { "type": "string", "enum": ["import", "remove"] },
                "zip": { "type": "string", "description": "import: the package zip, relative to the solution root." },
                "package": { "type": "string", "description": "remove: the package name." },
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
            }),
            &["action"],
            false,
        ),
        tool(
            "export",
            "Export from the server as Composer's Import/Export does: an entity (Collection/Name), a collection (optionally one project's part), or a whole project, as one XML file written inside the solution; or the source-control layout of a project, collection or tags into a file repository folder or zip (a dry run unless dry_run is false).",
            json!({
                "action": { "type": "string", "enum": ["entity", "collection", "project", "source_control"] },
                "entity": { "type": "string", "description": "entity: Collection/Name, such as Things/My.Thing." },
                "collection": { "type": "string" },
                "project": { "type": "string" },
                "out": { "type": "string", "description": "entity/collection/project: the file to write, relative to the solution root." },
                "overwrite": { "type": "boolean", "default": false },
                "repository": { "type": "string", "description": "source_control: the file repository." },
                "path": { "type": "string", "description": "source_control: the folder in it." },
                "tags": { "type": "string" },
                "zip": { "type": "string", "description": "source_control: write a zip of this name instead of a folder." },
                "with_dependents": { "type": "boolean", "default": false },
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
            }),
            &["action"],
            false,
        ),
        tool(
            "package",
            "Package the repository for release, offline, into a file inside the solution: a bundle (one importable XML; part all, backend or frontend), a source-control zip (<Project>/<Collection>/<Name>.xml), or a DPM-style extension zip (one project's, or the solution's as a zip of its projects' zips; editable or not; metadata from [package] in twaco.toml).",
            json!({
                "action": { "type": "string", "enum": ["bundle", "source_control", "extension"] },
                "project": { "type": "string", "description": "Only this project; the whole solution when omitted." },
                "part": { "type": "string", "enum": ["all", "backend", "frontend"], "default": "all", "description": "bundle: which collections." },
                "editable": { "type": "boolean", "default": false, "description": "extension: whether the entities stay editable once installed." },
                "out": { "type": "string", "description": "The file to write, relative to the solution root." },
                "overwrite": { "type": "boolean", "default": false },
            }),
            &["action", "out"],
            false,
        ),
        tool(
            "import",
            "Import into the server as Composer's Import/Export does: an export file of the solution (XML, or a zip of them), or a source-control tree in a file repository. A dry run unless dry_run is false: a file's plan lists what it adds and what it replaces; a source-control plan is the server's own diff. The server's property values and configuration table rows are kept unless the overwrite flags say otherwise.",
            json!({
                "action": { "type": "string", "enum": ["file", "source_control"] },
                "file": { "type": "string", "description": "file: relative to the solution root." },
                "repository": { "type": "string", "description": "source_control: the file repository." },
                "path": { "type": "string", "description": "source_control: the tree's folder in it." },
                "overwrite_properties": { "type": "boolean", "default": false },
                "overwrite_tables": { "type": "boolean", "default": false },
                "dry_run": { "type": "boolean", "default": true },
                "profile": profile_arg(),
            }),
            &["action"],
            false,
        ),
        tool(
            "settings",
            "Read the server's subsystem settings (Composer: Browse > Subsystems): list the subsystems, show one with every setting, its value and description, or search every setting by name or description. Read-only; PASSWORD values are never shown.",
            json!({
                "action": { "type": "string", "enum": ["list", "show", "search"], "default": "list" },
                "subsystem": { "type": "string", "description": "show: a subsystem, such as Logging or LoggingSubsystem." },
                "text": { "type": "string", "description": "search: part of a setting's name or description." },
                "profile": profile_arg(),
            }),
            &[],
            true,
        ),
        tool(
            "catalog",
            "The repository's offline service catalog: callable services by entity, their parameters, result, description, origin and whether code exists. Summary returns at most 50 services; detail returns all.",
            json!({
                "entity": { "type": "string", "description": "One entity, full name or its last dotted segment." },
                "project": { "type": "string", "description": "Narrow to one project of the solution." },
                "text": { "type": "string", "description": "Case-insensitive match over service names, parameter names and descriptions; use entity to narrow to one entity." },
                "detail": detail(),
            }),
            &[],
            true,
        ),
        tool(
            "impact",
            "What changing an entity, or one service, property or field of it, would reach: the Things, templates, shapes, mashups and projects that depend on it, directly or through others, each at the strength of its weakest reference (structural: declared in the XML or twaco.toml; resolved: a static name in a script or a mashup binding; review: a string that looks like the name, for a person to judge). Offline and read-only. It cannot see references built at run time or outside the repository; `complete` says whether every input was read and `unreadable` lists what was not. Summary leaves the chain to each dependent out; detail includes it.",
            json!({
                "entity": { "type": "string", "description": "Collection/Name, a full name, or its last dotted segment." },
                "member": { "type": "string", "description": "A service, property or field of the entity: ask only about what names it." },
                "min_confidence": { "type": "string", "enum": ["structural", "resolved", "review"], "default": "review", "description": "The weakest reference to follow." },
                "depth": { "type": "integer", "minimum": 1, "description": "How many references away to look; omit for no limit." },
                "format": { "type": "string", "enum": ["json", "dot"], "default": "json", "description": "dot returns the dependents as a Graphviz graph in `dot`." },
                "detail": detail(),
            }),
            &["entity"],
            true,
        ),
        tool(
            "unused",
            "Entities nothing reaches: Things, templates, shapes and DataShapes with no chain of references from an entry point (what twaco.toml deploys, every mashup, anything that runs on events, and what `[unused] keep` names). Advisory and read-only: it deletes nothing, and an entity used only from outside the repository (a REST client, a connected system) looks unused, so list those under `[unused] keep`. `complete` says whether every input was read; `keep_unmatched` lists keep patterns that match nothing.",
            json!({
                "min_confidence": { "type": "string", "enum": ["structural", "resolved", "review"], "default": "review", "description": "The weakest reference that counts as use; a stronger minimum reports more entities." },
                "collection": { "type": "string", "enum": ["Things", "ThingTemplates", "ThingShapes", "DataShapes"], "description": "Judge one collection only." },
                "detail": detail(),
            }),
            &[],
            true,
        ),
        tool(
            "docs",
            "The solution written down from the repository: the projects and their deploy order, how Things, templates and shapes inherit, every service with its signature, the DataShapes with their fields and where they are used, and the references that only look like a name and need a person's judgement. Offline and read-only; it has no dates, so regenerating it and diffing shows what changed. Permissions are not read yet, and references built at run time are not seen; `complete` says whether every input was read. Returns the document as JSON in `document` and as Markdown in `markdown`. Summary gives service and field counts; detail gives every signature and field.",
            json!({
                "detail": detail(),
            }),
            &[],
            true,
        ),
        tool(
            "guide",
            "Knowledge for working on this solution: twaco's workflow, the ThingWorx platform's verified-live quirks, the service-code reference, and the solution's own AGENTS.md, CLAUDE.md and docs/. Search before a live import, a hand-written mashup binding, a configuration-table change, or a service that introspects metadata or touches JSON. list: the topics; search: the best-matching sections; read: a topic (a long one gives its outline) or one section by heading.",
            json!({
                "action": { "type": "string", "enum": ["list", "search", "read"], "default": "search" },
                "text": { "type": "string", "description": "search: words, such as \"AddMember group\" or \"configuration table row replaced\"." },
                "topic": { "type": "string", "description": "read: a topic id from list or search, such as quirks or docs/DEVELOPER_GUIDE." },
                "section": { "type": "string", "description": "read: a section heading, or a part of it that only one heading has." },
                "limit": { "type": "integer", "minimum": 1, "default": 8 },
            }),
            &[],
            true,
        ),
        tool(
            "help_search",
            "Search the ThingWorx Platform help center for the server's version (or another): pages holding every word, best first, with title, path, summary and address. It explains concepts and how-tos; for one service's parameters, the editor types (`types`) are better.",
            json!({
                "query": { "type": "string", "description": "Words, as in the help's own search box. Code such as Resources[\"InfoTableFunctions\"] is split into its words." },
                "limit": { "type": "integer", "minimum": 1, "default": 10 },
                "version": { "type": "string", "description": "A help release such as 10.1; default: the server's own." },
                "refresh": { "type": "boolean", "default": false, "description": "Download the index again instead of using the cache." },
                "profile": profile_arg(),
            }),
            &["query"],
            true,
        ),
        tool(
            "help_page",
            "Read one ThingWorx Platform help page as Markdown, from a path or address that help_search returned. Long pages: ask for one section by heading; the result lists every heading.",
            json!({
                "page": { "type": "string", "description": "A path such as ThingWorx/Help/Composer/Things/ThingServices/QueryParameterforQueryServices.html, or its address." },
                "section": { "type": "string", "description": "Only the part under the first heading containing this text." },
                "max_chars": { "type": "integer", "minimum": 1000, "default": 20000 },
                "version": { "type": "string", "description": "A help release such as 10.1; default: the one in the address, else the server's own." },
                "refresh": { "type": "boolean", "default": false },
                "profile": profile_arg(),
            }),
            &["page"],
            true,
        ),
        tool(
            "javadoc",
            "Search or read the ThingWorx Platform API 10.1.0 Javadoc as Markdown. search ranks class and member simple names exact, prefix, then contains. class gives the class description and method summaries; member gives every overload with parameters, returns and throws. Useful for the Java methods of objects scripts call and Resource service parameter descriptions.",
            json!({
                "action": { "type": "string", "enum": ["search", "class"] },
                "name": { "type": "string", "description": "A search name, or a simple/qualified class name." },
                "member": { "type": "string", "description": "class: return full details for every overload of this member." },
                "limit": { "type": "integer", "minimum": 1, "default": 10 },
                "refresh": { "type": "boolean", "default": false },
            }),
            &["action", "name"],
            true,
        ),
        tool(
            "call",
            "Call a ThingWorx service. A service can write, and twaco cannot tell which do, so this is a dry run unless dry_run is false. A bare target is a Thing; Collection/Name reaches templates, shapes, resources and subsystems.",
            json!({
                "target": { "type": "string", "description": "Collection/Name, or an entity of the solution by full or last-segment name (Manager), or a platform Thing's name." },
                "service": { "type": "string" },
                "parameters": { "type": "object", "default": {} },
                "dry_run": { "type": "boolean", "default": true },
                "timeout_seconds": { "type": "integer", "minimum": 1, "default": 120 },
                "with_logs": { "type": "boolean", "default": false, "description": "Also return what the call wrote to ScriptLog and ApplicationLog, even when it fails. Waits up to 3 s for late entries. ScriptLog at WARN records no logger.info/debug; see log_level." },
                "profile": profile_arg(),
                "detail": detail(),
            }),
            &["target", "service"],
            false,
        ),
    ]
}

// ---- tool implementations -------------------------------------------------------------------

fn call_tool(root: &Path, name: &str, arguments: &Value) -> Option<Result<Value, ToolError>> {
    let started = Instant::now();
    let outcome = match name {
        "projects" => with_solution(root, projects),
        "types" => with_solution(root, |s| types_tool(s, arguments)),
        "check" => with_solution(root, |s| check_tool(s, arguments)),
        "status" => with_solution(root, |s| status_tool(s, arguments)),
        "sync" => with_solution(root, |s| sync_tool(s, arguments)),
        "extract" => with_solution(root, |s| extract_tool(s, arguments)),
        "fmt" => with_solution(root, |s| fmt_tool(s, arguments)),
        "adopt_report" => with_solution(root, |s| adopt_tool(s, arguments)),
        "adopt_apply" => with_solution(root, |s| adopt_apply_tool(s, arguments)),
        "rename" => with_solution(root, |s| rename_tool(s, arguments)),
        "push" => with_solution(root, |s| push_tool(s, arguments)),
        "entity_delete" => with_solution(root, |s| entity_delete_tool(s, arguments)),
        "entity_carry" => with_solution(root, |s| entity_carry_tool(s, arguments)),
        "move_member" => with_solution(root, |s| move_member_tool(s, arguments)),
        "retemplate" => with_solution(root, |s| retemplate_tool(s, arguments)),
        "new_building_block" => with_solution(root, |s| new_building_block_tool(s, arguments)),
        "entity_restore" => with_solution(root, |s| entity_restore_tool(s, arguments)),
        "db_run" => with_solution(root, |s| db_tool(s, arguments, db::Mode::Run)),
        "db_query" => with_solution(root, |s| db_tool(s, arguments, db::Mode::Query)),
        "db_clean" => with_solution(root, |s| db_clean_tool(s, arguments)),
        "datatable_copy" => with_solution(root, |s| datatable_copy_tool(s, arguments)),
        "deploy" => with_solution(root, |s| deploy_tool(s, arguments)),
        "config_table" => with_solution(root, |s| config_table_tool(s, arguments)),
        "call" => with_solution(root, |s| call_service_tool(s, arguments)),
        "logs" => with_solution(root, |s| logs_tool(s, arguments)),
        "help_search" => help_search_tool(root, arguments),
        "guide" => guide_tool(root, arguments),
        "repo" => with_solution(root, |s| repo_tool(s, arguments)),
        "repo_write" => with_solution(root, |s| repo_write_tool(s, arguments)),
        "extensions" => with_solution(root, |s| extensions_tool(s, arguments)),
        "settings" => with_solution(root, |s| settings_tool(s, arguments)),
        "catalog" => with_solution(root, |s| catalog_tool(s, arguments)),
        "impact" => with_solution(root, |s| impact_tool(s, arguments)),
        "unused" => with_solution(root, |s| unused_tool(s, arguments)),
        "docs" => with_solution(root, |s| docs_tool(s, arguments)),
        "export" => with_solution(root, |s| export_tool(s, arguments)),
        "package" => with_solution(root, |s| package_tool(s, arguments)),
        "import" => with_solution(root, |s| import_tool(s, arguments)),
        "extension_write" => with_solution(root, |s| extension_write_tool(s, arguments)),
        "help_page" => help_page_tool(root, arguments),
        "javadoc" => javadoc_tool(arguments),
        "log_level" => with_solution(root, |s| log_level_tool(s, arguments)),
        _ => return None,
    };
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

fn with_solution(
    root: &Path,
    tool: impl FnOnce(&Solution) -> Result<Value, ToolError>,
) -> Result<Value, ToolError> {
    let solution = Solution::discover(root).map_err(ToolError::coded)?;
    tool(&solution)
}

fn flag(arguments: &Value, name: &str, default: bool) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn text<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Value::as_str)
}

fn required<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    text(arguments, name)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ToolError::invalid(format!("`{name}` is required")))
}

fn client(solution: &Solution, arguments: &Value) -> Result<server::Client, ToolError> {
    let name = text(arguments, "profile").unwrap_or("default");
    profile::load(&solution.root, name)
        .map(server::Client::new)
        .map_err(ToolError::coded)
}

fn relative(solution: &Solution, path: &Path) -> String {
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

fn types_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    types_tool_with_compiler(solution, arguments, None)
}

fn types_tool_with_compiler(
    solution: &Solution,
    arguments: &Value,
    compiler: Option<&dyn types::CompilerRunner>,
) -> Result<Value, ToolError> {
    let action = match text(arguments, "action").unwrap_or("generate") {
        "generate" => commands::types::TypesAction::Generate,
        "check" => commands::types::TypesAction::Check,
        "platform" => commands::types::TypesAction::Platform,
        action => commands::types::TypesAction::Invalid(format!(
            "action must be generate, check or platform, not {action:?}"
        )),
    };
    let request = commands::types::TypesRequest {
        action,
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
        lock_label: "mcp types",
    };
    let mut notices = commands::Notices::default();
    let result = commands::types::execute(
        solution,
        &request,
        server::Client::new,
        compiler,
        &mut notices,
    );
    match result.map_err(ToolError::coded)? {
        commands::types::TypesOutcome::Generated(outcome) => {
            let mut result = json!({
                "ok": true,
                "entities": outcome.entities,
                "data_shapes": outcome.data_shapes,
                "services": outcome.services,
                "files_written": outcome.files_written,
                "skipped": outcome.skipped,
            });
            if !outcome.gitignore_covers_types {
                result["gitignore_note"] = json!("add `.twaco/types/`, `**/services/*/jsconfig.json`, and `**/services/*/twaco-globals.d.ts` to the solution root's .gitignore");
            }
            add_notices(&mut result, &notices);
            Ok(result)
        }
        commands::types::TypesOutcome::Checked(outcome) => {
            let mut by_code = std::collections::BTreeMap::new();
            for finding in &outcome.findings {
                *by_code
                    .entry(format!("TS{}", finding.code))
                    .or_insert(0usize) += 1;
            }
            let findings = outcome.findings.iter().map(|finding| {
                json!({
                    "file": finding.file,
                    "line": finding.line,
                    "column": finding.column,
                    "code": format!("TS{}", finding.code),
                    "message": finding.message,
                })
            });
            let mut result = json!({
                "ok": outcome.findings.is_empty(),
                "findings": outcome.findings.len(),
                "services_with_findings": outcome.affected_services,
                "services": outcome.services,
                "seconds": outcome.elapsed.as_secs_f64(),
                "by_code": by_code,
            });
            if flag(arguments, "detail", false) {
                result["findings_list"] = Value::Array(findings.collect());
            } else {
                result["first"] = Value::Array(findings.take(20).collect());
            }
            add_notices(&mut result, &notices);
            Ok(result)
        }
        commands::types::TypesOutcome::Platform(outcome) => {
            let mut result = json!({
                "ok": true,
                "templates": outcome.templates,
                "shapes": outcome.shapes,
                "resources": outcome.resources,
                "skipped": outcome.skipped.into_iter().chain(outcome.types.skipped).collect::<Vec<_>>(),
            });
            add_notices(&mut result, &notices);
            Ok(result)
        }
    }
}

/// Every gate, and the live script parse when `live`: what `check` reports and `deploy` obeys.
fn run_gates(solution: &Solution, arguments: &Value, live: bool) -> check::CheckReport {
    let mut report = check::run(solution);
    if live {
        let built = client(solution, arguments).map_err(|error| error.message);
        let checker = built
            .as_ref()
            .map(|c| c as &dyn check::ScriptChecker)
            .map_err(Clone::clone);
        report.gates.push(check::live_parse(solution, checker));
    }
    report
}

fn check_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let report = run_gates(
        solution,
        arguments,
        flag(arguments, "live", solution.gates.live),
    );
    let detail = flag(arguments, "detail", false);
    let gates: Vec<Value> = report
        .gates
        .iter()
        .map(|gate| {
            let mut entry = json!({ "gate": gate.name, "examined": gate.examined, "findings": gate.findings.len() });
            if let Some(why) = &gate.broken {
                entry["broken"] = json!(why);
            }
            if !gate.gates_the_run {
                entry["advisory"] = json!(true);
            }
            entry
        })
        .collect();
    let mut result = json!({
        "ok": !report.blocks(),
        "gates": gates,
        "findings": report.findings(),
        "broken": report.broken(),
    });
    if detail {
        let findings: Vec<Value> = report
            .gates
            .iter()
            .flat_map(|gate| &gate.findings)
            .map(|f| json!({ "gate": f.gate, "file": f.file, "line": f.line, "rule": f.rule, "message": f.message }))
            .collect();
        result["failures"] = json!(findings);
    }
    Ok(result)
}

fn status_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let record = flag(arguments, "record", false);
    // Taken before anything is read, so what is recorded is what this call saw.
    let _lock = if record {
        Some(lock::acquire_for(solution, "mcp status --record").map_err(ToolError::coded)?)
    } else {
        None
    };
    let found = workspace::discover(solution);
    // An unreadable entity file is as much a partial read as a failed server read, and is
    // known before the server is asked anything.
    if record && !found.unreadable.is_empty() {
        return Err(ToolError::with(
            ErrorCode::InvalidData,
            format!(
                "nothing was recorded: {} entity file(s) could not be read: {}",
                found.unreadable.len(),
                found.unreadable.join("; ")
            ),
        ));
    }
    let mut pool = found.entities;
    if let Some(project) = text(arguments, "project") {
        if solution.project(project).is_none() {
            return Err(ToolError::invalid(format!(
                "this solution has no project named {project}"
            )));
        }
        pool.retain(|e| e.found_under == project);
    }
    let (chosen, _) = pick(pool, arguments)?;
    let client = client(solution, arguments)?;
    let mut baseline = baseline::Baseline::load(&solution.root).map_err(ToolError::coded)?;
    let (statuses, failures) = status::compute(&client, &baseline, &chosen);
    if record && !failures.is_empty() {
        // As on the command line: a baseline recorded from a partial read is a partial truth.
        return Err(format!(
            "nothing was recorded: {} entity read(s) failed: {}",
            failures.len(),
            failures
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        )
        .into());
    }
    let recorded = if record {
        let recorded = status::record_matching(&mut baseline, &statuses);
        baseline.write(&solution.root).map_err(ToolError::coded)?;
        Some(recorded)
    } else {
        None
    };
    let counts: Map<String, Value> = status::counts(&statuses)
        .into_iter()
        .map(|(v, n)| (v.label().to_string(), json!(n)))
        .collect();
    let detail = flag(arguments, "detail", false);
    let listed: Vec<Value> = statuses
        .iter()
        .filter(|s| detail || s.verdict.is_drift())
        .map(|s| {
            let mut entry = json!({ "entity": format!("{}/{}", s.collection, s.name), "verdict": s.verdict.label() });
            if detail {
                entry["working"] = json!(s.working);
                entry["server"] = json!(s.server);
                entry["baseline_local"] = json!(s.local_baseline);
                entry["baseline_server"] = json!(s.server_baseline);
            }
            entry
        })
        .collect();
    let mut result = json!({
        "ok": failures.is_empty() && statuses.iter().all(|s| !s.verdict.is_drift()),
        "recorded": recorded,
        "entities": statuses.len(),
        "counts": counts,
        "failures": failures,
        "unreadable": found.unreadable,
    });
    // Summary: only what needs attention. Detail: every entity, with its hashes.
    result[if detail { "statuses" } else { "attention" }] = json!(listed);
    Ok(result)
}

/// The optional entity spelling that an executor will validate after taking a write lock.
fn tool_target(arguments: &Value) -> Vec<String> {
    text(arguments, "entity")
        .map(str::to_string)
        .into_iter()
        .collect()
}

/// One entity by name, or every one with `all: true`; status uses this after it has discovered
/// the workspace because it needs resolved entity files for its server reads.
fn pick(
    pool: Vec<workspace::EntityFile>,
    arguments: &Value,
) -> Result<(Vec<workspace::EntityFile>, bool), ToolError> {
    match (text(arguments, "entity"), flag(arguments, "all", false)) {
        (Some(_), true) => Err(ToolError::invalid(
            "name an entity or pass all: true, not both",
        )),
        (Some(name), false) => Ok((
            vec![workspace::resolve(&pool, name)
                .map_err(ToolError::coded)?
                .clone()],
            true,
        )),
        (None, true) => Ok((pool, false)),
        (None, false) => Err(ToolError::invalid("name an entity, or pass all: true")),
    }
}

fn sync_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let check = flag(arguments, "check", false);
    let target = tool_target(arguments);
    let request = commands::sync::SyncRequest {
        target: commands::sync::SyncTarget {
            project: text(arguments, "project").map(str::to_string),
            entities: target,
            all: flag(arguments, "all", false),
            reject_entities_with_all: true,
            missing_target: "name an entity, or pass all: true",
        },
        mode: if check { Mode::Plan } else { Mode::Apply },
        allow_structural: flag(arguments, "allow_add_remove", false),
        relayout: flag(arguments, "relayout", false),
        lock_label: "mcp sync",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::sync::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?
        .report;
    let mut result = json!({
        "ok": outcome.failed == 0,
        "check": check,
        "checked": outcome.checked,
        "changed": outcome.changed,
        "failed": outcome.failed,
        "changes": outcome.log.changes().collect::<Vec<_>>(),
        "errors": outcome.log.errors().collect::<Vec<_>>(),
    });
    add_types_refresh(&mut result, &outcome.types);
    add_notices(&mut result, &notices);
    Ok(result)
}

fn extract_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let target = tool_target(arguments);
    let request = commands::extract::ExtractRequest {
        target: commands::extract::ExtractTarget {
            project: text(arguments, "project").map(str::to_string),
            entities: target,
            all: flag(arguments, "all", false),
            reject_entities_with_all: true,
            missing_target: "name an entity, or pass all: true",
        },
        lock_label: "mcp extract",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::extract::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?
        .report;
    let mut result = json!({
        "ok": outcome.failed == 0,
        "parts": outcome.written,
        "entities": outcome.entities,
        "failed": outcome.failed,
        "written": outcome.log.changes().collect::<Vec<_>>(),
        "errors": outcome.log.errors().collect::<Vec<_>>(),
    });
    add_types_refresh(&mut result, &outcome.types);
    add_notices(&mut result, &notices);
    Ok(result)
}

fn add_types_refresh(result: &mut Value, refresh: &types::Refresh) {
    if let Some(files) = refresh.files_written {
        result["types_refreshed"] = json!(files);
    }
    if let Some(warning) = &refresh.warning {
        result["types_warning"] = json!(warning);
    }
}

fn fmt_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let check = flag(arguments, "check", false);
    let request = commands::fmt::FmtRequest {
        mode: if check { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp fmt",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::fmt::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?
        .report;
    let mut result = json!({
        "ok": outcome.failed == 0 && (!check || outcome.changed.is_empty()),
        "check": check,
        "scripts": outcome.files,
        "changed": outcome.changed.iter().map(|p| relative(solution, p)).collect::<Vec<_>>(),
        "failed": outcome.failed,
        "errors": outcome.log.errors().collect::<Vec<_>>(),
    });
    add_notices(&mut result, &notices);
    Ok(result)
}

fn strings(arguments: &Value, name: &str) -> Vec<String> {
    arguments
        .get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn deploy_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let force = flag(arguments, "force", false);
    let detail = flag(arguments, "detail", false);
    // A real deploy writes the baseline, so it holds the workspace lock from before the checks
    // read the files it sends; a plan reads only.
    let _lock = if dry_run {
        None
    } else {
        Some(lock::acquire_for(solution, "mcp deploy").map_err(ToolError::coded)?)
    };
    if !flag(arguments, "skip_checks", false) {
        // The same gates as `twaco deploy`, the configured live parse included.
        let report = run_gates(solution, arguments, solution.gates.live);
        if report.blocks() {
            let failing: Vec<Value> = report
                .gates
                .iter()
                .filter(|g| g.blocks())
                .map(
                    |g| json!({ "gate": g.name, "findings": g.findings.len(), "broken": g.broken }),
                )
                .collect();
            return Ok(json!({
                "ok": false,
                "stage": "offline gates",
                "blocking_gates": failing,
                "note": "nothing was sent; fix these, or pass skip_checks: true",
            }));
        }
    }
    let only = strings(arguments, "only");
    let only_projects = strings(arguments, "only_projects");
    let (projects, notes) = deploy::plan_bundles(
        solution,
        deploy::PlanOptions {
            only_projects: &only_projects,
            only: &only,
            backend_only: flag(arguments, "backend_only", false),
        },
    )?;
    let name = text(arguments, "profile").unwrap_or("default");
    let profile = profile::load(&solution.root, name).map_err(ToolError::coded)?;
    let client = server::Client::new(profile.clone());
    let saved = if !dry_run && force && flag(arguments, "backup", true) {
        backup::before_forced_deploy(&client, solution, &projects, &backup::new_stamp())
            .map_err(ToolError::coded)?
    } else {
        None
    };
    let result = deploy::run(
        &client,
        &deploy::DiskBaseline::new(&solution.root),
        &profile,
        &projects,
        !dry_run,
        force,
        !only.is_empty(),
    );
    let plans = |report: &deploy::Report| -> Value {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for plan in &report.plans {
            let key = match &plan.decision {
                push::Decision::AlreadyThere => "unchanged",
                push::Decision::Create => "create",
                push::Decision::Update => "update",
                push::Decision::Refuse(_) => "refuse",
            };
            *counts.entry(key).or_default() += 1;
        }
        json!(counts)
    };
    let calls = |report: &deploy::Report| -> Vec<Value> {
        report
            .calls
            .iter()
            .map(|c| json!({ "project": c.project, "call": c.call.to_string(), "post_import": c.post_import, "skipped": c.skipped }))
            .collect()
    };
    Ok(match result {
        Ok(report) => {
            let mut value = json!({
                "ok": true,
                "dry_run": dry_run,
                "projects": notes,
                "scripts_parsed": report.scripts_checked,
                "entities": plans(&report),
                "calls": calls(&report),
            });
            if let Some(dir) = &saved {
                value["backup"] = json!(dir);
            }
            if !dry_run {
                value["imported"] = json!(report.imported);
                value["kept"] = json!(report.kept.len());
                value["changed_by_deploy_step"] = json!(report
                    .changed_by_deploy
                    .iter()
                    .map(|(c, n)| format!("{c}/{n}"))
                    .collect::<Vec<_>>());
            }
            if detail {
                value["plans"] = json!(report
                    .plans
                    .iter()
                    .map(|p| json!({ "entity": format!("{}/{}", p.collection, p.name), "decision": format!("{:?}", p.decision) }))
                    .collect::<Vec<_>>());
            }
            value
        }
        Err(deploy::DeployError::ParseFailed(failures)) => json!({
            "ok": false,
            "stage": "server script parse",
            "failures": failures
                .iter()
                .map(|f| json!({ "entity": f.entity, "service": f.service, "line": f.line, "column": f.column, "message": f.message }))
                .collect::<Vec<_>>(),
            "note": "nothing was imported",
        }),
        Err(deploy::DeployError::Conflicts(conflicts)) => json!({
            "ok": false,
            "stage": "conflict check",
            // A short reason per entity; the full sentence, with hashes, is detail.
            "refused": conflicts
                .iter()
                .map(|c| {
                    let (code, full) = match &c.decision {
                        push::Decision::Refuse(reason) => (refusal_code(reason), reason.to_string()),
                        other => ("unexpected", format!("{other:?}")),
                    };
                    let mut entry = json!({ "entity": format!("{}/{}", c.collection, c.name), "reason": code });
                    if detail {
                        entry["explanation"] = json!(full);
                    }
                    entry
                })
                .collect::<Vec<_>>(),
            "note": "nothing was imported. server-changed: the server moved since the last sync. \
                     no-baseline: the sides differ and nothing records which changed (see status). \
                     deleted-on-server: it was removed there. force: true overwrites all of these.",
        }),
        Err(deploy::DeployError::NotKept(report)) => json!({
            "ok": false,
            "stage": "read-back",
            "imported": report.imported,
            "not_kept": report
                .not_kept
                .iter()
                .map(|n| json!({ "entity": format!("{}/{}", n.collection, n.name), "sent": n.sent, "read_back": n.read_back, "error": n.error }))
                .collect::<Vec<_>>(),
        }),
        Err(error) => return Err(ToolError::coded(error)),
    })
}

fn refusal_code(refusal: &push::Refusal) -> &'static str {
    match refusal {
        push::Refusal::Conflict { .. } => "server-changed",
        push::Refusal::UnknownAncestor { .. } => "no-baseline",
        push::Refusal::DeletedOnServer => "deleted-on-server",
    }
}

fn push_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let name = required(arguments, "entity")?;
    let dry_run = flag(arguments, "dry_run", true);
    let force = flag(arguments, "force", false);
    let request = commands::push::PushRequest {
        entity: name.to_string(),
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        force,
        backup: flag(arguments, "backup", true),
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::push::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(|error| ToolError {
            code: error.code(),
            message: match error.backup() {
                Some(dir) => {
                    format!("{error}; the server's copy was saved to {dir} before the push")
                }
                None => error.to_string(),
            },
        })?;
    let label = match &outcome {
        commands::push::PushOutcome::Plan { entity, .. }
        | commands::push::PushOutcome::Applied { entity, .. } => entity.to_string(),
    };
    let saved = match &outcome {
        commands::push::PushOutcome::Applied { backup, .. } => backup.clone(),
        commands::push::PushOutcome::Plan { .. } => None,
    };
    let mut result = push_outcome_json(&label, dry_run, force, outcome);
    if let Some(dir) = saved {
        result["backup"] = json!(dir);
    }
    add_notices(&mut result, &notices);
    Ok(result)
}

/// What taking the workspace lock did (files swept, interrupted operations recovered), when it
/// did anything.
fn add_notices(result: &mut Value, notices: &commands::Notices) {
    if !notices.is_empty() {
        result["notices"] = json!(notices.lines());
    }
}

fn push_outcome_json(
    label: &str,
    dry_run: bool,
    force: bool,
    outcome: commands::push::PushOutcome,
) -> Value {
    match outcome {
        commands::push::PushOutcome::Plan { decision, .. } => {
            let (would, refusal) = match &decision {
                push::Decision::AlreadyThere => {
                    ("nothing: the server already has this version", None)
                }
                push::Decision::Create => ("create it on the server", None),
                push::Decision::Update => (
                    "update it; the server is unchanged since the last sync",
                    None,
                ),
                push::Decision::Refuse(refusal) => (
                    "refuse",
                    Some((refusal.to_string(), refusal.code().as_str())),
                ),
            };
            match refusal {
                Some((refusal, code)) => {
                    json!({ "entity": label, "dry_run": dry_run, "would": would, "refusal": refusal, "code": code, "force": force })
                }
                None => {
                    json!({ "entity": label, "dry_run": dry_run, "would": would, "refusal": null, "force": force })
                }
            }
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::AlreadyThere,
            ..
        } => {
            json!({ "entity": label, "dry_run": dry_run, "pushed": false, "note": "the server already has this version; baseline recorded" })
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::Pushed { created },
            ..
        } => {
            json!({ "entity": label, "dry_run": dry_run, "pushed": true, "created": created, "note": "read back and matching; baseline recorded" })
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::Refused(refusal),
            ..
        } => {
            json!({ "entity": label, "dry_run": dry_run, "pushed": false, "refusal": refusal.to_string(), "code": refusal.code().as_str(), "note": "nothing was sent; force: true pushes anyway" })
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::WouldDo(_),
            ..
        } => unreachable!("an applied outcome cannot be a plan"),
    }
}

fn entity_delete_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let (acknowledged, force_used) = entity_delete::acknowledged(
        flag(arguments, "force", false),
        flag(arguments, "allow_repository_defined", false),
        flag(arguments, "allow_outside_dependents", false),
        flag(arguments, "allow_file_repository_data_loss", false),
    );
    let request = commands::delete::EntityDeleteRequest {
        entities: strings(arguments, "entities"),
        renamed: flag(arguments, "renamed", false),
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        acknowledgements: acknowledged,
        legacy_force_used: force_used,
        backup: flag(arguments, "backup", true),
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::delete::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(|error| ToolError {
            code: error.code(),
            message: error.to_string(),
        })?;
    let (report, date, force_used) = match outcome {
        commands::delete::EntityDeleteOutcome::Plan {
            report,
            legacy_force_used,
            ..
        } => (report, None, legacy_force_used),
        commands::delete::EntityDeleteOutcome::Applied {
            report,
            date,
            legacy_force_used,
            ..
        } => (report, Some(date), legacy_force_used),
    };
    let mut result = json!({
        "ok": !report.failed(),
        "entities": report.entities,
        "dependency_limit": report.dependency_limit,
    });
    if let Some(dir) = &report.backup {
        result["backup"] = json!(dir);
    }
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    if report.ledger_changed {
        result["ledger_marked"] = json!(date.expect("an applied delete outcome has a date"));
    }
    if force_used {
        result["deprecated"] = json!(entity_delete::FORCE_DEPRECATION);
    }
    add_notices(&mut result, &notices);
    Ok(result)
}

fn move_member_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let action = required(arguments, "action")?;
    if !matches!(action, "move" | "copy") {
        return Err(ToolError::invalid(format!(
            "unknown action `{action}`; use `move` or `copy`"
        )));
    }
    let kind = required(arguments, "kind")?;
    let member = relocate::Member::from_word(kind).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown kind `{kind}`; use `service` or `property`"
        ))
    })?;
    let request = relocate::Request {
        member,
        copy: action == "copy",
        from: required(arguments, "from")?.to_string(),
        to: required(arguments, "to")?.to_string(),
        name: required(arguments, "name")?.to_string(),
        new_name: text(arguments, "new_name").map(str::to_string),
        leave_delegate: flag(arguments, "leave_delegate", false),
    };
    let _lock = if dry_run {
        None
    } else {
        Some(lock::acquire_for(solution, "mcp move_member").map_err(ToolError::coded)?)
    };
    let plan = relocate::plan(solution, &request).map_err(ToolError::coded)?;
    let mut problems = Vec::new();
    if !dry_run {
        let lock = _lock
            .as_ref()
            .ok_or_else(|| ToolError::invalid("applying needs the workspace lock"))?;
        relocate::apply(&plan, lock).map_err(ToolError::coded)?;
        problems = relocate::verify(solution, &plan);
    }
    let mut result = json!({
        "ok": problems.is_empty(), "action": action, "member": kind, "from": plan.request.from, "to": plan.request.to,
        "name": plan.request.name, "as": plan.final_name, "files": plan.files(solution),
        "callers": plan.callers, "notes": plan.notes, "out_of_step": problems,
    });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    Ok(result)
}

fn new_building_block_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let _lock = if dry_run {
        None
    } else {
        Some(lock::acquire_for(solution, "mcp new_building_block").map_err(ToolError::coded)?)
    };
    let reloaded;
    let solution = if dry_run {
        solution
    } else {
        reloaded = Solution::load(&solution.root.join(crate::core::config::CONFIG_FILE))
            .map_err(ToolError::coded)?;
        &reloaded
    };
    let kind_word = text(arguments, "type").unwrap_or("standard");
    let kind = newblock::BlockType::from_word(kind_word).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown type `{kind_word}`; use standard, abstract or implementation"
        ))
    })?;
    let request = newblock::Request {
        name: required(arguments, "name")?.to_string(),
        kind,
        display_name: text(arguments, "display_name").map(str::to_string),
        description: text(arguments, "description")
            .unwrap_or_default()
            .to_string(),
        parent: text(arguments, "parent").map(str::to_string),
        model_logic: flag(arguments, "model_logic", false),
        management_shape: flag(arguments, "management_shape", true),
        root: text(arguments, "root").map(str::to_string),
        base_extension: text(arguments, "base_extension").map(str::to_string),
    };
    let plan = newblock::plan(solution, &request).map_err(ToolError::coded)?;
    if !dry_run {
        let lock = _lock
            .as_ref()
            .ok_or_else(|| ToolError::invalid("applying needs the workspace lock"))?;
        newblock::apply(solution, &plan, lock).map_err(ToolError::coded)?;
    }
    let files: Vec<String> = plan
        .files
        .iter()
        .map(|file| {
            file.path
                .strip_prefix(&solution.root)
                .unwrap_or(&file.path)
                .display()
                .to_string()
                .replace('\\', "/")
        })
        .collect();
    let mut result = json!({
        "ok": true, "name": request.name, "type": request.kind.word(), "root": plan.root,
        "files": files, "twaco_toml": plan.config_addition, "notes": plan.notes,
    });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    Ok(result)
}

fn retemplate_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let request = retemplate::Request {
        entity: required(arguments, "entity")?.to_string(),
        template: text(arguments, "template").map(str::to_string),
        add_shapes: strings(arguments, "add_shapes"),
        remove_shapes: strings(arguments, "remove_shapes"),
        accept_loss: flag(arguments, "accept_loss", false),
    };
    let _lock = if dry_run {
        None
    } else {
        Some(lock::acquire_for(solution, "mcp retemplate").map_err(ToolError::coded)?)
    };
    let plan = retemplate::plan(solution, &request).map_err(ToolError::coded)?;
    if !dry_run {
        retemplate::apply(&plan).map_err(ToolError::coded)?;
    }
    let mut result = json!({
        "ok": true, "entity": plan.request.entity, "collection": plan.collection, "file": plan.file_relative(solution),
        "affected": plan.affected, "gained": plan.gained, "lost": plan.lost,
        "needs_accept_loss": plan.blocked, "notes": plan.notes,
    });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    Ok(result)
}

fn entity_restore_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let Some(id) = text(arguments, "set").filter(|id| !id.is_empty()) else {
        let sets = backup::list(solution);
        return Ok(json!({ "ok": true, "sets": sets.iter().map(|set| json!({
            "id": set.id, "created": set.manifest.created, "reason": set.manifest.reason,
            "entities": set.manifest.entities.iter().map(|item| format!("{}/{}", item.collection, item.name)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>() }));
    };
    let dry_run = flag(arguments, "dry_run", true);
    let set = backup::find(solution, id).map_err(ToolError::coded)?;
    let client = client(solution, arguments)?;
    let report = backup::restore(&client, &set, &strings(arguments, "entities"), !dry_run)
        .map_err(ToolError::coded)?;
    let failed = report
        .iter()
        .any(|entry| entry.status == backup::Status::Failed);
    let mut result = json!({ "ok": !failed, "set": set.id, "entities": report });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    Ok(result)
}

fn entity_carry_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let renamed = flag(arguments, "renamed", false);
    let pairs =
        entity_carry::pairs_from_names(&strings(arguments, "pairs")).map_err(ToolError::coded)?;
    let _lock = if !dry_run && renamed {
        Some(lock::acquire_for(solution, "mcp entity_carry").map_err(ToolError::coded)?)
    } else {
        None
    };
    let client = client(solution, arguments)?;
    let date = jiff::Zoned::now().strftime("%Y-%m-%d").to_string();
    let request = entity_carry::Request {
        pairs,
        renamed,
        apply: !dry_run,
        detail: flag(arguments, "detail", false),
    };
    let report = entity_carry::run(&client, solution, &request, &date).map_err(ToolError::coded)?;
    let failed = !dry_run
        && report
            .entities
            .iter()
            .any(|entity| entity.status == entity_carry::Status::Failed);
    let mut result = json!({ "ok": !failed, "entities": report.entities });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    if report.ledger_changed {
        result["ledger_marked"] = json!(date);
    }
    Ok(result)
}

fn datatable_copy_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let name = |key: &str| {
        text(arguments, key)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ToolError::invalid(format!("`{key}` is required")))
    };
    let mut map = std::collections::BTreeMap::new();
    for (from, to) in arguments
        .get("map")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let to = to
            .as_str()
            .ok_or_else(|| ToolError::invalid(format!("map.{from} must be a field name")))?;
        map.insert(from.clone(), to.to_string());
    }
    let max_rows = match arguments.get("max_rows") {
        None => 100_000,
        Some(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| ToolError::invalid("`max_rows` must be a positive whole number"))?,
    };
    let request = datatable_copy::Request {
        old: name("old")?,
        new: name("new")?,
        map,
        drop_unmapped: flag(arguments, "drop_unmapped", false),
        append: flag(arguments, "append", false),
        max_rows,
        apply: !dry_run,
    };
    let client = client(solution, arguments)?;
    let report = datatable_copy::run(&client, solution, &request).map_err(ToolError::coded)?;
    let mut result = json!({ "ok": true, "report": report });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    Ok(result)
}

fn db_clean_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let client = client(solution, arguments)?;
    let swept = db::sweep(&client, !dry_run).map_err(ToolError::coded)?;
    let failed = swept
        .iter()
        .any(|thing| thing.status == db::SweepStatus::Failed);
    let mut result = json!({ "ok": !failed, "things": swept });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    Ok(result)
}

fn db_tool(solution: &Solution, arguments: &Value, mode: db::Mode) -> Result<Value, ToolError> {
    let sql = match (text(arguments, "file"), text(arguments, "sql")) {
        (Some(file), None) if !file.is_empty() => {
            let candidate = solution.root.join(file);
            let real = std::fs::canonicalize(&candidate)
                .map_err(|error| ToolError::with(ErrorCode::IoError, format!("{file}: {error}")))?;
            let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
            if !real.starts_with(&root) {
                return Err(ToolError::invalid(format!(
                    "{file} is outside the solution"
                )));
            }
            std::fs::read_to_string(&real)
                .map_err(|error| ToolError::with(ErrorCode::IoError, format!("{file}: {error}")))?
        }
        (None, Some(sql)) if !sql.is_empty() => sql.to_string(),
        _ => return Err(ToolError::invalid("give exactly one of `file` or `sql`")),
    };
    let positive = |name: &str, default: u64| -> Result<u64, ToolError> {
        match arguments.get(name) {
            None => Ok(default),
            Some(value) => value.as_u64().filter(|value| *value > 0).ok_or_else(|| {
                ToolError::invalid(format!("`{name}` must be a positive whole number"))
            }),
        }
    };
    let timeout = positive("timeout", 120)?;
    let max_rows = positive("max_rows", 500)?;
    let profile_name = text(arguments, "profile").unwrap_or("default");
    let selected = profile::load(&solution.root, profile_name).map_err(ToolError::coded)?;
    let client = server::Client::new(selected.clone());
    let options = db::Options {
        mode,
        thing: text(arguments, "thing").map(str::to_string),
        apply: mode == db::Mode::Query || !flag(arguments, "dry_run", true),
        no_transaction: flag(arguments, "no_transaction", false),
        max_rows,
        timeout: Duration::from_secs(timeout),
    };
    let report =
        db::execute(&client, solution, &selected, &sql, &options).map_err(ToolError::coded)?;
    let mut value = serde_json::to_value(report).expect("db report serialises");
    if mode == db::Mode::Query {
        if let Some(result) = value.get_mut("result").and_then(Value::as_object_mut) {
            let total = result
                .get("rows")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let columns: Vec<String> = result
                .get("dataShape")
                .and_then(|shape| shape.get("fieldDefinitions"))
                .and_then(Value::as_object)
                .map(|fields| fields.keys().cloned().collect())
                .or_else(|| {
                    result
                        .get("rows")
                        .and_then(Value::as_array)
                        .and_then(|rows| rows.first())
                        .and_then(Value::as_object)
                        .map(|row| row.keys().cloned().collect())
                })
                .unwrap_or_default();
            if !flag(arguments, "detail", false) {
                if let Some(rows) = result.get_mut("rows").and_then(Value::as_array_mut) {
                    rows.truncate(20);
                }
            }
            result.insert("total_rows".to_string(), json!(total));
            result.insert("columns".to_string(), json!(columns));
        }
    }
    Ok(value)
}

fn adopt_apply_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let export = required(arguments, "export")?;
    let export = {
        let path = PathBuf::from(export);
        if path.is_absolute() {
            path
        } else {
            solution.root.join(path)
        }
    };
    let only: Vec<String> = arguments
        .get("entity")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let _lock = lock::acquire_for(solution, "mcp adopt_apply").map_err(ToolError::coded)?;
    let report = adopt::compare(solution, &export, &only).map_err(ToolError::coded)?;
    let outcome = adopt::apply(solution, &export, &report).map_err(ToolError::coded)?;
    let mut result = json!({
        "applied": outcome.lines,
        "reverts_not_applied": report.reverts().count(),
        "next": "run sync, then check",
    });
    add_types_refresh(&mut result, &outcome.types);
    Ok(result)
}

fn adopt_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let export = required(arguments, "export")?;
    let export = {
        let path = PathBuf::from(export);
        if path.is_absolute() {
            path
        } else {
            solution.root.join(path)
        }
    };
    let only: Vec<String> = arguments
        .get("entity")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let report = adopt::compare(solution, &export, &only).map_err(ToolError::coded)?;
    let detail = flag(arguments, "detail", false);
    let services: Vec<Value> = report
        .services
        .iter()
        .map(|s| {
            json!({
                "entity": s.entity,
                "service": s.service,
                "generated": s.generated,
                "compared_with": relative(solution, &s.source),
            })
        })
        .collect();
    let changed: Vec<Value> = report
        .with_status(adopt::Status::Changed)
        .map(|e| {
            let mut entry = json!({
                "entity": e.entity.path(),
                "nodes": e.differences.len(),
                "regenerated_ids": e.volatile_ids,
                "ignored": e.ignored,
            });
            if detail {
                entry["differences"] = json!(e
                    .differences
                    .iter()
                    .map(|d| json!({ "path": d.path, "export": d.export, "repo": d.repo }))
                    .collect::<Vec<_>>());
            }
            entry
        })
        .collect();
    Ok(json!({
        "reverts": report.reverts().count(),
        "services": services,
        "unmatched_services": report.unmatched_services,
        "new": report.with_status(adopt::Status::New).map(|e| json!({ "entity": e.entity.path(), "project": e.project })).collect::<Vec<_>>(),
        "absent": report.absent.iter().map(adopt::EntityRef::path).collect::<Vec<_>>(),
        "changed": changed,
        "identical": report.with_status(adopt::Status::Identical).count(),
    }))
}

fn rename_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let word = required(arguments, "kind")?;
    let kind = rename::Kind::from_word(word).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown rename kind `{word}`; use {}",
            rename::Kind::list_words()
        ))
    })?;
    let dry_run = flag(arguments, "dry_run", true);
    let request = rename::Request {
        kind,
        scope: text(arguments, "scope").map(str::to_string),
        service: text(arguments, "service").map(str::to_string),
        old: required(arguments, "old")?.to_string(),
        new: required(arguments, "new")?.to_string(),
        apply: !dry_run,
        include_outside: flag(arguments, "include_outside", false),
        skip_checks: flag(arguments, "skip_checks", false),
        expect_digest: text(arguments, "plan_digest").map(str::to_string),
        database: rename::DatabaseFlags {
            sql: flag(arguments, "sql", false),
            no_sql: flag(arguments, "no_sql", false),
            dir: text(arguments, "sql_dir").map(str::to_string),
        },
    };
    let date = jiff::Zoned::now().strftime("%Y-%m-%d").to_string();
    let (spec, options) = request
        .build(&solution.root, &date)
        .map_err(ToolError::invalid)?;
    let _lock = if dry_run {
        None
    } else {
        Some(lock::acquire_for(solution, "mcp rename").map_err(ToolError::coded)?)
    };
    let outcome =
        rename::run(solution, &spec, &options, _lock.as_ref()).map_err(ToolError::coded)?;
    Ok(rename::summary_json(
        solution,
        &outcome,
        options.include_outside,
        10,
    ))
}

fn config_table_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let thing_arg = required(arguments, "thing")?;
    let table = required(arguments, "table")?;
    let action = text(arguments, "action").unwrap_or("read");
    // As on the command line: a Thing of the solution may be named by its last segment, one not
    // in the solution is taken as given (diff aside), and anything else is refused.
    let found = workspace::discover(solution).entities;
    let resolved = match workspace::resolve(&found, thing_arg) {
        Ok(entity) if entity.info.collection == "Things" => Some(entity),
        Ok(entity) => {
            return Err(ToolError::invalid(format!(
                "{} is a {}, and only a Thing has configuration tables here",
                entity.info.name, entity.info.collection
            )))
        }
        Err(workspace::WorkspaceError::UnknownEntity { .. }) => None,
        Err(error) => return Err(ToolError::coded(error)),
    };
    let thing = resolved
        .map(|e| e.info.name.clone())
        .unwrap_or_else(|| thing_arg.to_string());
    let client = client(solution, arguments)?;
    if action == "restore" {
        let backup = required(arguments, "backup")?;
        let backup = {
            let path = PathBuf::from(backup);
            if path.is_absolute() {
                path
            } else {
                solution.root.join(path)
            }
        };
        let saved = config_table::read_backup(&backup, &thing, table).map_err(ToolError::coded)?;
        let dry_run = flag(arguments, "dry_run", true);
        let plan = config_table::restore(&client, &thing, table, &saved, !dry_run)
            .map_err(ToolError::coded)?;
        return Ok(json!({
            "thing": thing,
            "table": table,
            "dry_run": dry_run,
            "rows_written": plan.writes,
            "rows_removed": plan.deletes,
            "note": if dry_run { "nothing was written; pass dry_run: false to restore" } else { "restored and read back" },
        }));
    }
    let live = config_table::fetch(&client, &thing, table).map_err(ToolError::coded)?;
    let key = config_table::primary_key(&live.data_shape);
    match action {
        "read" => {
            let detail = flag(arguments, "detail", false);
            let mut result = json!({
                "thing": thing,
                "table": table,
                "rows": live.rows.len(),
                "primary_key": key,
            });
            result[if detail { "values" } else { "first_rows" }] = json!(live
                .rows
                .iter()
                .take(if detail { usize::MAX } else { 3 })
                .collect::<Vec<_>>());
            Ok(result)
        }
        "diff" => {
            let entity = resolved.ok_or_else(|| {
                ToolError::with(
                    ErrorCode::UnknownEntity,
                    format!("{thing_arg} is not an entity of this solution"),
                )
            })?;
            let src = std::fs::read(&entity.path).map_err(|e| {
                ToolError::with(
                    ErrorCode::IoError,
                    format!("{}: {e}", entity.path.display()),
                )
            })?;
            let repository =
                config_table::repository_rows(&src, table).map_err(ToolError::coded)?;
            let differences = config_table::differences(
                "server",
                &live.rows,
                "source control",
                &repository,
                &key,
            );
            Ok(json!({
                "thing": thing,
                "table": table,
                "identical": differences.is_empty(),
                "rows": live.rows.len(),
                "differences": differences,
            }))
        }
        other => Err(ToolError::invalid(format!(
            "action must be read, diff or restore, not {other:?}"
        ))),
    }
}

/// The server's file repositories, read-only.
fn repo_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let client = client(solution, arguments)?;
    let action = text(arguments, "action").unwrap_or("list");
    if action == "list" {
        let names = repo::Remote::repositories(&client).map_err(ToolError::coded)?;
        return Ok(json!({ "ok": true, "repositories": names }));
    }
    let repository = required(arguments, "repository")?;
    match action {
        "ls" => {
            let folder = text(arguments, "path").unwrap_or("/");
            let listing = repo::list(
                &client,
                repository,
                folder,
                flag(arguments, "recursive", false),
            )
            .map_err(ToolError::coded)?;
            let shown = if flag(arguments, "detail", false) {
                usize::MAX
            } else {
                200
            };
            let mut result = json!({
                "ok": true,
                "repository": repository,
                "folders": listing.folders,
                "files": listing.files.iter().take(shown).map(|file| json!({
                    "path": file.path,
                    "size": file.size,
                    "modified": logs::iso(file.modified),
                })).collect::<Vec<_>>(),
                "file_count": listing.files.len(),
            });
            if listing.files.len() > shown {
                result["note"] = json!(format!(
                    "{} files in all; detail: true lists every one",
                    listing.files.len()
                ));
            }
            Ok(result)
        }
        "get" => {
            let path = required(arguments, "path")?;
            let bytes = repo::get(&client, repository, path).map_err(ToolError::coded)?;
            let max = arguments
                .get("max_chars")
                .and_then(Value::as_u64)
                .unwrap_or(100_000) as usize;
            let digest = repo::sha256_hex(&bytes);
            match std::str::from_utf8(&bytes) {
                Ok(text) => {
                    let total = text.chars().count();
                    let mut result = json!({
                        "ok": true,
                        "repository": repository,
                        "path": repo::remote_path(path).map_err(ToolError::coded)?,
                        "size": bytes.len(),
                        "sha256": digest,
                        "text": text.chars().take(max).collect::<String>(),
                    });
                    if total > max {
                        result["truncated"] = json!(true);
                        result["note"] = json!(format!(
                            "{total} characters in all; raise max_chars, or `twaco repo get --out`"
                        ));
                    }
                    Ok(result)
                }
                Err(_) => Ok(json!({
                    "ok": true,
                    "repository": repository,
                    "path": repo::remote_path(path).map_err(ToolError::coded)?,
                    "size": bytes.len(),
                    "sha256": digest,
                    "binary": true,
                    "note": "not text, so its bytes are not returned here; `twaco repo get <repo> <path> --out <file>` saves it",
                })),
            }
        }
        "status" => {
            let local = repo::local_root(
                &solution.root,
                solution.repositories.root.as_deref(),
                repository,
            );
            let compared = repo::status(&client, repository, &local).map_err(ToolError::coded)?;
            let mut counts = std::collections::BTreeMap::new();
            for item in &compared {
                *counts.entry(item.state.label()).or_insert(0usize) += 1;
            }
            let detail = flag(arguments, "detail", false);
            let listed: Vec<Value> = compared
                .iter()
                .filter(|item| detail || item.state != repo::State::Same)
                .map(|item| json!({ "path": item.path, "state": item.state.label(), "local_size": item.local_size, "remote_size": item.remote_size }))
                .collect();
            Ok(json!({
                "ok": true,
                "repository": repository,
                "local": local.display().to_string(),
                "counts": counts,
                "in_sync": compared.iter().all(|item| item.state == repo::State::Same),
                (if detail { "files" } else { "attention" }): listed,
            }))
        }
        other => Err(ToolError::invalid(format!(
            "action must be list, ls, get or status, not {other:?}"
        ))),
    }
}

/// An import into the server, a plan unless dry_run is false.
fn import_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let client = client(solution, arguments)?;
    let dry_run = flag(arguments, "dry_run", true);
    let (properties, tables) = (
        flag(arguments, "overwrite_properties", false),
        flag(arguments, "overwrite_tables", false),
    );
    let differs = |list: &[imports::Differs]| -> Vec<Value> {
        list.iter()
            .map(|d| json!({ "type": d.entity_type, "name": d.name, "what": d.what }))
            .collect()
    };
    match required(arguments, "action")? {
        "file" => {
            let relative = required(arguments, "file")?;
            let real = std::fs::canonicalize(solution.root.join(relative))
                .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{relative}: {e}")))?;
            let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
            if !real.starts_with(&root) {
                return Err(ToolError::invalid(format!(
                    "{relative} is outside the solution"
                )));
            }
            let bytes = std::fs::read(&real)
                .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{relative}: {e}")))?;
            let file_name = real
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "import.xml".into());
            let plan =
                imports::import_file(&client, &file_name, &bytes, properties, tables, !dry_run)
                    .map_err(ToolError::coded)?;
            let names = |list: &[(String, String)]| {
                list.iter()
                    .map(|(c, n)| format!("{c}/{n}"))
                    .collect::<Vec<_>>()
            };
            Ok(json!({
                "ok": true,
                "dry_run": dry_run,
                "adds": names(&plan.new),
                "replaces": names(&plan.replaced),
                "applied": plan.applied,
            }))
        }
        "source_control" => {
            let imported = imports::import_source_control(
                &client,
                required(arguments, "repository")?,
                required(arguments, "path")?,
                properties,
                tables,
                !dry_run,
            )
            .map_err(ToolError::coded)?;
            let mut result = json!({ "ok": true, "dry_run": dry_run, "entities": imported.total, "differ": differs(&imported.differ) });
            if let Some(after) = &imported.still_differ {
                result["still_differ"] = json!(differs(after));
            }
            Ok(result)
        }
        other => Err(ToolError::invalid(format!(
            "action must be file or source_control, not {other:?}"
        ))),
    }
}

/// An export from the server, to a file inside the solution or into a repository.
fn export_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let client = client(solution, arguments)?;
    let action = required(arguments, "action")?;
    if action == "source_control" {
        let filters = export::Filters {
            project: text(arguments, "project").map(str::to_string),
            collection: text(arguments, "collection").map(str::to_string),
            tags: text(arguments, "tags").map(str::to_string),
            include_dependents: flag(arguments, "with_dependents", false),
        };
        let dry_run = flag(arguments, "dry_run", true);
        let (plan, link) = export::source_control(
            &client,
            required(arguments, "repository")?,
            required(arguments, "path")?,
            &filters,
            text(arguments, "zip"),
            !dry_run,
        )
        .map_err(ToolError::coded)?;
        return Ok(json!({ "ok": true, "dry_run": dry_run, "change": plan, "download": link }));
    }
    let what = match action {
        "entity" => {
            export::What::entity(required(arguments, "entity")?).map_err(ToolError::coded)?
        }
        "collection" => export::What::Collection {
            collection: required(arguments, "collection")?.to_string(),
            project: text(arguments, "project").map(str::to_string),
        },
        "project" => export::What::Project {
            project: required(arguments, "project")?.to_string(),
        },
        other => {
            return Err(ToolError::invalid(format!(
                "action must be entity, collection, project or source_control, not {other:?}"
            )))
        }
    };
    let relative = required(arguments, "out")?;
    let out = out_path(solution, relative, flag(arguments, "overwrite", false))?;
    let exported = export::export(&client, &what).map_err(ToolError::coded)?;
    if let Some(folder) = out.parent() {
        std::fs::create_dir_all(folder).map_err(|e| {
            ToolError::with(ErrorCode::IoError, format!("{}: {e}", folder.display()))
        })?;
    }
    workspace::write_entity(&out, &exported.xml).map_err(ToolError::coded)?;
    Ok(json!({
        "ok": true,
        "out": relative,
        "bytes": exported.xml.len(),
        "entities": exported.counts.iter().map(|(c, n)| json!({ "collection": c, "count": n })).collect::<Vec<_>>(),
    }))
}

/// A file to write, given relative to the solution: a plain path whose nearest existing folder
/// is really inside the solution, and not an existing file unless overwriting.
fn out_path(
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

/// The repository packaged for release, offline, into a file inside the solution.
fn package_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    use crate::core::package;
    let relative = required(arguments, "out")?;
    let out = out_path(solution, relative, flag(arguments, "overwrite", false))?;
    let project = text(arguments, "project");
    let (bytes, detail) = match required(arguments, "action")? {
        "bundle" => {
            let part = match text(arguments, "part").unwrap_or("all") {
                "all" => package::Part::All,
                "backend" => package::Part::Backend,
                "frontend" => package::Part::Frontend,
                other => {
                    return Err(ToolError::invalid(format!(
                        "part must be all, backend or frontend, not {other:?}"
                    )))
                }
            };
            let built = package::bundle(solution, project, part).map_err(ToolError::coded)?;
            let count: usize = built.entities.values().sum();
            (
                built.bytes,
                json!({ "entities": count, "files": built.files }),
            )
        }
        "source_control" => {
            let (bytes, count) =
                package::source_control(solution, project).map_err(ToolError::coded)?;
            (bytes, json!({ "entities": count }))
        }
        "extension" => {
            let meta = package::Metadata::from_solution(solution);
            let editable = flag(arguments, "editable", false);
            match project {
                Some(project) => {
                    let (bytes, count) = package::extension(solution, project, editable, &meta)
                        .map_err(ToolError::coded)?;
                    (
                        bytes,
                        json!({ "editable": editable, "version": meta.version, "entities": count }),
                    )
                }
                None => {
                    let (bytes, counts) = package::solution_extensions(solution, editable, &meta)
                        .map_err(ToolError::coded)?;
                    let projects: Vec<Value> = counts
                        .iter()
                        .map(|(p, n)| json!({ "project": p, "entities": n }))
                        .collect();
                    (
                        bytes,
                        json!({ "editable": editable, "version": meta.version, "projects": projects }),
                    )
                }
            }
        }
        other => {
            return Err(ToolError::invalid(format!(
                "action must be bundle, source_control or extension, not {other:?}"
            )))
        }
    };
    if let Some(folder) = out.parent() {
        std::fs::create_dir_all(folder).map_err(|e| {
            ToolError::with(ErrorCode::IoError, format!("{}: {e}", folder.display()))
        })?;
    }
    workspace::write_entity(&out, &bytes).map_err(ToolError::coded)?;
    Ok(json!({ "ok": true, "out": relative, "bytes": bytes.len(), "detail": detail }))
}

/// The server's subsystem settings, read-only, secrets hidden.
fn settings_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let client = client(solution, arguments)?;
    match text(arguments, "action").unwrap_or("list") {
        "list" => {
            let all = settings::summaries(&client).map_err(ToolError::coded)?;
            Ok(json!({ "ok": true, "subsystems": all.iter().map(|s| json!({
                "name": s.name,
                "running": s.running,
                "tables": s.tables,
            })).collect::<Vec<_>>() }))
        }
        "show" => {
            let names = settings::Remote::subsystems(&client).map_err(ToolError::coded)?;
            let name = settings::resolve(&names, required(arguments, "subsystem")?)
                .map_err(ToolError::coded)?;
            let read = settings::read(&client, name).map_err(ToolError::coded)?;
            Ok(
                json!({ "ok": true, "subsystem": read.name, "running": read.running, "tables": read.tables.iter().map(|t| settings::table_json(&read.name, t)).collect::<Vec<_>>() }),
            )
        }
        "search" => {
            let wanted = required(arguments, "text")?;
            let all = settings::read_all(&client).map_err(ToolError::coded)?;
            let found = settings::search(&all, wanted);
            Ok(json!({ "ok": true, "matches": found.iter().map(|f| json!({
                "subsystem": f.subsystem,
                "table": f.table,
                "setting": f.field.name,
                "type": f.field.base_type,
                "values": f.values,
                "description": f.field.description,
            })).collect::<Vec<_>>() }))
        }
        other => Err(ToolError::invalid(format!(
            "action must be list, show or search, not {other:?}"
        ))),
    }
}

/// The repository-derived service catalog, offline and read-only.
fn unused_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let min = match text(arguments, "min_confidence") {
        None => Confidence::Review,
        Some(word) => Confidence::parse(word).ok_or_else(|| {
            ToolError::invalid(format!(
                "`min_confidence` is structural, resolved or review, not {word:?}"
            ))
        })?,
    };
    let report = unused::run(
        solution,
        &unused::Request {
            min,
            collection: text(arguments, "collection").map(str::to_string),
        },
    );
    Ok(report.to_json(flag(arguments, "detail", false)))
}

fn docs_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let detail = flag(arguments, "detail", false);
    let document = docs::build(solution);
    Ok(json!({
        "document": document.to_json(detail),
        "markdown": docs::render_markdown(&document, detail),
    }))
}

fn impact_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let min = match text(arguments, "min_confidence") {
        None => Confidence::Review,
        Some(word) => Confidence::parse(word).ok_or_else(|| {
            ToolError::invalid(format!(
                "`min_confidence` is structural, resolved or review, not {word:?}"
            ))
        })?,
    };
    let depth = match arguments.get("depth") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|depth| *depth > 0)
                .and_then(|depth| usize::try_from(depth).ok())
                .ok_or_else(|| ToolError::invalid("`depth` must be a positive whole number"))?,
        ),
    };
    let report = impact::run(
        solution,
        &impact::Request {
            entity: required(arguments, "entity")?.to_string(),
            member: text(arguments, "member").map(str::to_string),
            min,
            depth,
        },
    )
    .map_err(ToolError::coded)?;
    if text(arguments, "format") == Some("dot") {
        return Ok(json!({
            "entity": report.entity,
            "dot": impact::render_dot(&report),
            "complete": report.complete,
            "unreadable": report.unreadable,
            "unparsed_scripts": report.unparsed_scripts,
            "limits": report.limits,
        }));
    }
    Ok(report.to_json(flag(arguments, "detail", false)))
}

fn catalog_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let catalog = catalog::build(
        solution,
        catalog::Query {
            entity: text(arguments, "entity"),
            project: text(arguments, "project"),
            text: text(arguments, "text"),
        },
    )
    .map_err(ToolError::coded)?;
    let service_count = catalog.service_count();
    let entity_count = catalog.entities.len();
    let limit = if flag(arguments, "detail", false) {
        usize::MAX
    } else {
        50
    };
    let mut services = Vec::new();
    for entity in &catalog.entities {
        for service in &entity.services {
            if services.len() == limit {
                break;
            }
            let mut value = serde_json::to_value(service).expect("catalog services serialise");
            let object = value
                .as_object_mut()
                .expect("a catalog service is an object");
            object.insert("collection".to_string(), json!(entity.collection));
            object.insert("entity".to_string(), json!(entity.name));
            object.insert("project".to_string(), json!(entity.project));
            if !entity.inherits.is_empty() {
                object.insert("inherits".to_string(), json!(entity.inherits));
            }
            if !entity.implemented_by.is_empty() {
                object.insert("implemented_by".to_string(), json!(entity.implemented_by));
            }
            services.push(value);
        }
        if services.len() == limit {
            break;
        }
    }
    let mut result = json!({
        "ok": true,
        "service_count": service_count,
        "entity_count": entity_count,
        "services": services,
    });
    if service_count > limit {
        result["note"] = json!(format!(
            "showing 50 of {service_count} services; detail: true lists every one"
        ));
    }
    if !catalog.skipped.is_empty() {
        result["skipped"] = json!(catalog.skipped);
    }
    Ok(result)
}

/// The server's extension packages, read-only.
fn extensions_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let client = client(solution, arguments)?;
    let package_json = |p: &extensions::Package| json!({ "name": p.name, "version": p.version, "vendor": p.vendor, "description": p.description, "minimumThingWorxVersion": p.minimum_thingworx });
    match text(arguments, "action").unwrap_or("list") {
        "list" => {
            let packages = extensions::list(&client).map_err(ToolError::coded)?;
            Ok(
                json!({ "ok": true, "packages": packages.iter().map(package_json).collect::<Vec<_>>() }),
            )
        }
        "show" => {
            let shown = extensions::show(&client, required(arguments, "package")?)
                .map_err(ToolError::coded)?;
            Ok(
                json!({ "ok": true, "package": package_json(&shown.package), "extensions": shown.extensions, "in_use": shown.in_use }),
            )
        }
        other => Err(ToolError::invalid(format!(
            "action must be list or show, not {other:?}"
        ))),
    }
}

/// Import or remove an extension package, a plan unless dry_run is false.
fn extension_write_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let client = client(solution, arguments)?;
    match required(arguments, "action")? {
        "import" => {
            let relative = required(arguments, "zip")?;
            // A zip of the solution, never one outside it.
            let real = std::fs::canonicalize(solution.root.join(relative))
                .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{relative}: {e}")))?;
            let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
            if !real.starts_with(&root) {
                return Err(ToolError::invalid(format!(
                    "{relative} is outside the solution"
                )));
            }
            let zip = std::fs::read(&real)
                .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{relative}: {e}")))?;
            let file_name = real
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "package.zip".into());
            let imported = extensions::import(&client, &file_name, &zip, !dry_run)
                .map_err(ToolError::coded)?;
            Ok(json!({
                "ok": true,
                "dry_run": dry_run,
                "change": imported.plan,
                "applied": imported.applied,
                "note": if imported.applied { "installed; the package list shows it" } else { "the server validated it and installed nothing; pass dry_run: false" },
            }))
        }
        "remove" => {
            let plan = extensions::remove(&client, required(arguments, "package")?, !dry_run)
                .map_err(ToolError::coded)?;
            Ok(json!({ "ok": true, "dry_run": dry_run, "change": plan, "applied": !dry_run }))
        }
        other => Err(ToolError::invalid(format!(
            "action must be import or remove, not {other:?}"
        ))),
    }
}

/// One change to a file repository, a plan unless dry_run is false.
fn repo_write_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let repository = required(arguments, "repository")?;
    let dry_run = flag(arguments, "dry_run", true);
    if let Some(direction) = text(arguments, "action").filter(|a| *a == "push" || *a == "pull") {
        let way = if direction == "push" {
            repo::Direction::Push
        } else {
            repo::Direction::Pull
        };
        // A pull writes the solution's tree, so it holds the workspace lock from the start.
        let _lock = if way == repo::Direction::Pull && !dry_run {
            Some(lock::acquire_for(solution, "mcp repo pull").map_err(ToolError::coded)?)
        } else {
            None
        };
        let local = repo::local_root(
            &solution.root,
            solution.repositories.root.as_deref(),
            repository,
        );
        let client = client(solution, arguments)?;
        let synced = repo::sync(
            &client,
            repository,
            &local,
            way,
            flag(arguments, "overwrite", false),
            !dry_run,
        )
        .map_err(ToolError::coded)?;
        let mut result = json!({
            "ok": true,
            "repository": repository,
            "local": local.display().to_string(),
            "dry_run": dry_run,
            "copied": synced.copied,
            "same": synced.same,
            "left_alone": synced.left,
        });
        if dry_run && !synced.copied.is_empty() {
            result["note"] = json!("nothing was copied; pass dry_run: false");
        }
        return Ok(result);
    }
    let path = repo::remote_path(required(arguments, "path")?).map_err(ToolError::coded)?;
    let overwrite = flag(arguments, "overwrite", false);
    let change = match required(arguments, "action")? {
        "put" => {
            let bytes = match (text(arguments, "text"), text(arguments, "local")) {
                (Some(_), Some(_)) => {
                    return Err(ToolError::invalid("give text or local, not both"))
                }
                (Some(content), None) => content.as_bytes().to_vec(),
                (None, Some(local)) => {
                    // A file of the solution, never one outside it.
                    let candidate = solution.root.join(local);
                    let real = std::fs::canonicalize(&candidate).map_err(|e| {
                        ToolError::with(ErrorCode::IoError, format!("{local}: {e}"))
                    })?;
                    let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
                    if !real.starts_with(&root) {
                        return Err(ToolError::invalid(format!(
                            "{local} is outside the solution"
                        )));
                    }
                    std::fs::read(&real)
                        .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{local}: {e}")))?
                }
                (None, None) => return Err(ToolError::invalid("put needs text or local")),
            };
            repo::Change::Put {
                path,
                bytes,
                overwrite,
            }
        }
        "mkdir" => repo::Change::Mkdir { path },
        "rm" if path == "/" => {
            return Err(ToolError::invalid("the repository root cannot be deleted"))
        }
        "rm" => repo::Change::Remove {
            path,
            recursive: flag(arguments, "recursive", false),
        },
        "mv" => {
            let to = repo::remote_path(required(arguments, "to")?).map_err(ToolError::coded)?;
            repo::Change::Move {
                from: path,
                to,
                overwrite,
            }
        }
        other => {
            return Err(ToolError::invalid(format!(
                "action must be put, mkdir, rm or mv, not {other:?}"
            )))
        }
    };
    let client = client(solution, arguments)?;
    let planned = repo::change(&client, repository, &change, !dry_run).map_err(ToolError::coded)?;
    let mut result = json!({
        "ok": true,
        "repository": repository,
        "change": planned.plan,
        "dry_run": dry_run,
        "applied": planned.applied,
    });
    if planned.nothing {
        result["note"] = json!("nothing to do");
    } else if !planned.applied {
        result["note"] = json!("nothing was sent; pass dry_run: false");
    }
    Ok(result)
}

/// The help version for a call: asked, named in a page address, configured, or the server's.
fn help_version(
    root: &Path,
    arguments: &Value,
    named: Option<String>,
) -> Result<(String, Vec<String>), ToolError> {
    // No twaco.toml means no solution; a broken one is an error.
    let solution = match Solution::discover(root) {
        Ok(solution) => Some(solution),
        Err(crate::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => return Err(ToolError::coded(error)),
    };
    let mut notes = Vec::new();
    let version = help::choose_version(
        text(arguments, "version"),
        named,
        solution.as_ref(),
        text(arguments, "profile").unwrap_or("default"),
        &mut notes,
    )
    // The only failures are a version that does not parse: the caller's own text when one was
    // given, otherwise the solution's `[help] version`.
    .map_err(|why| {
        let code = if text(arguments, "version").is_some() {
            ErrorCode::InvalidArguments
        } else {
            ErrorCode::InvalidData
        };
        ToolError::with(code, why)
    })?;
    Ok((version, notes))
}

/// Search the help center. Needs no solution; downloads go to the user's cache.
/// The knowledge topics, built in and the solution's own; needs no solution.
fn guide_tool(root: &Path, arguments: &Value) -> Result<Value, ToolError> {
    let solution = match Solution::discover(root) {
        Ok(solution) => Some(solution),
        Err(crate::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => return Err(ToolError::coded(error)),
    };
    let (topics, problems) = guide::topics(solution.as_ref());
    let mut result = match text(arguments, "action").unwrap_or("search") {
        "list" => json!({ "ok": true, "topics": topics.iter().map(|t| json!({
            "topic": t.id,
            "title": t.title,
            "origin": if t.file.is_some() { "solution" } else { "built in" },
            "sections": guide::sections(&t.text).len(),
        })).collect::<Vec<_>>() }),
        "search" => {
            let query = required(arguments, "text")?;
            let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(8) as usize;
            let hits = guide::search(&topics, query, limit);
            json!({
                "ok": true,
                "results": hits.iter().map(|h| json!({ "topic": h.topic, "section": h.heading, "matched": h.matched, "of": h.of, "line": h.line })).collect::<Vec<_>>(),
                "next": "read a section with action read, topic and section",
            })
        }
        "read" => {
            let topic =
                guide::find(&topics, required(arguments, "topic")?).map_err(ToolError::coded)?;
            match guide::read(topic, text(arguments, "section")).map_err(ToolError::coded)? {
                guide::Reading::Text(markdown) => {
                    json!({ "ok": true, "topic": topic.id, "markdown": markdown })
                }
                guide::Reading::Outline { title, headings } => json!({
                    "ok": true,
                    "topic": topic.id,
                    "title": title,
                    "sections": headings,
                    "note": "too long to read whole; read one section by its heading",
                }),
            }
        }
        other => {
            return Err(ToolError::invalid(format!(
                "action must be list, search or read, not {other:?}"
            )))
        }
    };
    if !problems.is_empty() {
        result["problems"] = json!(problems);
    }
    Ok(result)
}

fn help_search_tool(root: &Path, arguments: &Value) -> Result<Value, ToolError> {
    let query = required(arguments, "query")?;
    let (version, notes) = help_version(root, arguments, None)?;
    let cache = help::cache_root().map_err(ToolError::coded)?;
    let bytes = help::cached(
        &help::Web::default(),
        &cache,
        &version,
        help::INDEX_FILE,
        flag(arguments, "refresh", false),
    )
    .map_err(ToolError::coded)?;
    let index = help::Index::parse(&String::from_utf8_lossy(&bytes)).map_err(ToolError::coded)?;
    let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
    let found = help::search(&index, &version, query, limit);
    let mut result = json!({
        "ok": true,
        "version": version,
        "matched": found.matched,
        "results": found.hits.iter().map(|hit| json!({
            "title": hit.page.title,
            "path": hit.page.path,
            "url": hit.url,
            "summary": hit.page.summary,
        })).collect::<Vec<_>>(),
    });
    if !found.unknown.is_empty() {
        result["unknown_words"] = json!(found.unknown);
        result["note"] = json!("the help never uses these words, so no page holds every word; try fewer or other words");
    }
    if !notes.is_empty() {
        result["notes"] = json!(notes);
    }
    Ok(result)
}

/// Read one help page as Markdown, bounded by max_chars.
fn help_page_tool(root: &Path, arguments: &Value) -> Result<Value, ToolError> {
    let page = required(arguments, "page")?;
    let (named, path) = help::page_path(page).map_err(ToolError::coded)?;
    let (version, notes) = help_version(root, arguments, named)?;
    let cache = help::cache_root().map_err(ToolError::coded)?;
    let html = help::cached(
        &help::Web::default(),
        &cache,
        &version,
        &path,
        flag(arguments, "refresh", false),
    )
    .map_err(ToolError::coded)?;
    let read =
        help::read(&html, &version, &path, text(arguments, "section")).map_err(ToolError::coded)?;
    let max = arguments
        .get("max_chars")
        .and_then(Value::as_u64)
        .unwrap_or(20_000) as usize;
    let total = read.markdown.chars().count();
    let mut result = json!({
        "ok": true,
        "version": version,
        "title": read.title,
        "url": read.url,
        "headings": read.headings,
        "markdown": read.markdown.chars().take(max).collect::<String>(),
    });
    if total > max {
        result["truncated"] = json!(true);
        result["note"] = json!(format!(
            "{total} characters in all; ask for one section by heading, or raise max_chars"
        ));
    }
    if !notes.is_empty() {
        result["notes"] = json!(notes);
    }
    Ok(result)
}

/// Search or read the fixed-version Java API docs. It needs no solution and contacts only the
/// Javadoc site, never a ThingWorx server.
fn javadoc_tool(arguments: &Value) -> Result<Value, ToolError> {
    let action = required(arguments, "action")?;
    let name = required(arguments, "name")?;
    let refresh = flag(arguments, "refresh", false);
    let cache = javadoc::cache_root().map_err(|why| ToolError::with(ErrorCode::IoError, why))?;
    let web = help::Web::new(javadoc::BASE);
    let fetch = |path: &str| javadoc::cached(&web, &cache, path, refresh);
    let types = fetch(javadoc::TYPE_INDEX)?;
    let result = match action {
        "search" => {
            if arguments.get("member").is_some() {
                return Err(ToolError::invalid("`member` is only for action class"));
            }
            let members = fetch(javadoc::MEMBER_INDEX)?;
            let index = javadoc::Index::parse(
                &String::from_utf8_lossy(&types),
                &String::from_utf8_lossy(&members),
            )
            .map_err(|why| ToolError::with(ErrorCode::InvalidData, why))?;
            let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
            let hits = javadoc::search(&index, name, limit);
            json!({
                "ok": true,
                "summary": format!("{} matching Java API class(es) and member(s)", hits.len()),
                "version": javadoc::VERSION,
                "results": hits.iter().map(|hit| json!({
                    "kind": if hit.kind == javadoc::Kind::Class { "class" } else { "member" },
                    "display": hit.display(), "package": hit.package, "class": hit.class,
                    "label": hit.label, "path": hit.path, "url": hit.url,
                })).collect::<Vec<_>>(),
            })
        }
        "class" => {
            let index =
                javadoc::Index::parse(&String::from_utf8_lossy(&types), "memberSearchIndex = []")
                    .map_err(|why| ToolError::with(ErrorCode::InvalidData, why))?;
            // Unknown and ambiguous alike: the name the caller gave does not pick one class.
            let class = javadoc::find_class(&index, name)
                .map_err(|why| ToolError::with(ErrorCode::InvalidArguments, why))?;
            let path = javadoc::class_path(&class)
                .map_err(|why| ToolError::with(ErrorCode::InvalidData, why))?;
            let html = fetch(&path)?;
            let read = javadoc::read(&html, &class, text(arguments, "member"))?;
            json!({
                "ok": true,
                "summary": format!("{} method overload(s) documented for {}", read.methods, read.title),
                "version": javadoc::VERSION,
                "title": read.title,
                "path": read.path,
                "url": read.url,
                "markdown": read.markdown,
            })
        }
        other => {
            return Err(ToolError::invalid(format!(
                "action must be search or class, not {other:?}"
            )))
        }
    };
    Ok(result)
}

/// One of the server's logs, summary first. Read-only, so it takes no lock.
fn logs_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let log = required(arguments, "log")?;
    let now = logs::now_ms();
    let (from_ms, to_ms) = match (
        text(arguments, "since"),
        text(arguments, "from"),
        text(arguments, "to"),
    ) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err(ToolError::invalid(
                "since is a window ending now; give it, or from and to, not both",
            ))
        }
        (since, None, None) => (
            now - logs::parse_since(since.unwrap_or("1h")).map_err(ToolError::coded)?,
            now,
        ),
        (None, from, to) => {
            let to_ms = logs::parse_time(to.unwrap_or("now"), now).map_err(ToolError::coded)?;
            let from_ms = match from {
                Some(from) => logs::parse_time(from, now).map_err(ToolError::coded)?,
                None => to_ms - 3_600_000,
            };
            (from_ms, to_ms)
        }
    };
    let search = match (text(arguments, "grep"), text(arguments, "regex")) {
        (Some(_), Some(_)) => {
            return Err(ToolError::invalid(
                "grep and regex are two ways to search; give one",
            ))
        }
        (Some(grep), None) => Some(logs::Search::Grep(grep.to_string())),
        (None, Some(regex)) => Some(logs::Search::Regex(regex.to_string())),
        (None, None) => None,
    };
    let query = logs::Query {
        log: log.to_string(),
        from_ms,
        to_ms,
        level: text(arguments, "level")
            .map(logs::level)
            .transpose()
            .map_err(ToolError::coded)?,
        search,
        user: text(arguments, "user").map(str::to_string),
        thread: text(arguments, "thread").map(str::to_string),
        origin: text(arguments, "origin").map(str::to_string),
        limit: arguments
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(200),
        oldest_first: flag(arguments, "oldest_first", false),
    };
    let client = client(solution, arguments)?;
    let outcome = logs::query(&client, &query).map_err(ToolError::coded)?;
    Ok(logs::summary(
        log,
        &outcome,
        flag(arguments, "detail", false),
    ))
}

/// A log's levels, read, or changed as a plan unless dry_run is false. Writes no workspace file.
fn log_level_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let log = required(arguments, "log")?;
    let sublogger = text(arguments, "sublogger").map(str::to_string);
    let reset = flag(arguments, "reset", false);
    let change = match (text(arguments, "level"), reset) {
        (Some(_), true) => return Err(ToolError::invalid("give a level or reset, not both")),
        (Some(level), false) => Some(logs::Change::Set {
            level: logs::level(level).map_err(ToolError::coded)?,
            sublogger,
        }),
        (None, true) => Some(logs::Change::Reset { sublogger }),
        (None, false) if sublogger.is_some() => {
            return Err(ToolError::invalid(
                "a sublogger needs a level to set, or reset",
            ))
        }
        (None, false) => None,
    };
    let client = client(solution, arguments)?;
    let levels_json = |levels: &logs::Levels| {
        json!({
            "level": levels.level,
            "subloggers": levels.subloggers.iter().map(|(name, level)| json!({ "sublogger": name, "level": level })).collect::<Vec<_>>(),
        })
    };
    let Some(change) = change else {
        let levels = logs::levels(&client, log).map_err(ToolError::coded)?;
        return Ok(json!({ "ok": true, "log": log, "levels": levels_json(&levels) }));
    };
    let dry_run = flag(arguments, "dry_run", true);
    let report = logs::change(&client, log, &change, !dry_run).map_err(ToolError::coded)?;
    let mut result = json!({
        "ok": true,
        "log": log,
        "dry_run": dry_run,
        "change": report.plan,
        "before": levels_json(&report.before),
        "undo": report.undo,
    });
    match &report.after {
        Some(after) => result["after"] = levels_json(after),
        None => {
            result["note"] =
                json!("nothing was sent; pass dry_run: false. The level is the whole server's")
        }
    }
    Ok(result)
}

fn call_service_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let target = workspace::call_target(
        &workspace::discover(solution).entities,
        required(arguments, "target")?,
    )
    .map_err(ToolError::coded)?;
    let service = required(arguments, "service")?;
    let parameters = arguments
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !parameters.is_object() {
        return Err(ToolError::invalid("`parameters` must be a JSON object"));
    }
    if flag(arguments, "dry_run", true) {
        return Ok(json!({
            "dry_run": true,
            "would_call": { "target": target.to_string(), "service": service, "parameters": parameters },
            "note": "nothing was sent; pass dry_run: false to call it",
        }));
    }
    let timeout = arguments
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .filter(|s| *s > 0)
        .unwrap_or(120);
    let client = client(solution, arguments)?;
    let started = logs::now_ms();
    let outcome = client.call_service(&target, service, &parameters, Duration::from_secs(timeout));
    let logged = if flag(arguments, "with_logs", false) {
        let ended = logs::now_ms();
        Some(logs::during_call(
            &client,
            started,
            ended,
            logs::Wait::default(),
            &logs::now_ms,
            &std::thread::sleep,
        ))
    } else {
        None
    };
    let logs_json = |found: &Result<Vec<(String, logs::Entry)>, logs::LogsError>| match found {
        Ok(entries) => json!(entries
            .iter()
            .map(|(log, entry)| {
                let mut value = logs::entry_json(entry);
                value["log"] = json!(log);
                value
            })
            .collect::<Vec<_>>()),
        Err(error) => json!({ "error": format!("the logs could not be read: {error}") }),
    };
    let reply = match outcome {
        Ok(reply) => reply,
        // A failed call's log is the most useful thing about it, so it is returned, not lost.
        // A tool error, so the agent cannot take the failed call for a success; its logs, the
        // most useful thing about it, are in the message.
        Err(error) => {
            let message = match &logged {
                Some(Ok(entries)) if !entries.is_empty() => {
                    let lines: Vec<String> = entries
                        .iter()
                        .map(|(log, entry)| format!("{log}: {}", logs::line(entry)))
                        .collect();
                    format!("{error}\nlogged during the call:\n{}", lines.join("\n"))
                }
                Some(Err(why)) => {
                    format!("{error}\n(the call's logs could not be read: {why})")
                }
                _ => error.to_string(),
            };
            return Err(ToolError::with(error.code(), message));
        }
    };
    let detail = flag(arguments, "detail", false);
    let mut result = match reply {
        None => json!({ "dry_run": false, "result": "void" }),
        Some(value) if !detail && value.get("rows").is_some_and(Value::is_array) => {
            let rows = value["rows"].as_array().expect("checked");
            let fields: Vec<&String> = value
                .pointer("/dataShape/fieldDefinitions")
                .and_then(Value::as_object)
                .map(|f| f.keys().collect())
                .unwrap_or_default();
            json!({
                "dry_run": false,
                "rows": rows.len(),
                "fields": fields,
                "first_rows": rows.iter().take(3).collect::<Vec<_>>(),
            })
        }
        Some(value) => {
            // Not an InfoTable, so it has no rows to summarise; bound it by size instead.
            const SUMMARY_BYTES: usize = 16 * 1024;
            let text = value.to_string();
            if detail || text.len() <= SUMMARY_BYTES {
                json!({ "dry_run": false, "result": value })
            } else {
                let head: String = text.chars().take(4096).collect();
                json!({
                    "dry_run": false,
                    "result_bytes": text.len(),
                    "result_head": head,
                    "note": "the result is long; pass detail: true for all of it",
                })
            }
        }
    };
    if let Some(found) = &logged {
        result["logs"] = logs_json(found);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::datashape;

    /// Run a whole conversation through `serve` and return the responses, one per line.
    fn converse(root: &Path, messages: &[Value]) -> Vec<Value> {
        let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
        let mut output = Vec::new();
        serve(root, input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn solution_dir() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("twaco-mcp-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("Things/P.T.xml"),
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"></Thing></Things></Entities>",
        )
        .unwrap();
        root
    }

    fn default_matches_type(schema: &Value, value: &Value) -> bool {
        match schema["type"].as_str() {
            Some("string") => value.is_string(),
            Some("boolean") => value.is_boolean(),
            Some("integer") => value.as_u64().is_some(),
            Some("object") => value.is_object(),
            Some("array") => value.is_array(),
            _ => false,
        }
    }

    fn assert_schema_hygiene(tool: &str, path: &str, schema: &Value) {
        let object = schema
            .as_object()
            .unwrap_or_else(|| panic!("{tool} {path} is not a schema object"));
        for keyword in object.keys() {
            assert!(
                matches!(
                    keyword.as_str(),
                    "type"
                        | "properties"
                        | "required"
                        | "additionalProperties"
                        | "items"
                        | "enum"
                        | "minimum"
                        | "default"
                        | "description"
                ),
                "{tool} {path} uses unsupported keyword `{keyword}`"
            );
        }
        let kind = schema["type"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool} {path} has no type"));
        assert!(
            matches!(kind, "string" | "boolean" | "integer" | "object" | "array"),
            "{tool} {path} has unsupported type `{kind}`"
        );
        if let Some(default) = schema.get("default") {
            assert!(
                default_matches_type(schema, default),
                "{tool} {path} has a default with the wrong type"
            );
        }

        let empty = Map::new();
        let properties = schema["properties"].as_object().unwrap_or(&empty);
        for name in schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            assert!(
                properties.contains_key(name),
                "{tool} {path}.{name} is required but not a property"
            );
        }
        for (name, property) in properties {
            let property_path = if path.is_empty() {
                name.to_string()
            } else {
                format!("{path}.{name}")
            };
            assert_schema_hygiene(tool, &property_path, property);
        }
        if let Some(additional) = schema.get("additionalProperties") {
            assert!(
                additional == false || additional.is_object(),
                "{tool} {path} has unsupported additionalProperties"
            );
            if additional.is_object() {
                assert_schema_hygiene(tool, &format!("{path}.*"), additional);
            }
        }
        if let Some(items) = schema.get("items") {
            assert!(items.is_object(), "{tool} {path} has non-schema items");
            assert_schema_hygiene(tool, &format!("{path}[]"), items);
        }
    }

    #[test]
    fn tool_definitions_match_golden_file() {
        const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp_tools.json");
        let current = format!(
            "{}\n",
            serde_json::to_string_pretty(&tool_definitions()).unwrap()
        );
        if std::env::var("TWACO_BLESS").as_deref() == Ok("1") {
            std::fs::write(GOLDEN, current).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(GOLDEN).unwrap();
        assert_eq!(
            current, expected,
            "run with TWACO_BLESS=1 to rewrite tests/fixtures/mcp_tools.json, then review the diff"
        );
    }

    #[test]
    fn tool_schemas_use_the_supported_subset() {
        for tool in tool_definitions() {
            let name = tool["name"].as_str().unwrap();
            let schema = &tool["inputSchema"];
            assert_eq!(
                schema["additionalProperties"], false,
                "{name} inputSchema must refuse unknown arguments"
            );
            assert_schema_hygiene(name, "inputSchema", schema);
        }
    }

    #[test]
    fn validator_keeps_top_level_messages_stable() {
        let required = json!({ "type": "object", "properties": { "x": { "type": "string" } }, "required": ["x"], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&required, &json!({})),
            Err("`x` is required".to_string())
        );

        let names = json!({ "type": "object", "properties": { "a": { "type": "string" }, "b": { "type": "string" } }, "required": [], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&names, &json!({"x": 1})),
            Err("this tool takes no argument `x` (it takes: a, b)".to_string())
        );

        let boolean = json!({ "type": "object", "properties": { "x": { "type": "boolean" } }, "required": [], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&boolean, &json!({"x": 1})),
            Err("`x` must be a boolean, not 1".to_string())
        );

        let array = json!({ "type": "object", "properties": { "x": { "type": "array", "items": { "type": "string" } } }, "required": [], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&array, &json!({"x": "not an array"})),
            Err("`x` must be an array of strings, not \"not an array\"".to_string())
        );

        let integer = json!({ "type": "object", "properties": { "x": { "type": "integer", "minimum": 3 } }, "required": [], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&integer, &json!({"x": 1})),
            Err("`x` must be an integer of at least 3, not 1".to_string())
        );

        let choices = json!({ "type": "object", "properties": { "x": { "type": "string", "enum": ["a", "b"] } }, "required": [], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&choices, &json!({"x": "c"})),
            Err("`x` must be one of [\"a\",\"b\"], not \"c\"".to_string())
        );
        assert_eq!(
            validate_arguments(&choices, &json!([])),
            Err("`arguments` must be a JSON object".to_string())
        );
    }

    #[test]
    fn validator_follows_nested_schemas() {
        let map = json!({ "type": "object", "properties": { "map": { "type": "object", "additionalProperties": { "type": "string" } } }, "required": [], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&map, &json!({"map": {"title": 1}})),
            Err("`map.title` must be a string, not 1".to_string())
        );

        let closed = json!({ "type": "object", "properties": { "map": { "type": "object", "properties": { "title": { "type": "string" } }, "additionalProperties": false } }, "required": [], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&closed, &json!({"map": {"other": "x"}})),
            Err("`map.other` is not allowed".to_string())
        );

        let array = json!({ "type": "object", "properties": { "only": { "type": "array", "items": { "type": "string" } } }, "required": [], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&array, &json!({"only": ["P.T", 2]})),
            Err("`only[1]` must be a string, not 2".to_string())
        );

        let objects = json!({ "type": "object", "properties": { "rows": { "type": "array", "items": { "type": "object" } } }, "required": [], "additionalProperties": false });
        assert_eq!(
            validate_arguments(&objects, &json!({"rows": 1})),
            Err("`rows` must be an array, not 1".to_string())
        );
    }

    #[test]
    fn tool_errors_include_stable_codes() {
        let root = solution_dir();
        let reply = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects","arguments":{"unexpected":true}}}),
            ],
        );
        let value: Value =
            serde_json::from_str(reply[0]["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(
            value["error"],
            "this tool takes no argument `unexpected` (it takes: ); nothing was done"
        );
        assert_eq!(value["code"], "invalid_arguments");

        let unclassified = tool_result(Err(ToolError::from("free text")), LATEST);
        let value: Value =
            serde_json::from_str(unclassified["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({ "error": "free text", "code": "unclassified" })
        );
    }

    #[test]
    fn typed_and_argument_failures_keep_their_codes() {
        let root = solution_dir();
        let cases = [
            (
                "rename",
                json!({"kind":"service","old":"A","new":"B"}),
                "invalid_arguments",
            ),
            (
                "status",
                json!({"entity":"No.Such.Thing"}),
                "unknown_entity",
            ),
            ("sync", json!({"entity":"No.Such.Thing"}), "unknown_entity"),
            (
                "catalog",
                json!({"entity":"No.Such.Thing"}),
                "unknown_entity",
            ),
            (
                "impact",
                json!({"entity":"No.Such.Thing"}),
                "unknown_entity",
            ),
            (
                "call",
                json!({"target":"Things/..","service":"X"}),
                "invalid_arguments",
            ),
            ("db_run", json!({}), "invalid_arguments"),
            ("entity_delete", json!({}), "invalid_arguments"),
            (
                "rename",
                json!({"kind":"entity","old":"No.Such.Thing","new":"No.Such.Other"}),
                "unknown_entity",
            ),
            (
                "push",
                json!({"entity":"Things/No.Such.Thing"}),
                "unknown_entity",
            ),
            (
                "new_building_block",
                json!({"name":"Bad Name!"}),
                "invalid_arguments",
            ),
            (
                "rename",
                json!({"kind":"entity","old":"P.T","new":"P.T"}),
                "invalid_arguments",
            ),
        ];
        for (id, (tool, arguments, code)) in cases.into_iter().enumerate() {
            let reply = converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":arguments}}),
                ],
            );
            assert_eq!(reply[0]["result"]["isError"], true, "{tool}");
            let body: Value =
                serde_json::from_str(reply[0]["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(body["code"], code, "{tool}: {body}");
            assert!(body["error"]
                .as_str()
                .is_some_and(|message| !message.is_empty()));
        }
    }

    #[test]
    fn every_empty_tool_failure_has_a_classified_code() {
        let root = solution_dir();
        let definitions = tool_definitions();
        let messages: Vec<Value> = definitions
            .iter()
            .enumerate()
            .map(|(id, definition)| {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": { "name": definition["name"], "arguments": {} },
                })
            })
            .collect();
        let replies = converse(&root, &messages);
        let mut unclassified = Vec::new();
        for (definition, reply) in definitions.iter().zip(replies) {
            if reply["result"]["isError"] == true {
                let body: Value =
                    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap())
                        .unwrap();
                if body["code"]
                    .as_str()
                    .is_none_or(|code| code == "unclassified")
                {
                    unclassified.push(format!("{}: {body}", definition["name"]));
                }
            }
        }
        assert!(
            unclassified.is_empty(),
            "empty calls with missing or unclassified codes: {}",
            unclassified.join("; ")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refused_push_result_has_a_server_conflict_code() {
        let refusal = push::Refusal::Conflict {
            server: "server".to_string(),
            baseline: "baseline".to_string(),
        };
        let result = push_outcome_json(
            "Things/P.T",
            false,
            false,
            commands::push::PushOutcome::Applied {
                entity: crate::core::entity_key::EntityKey::new("Things", "P.T").unwrap(),
                result: push::Outcome::Refused(refusal.clone()),
                backup: None,
                effects: commands::Effects::new(commands::Access::Read, commands::Access::Read),
            },
        );
        assert_eq!(result["code"], "server_conflict");
        assert_eq!(result["refusal"], refusal.to_string());
    }

    #[test]
    fn nested_tool_arguments_are_validated_without_closing_parameters() {
        let root = solution_dir();
        let map = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"datatable_copy","arguments":{"old":"P.Old","new":"P.New","map":{"title":1}}}}),
            ],
        );
        assert_eq!(map[0]["result"]["isError"], true);
        assert!(map[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("`map.title` must be a string, not 1"));

        let parameters = json!({ "row": { "title": "x", "values": [1, { "nested": true }] } });
        let call = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"call","arguments":{"target":"T","service":"Reset","parameters":parameters}}}),
            ],
        );
        assert_eq!(call[0]["result"]["isError"], false);
        assert_eq!(
            call[0]["result"]["structuredContent"]["would_call"]["parameters"],
            parameters
        );

        let unknown = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"projects","arguments":{"x":1}}}),
            ],
        );
        assert_eq!(unknown[0]["result"]["isError"], true);
        assert!(unknown[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("this tool takes no argument `x` (it takes: )"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_handshake_negotiates_a_version_and_notifications_get_no_reply() {
        let root = solution_dir();
        let responses = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
                json!({"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
            ],
        );
        assert_eq!(responses.len(), 3, "the notification is not answered");
        assert_eq!(responses[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "twaco");
        assert_eq!(responses[1]["result"], json!({}));
        assert_eq!(
            responses[2]["result"]["protocolVersion"], LATEST,
            "an unknown version gets ours"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn errors_are_protocol_errors_or_tool_errors_as_the_spec_says() {
        let root = solution_dir();
        let responses = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"no/such"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"nope","arguments":{}}}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"call","arguments":{"service":"S"}}}),
            ],
        );
        assert_eq!(responses[0]["error"]["code"], -32601);
        assert_eq!(responses[1]["error"]["code"], -32602);
        // A tool that ran and failed is a result with isError, so the model can see why.
        assert_eq!(responses[2]["result"]["isError"], true);
        assert!(responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("`target` is required"));
        let mut output = Vec::new();
        serve(&root, "not json\n".as_bytes(), &mut output).unwrap();
        let parsed: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(parsed["error"]["code"], -32700);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn logs_arguments_are_refused_before_the_server_is_asked() {
        let root = solution_dir();
        for (arguments, why) in [
            (json!({}), "`log` is required"),
            (
                json!({"log": "ScriptLog", "level": "LOUD"}),
                "must be one of",
            ),
            (json!({"log": "ScriptLog", "limit": 0}), "at least 1"),
            (
                json!({"log": "ScriptLog", "grep": "a", "regex": "b"}),
                "give one",
            ),
            (
                json!({"log": "ScriptLog", "since": "1h", "from": "now"}),
                "not both",
            ),
            (
                json!({"log": "ScriptLog", "since": "soon"}),
                "since must be",
            ),
        ] {
            let responses = converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"logs","arguments":arguments}}),
                ],
            );
            let result = &responses[0]["result"];
            let text = result["content"][0]["text"].as_str().unwrap_or_default();
            assert_eq!(result["isError"], true, "{arguments}: {text}");
            assert!(text.contains(why), "{arguments}: {text}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_tool_is_listed_with_a_schema_and_honest_annotations() {
        let root = solution_dir();
        let responses = converse(
            &root,
            &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
        );
        let tools = responses[0]["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "projects",
                "types",
                "check",
                "status",
                "sync",
                "extract",
                "fmt",
                "push",
                "entity_delete",
                "entity_restore",
                "entity_carry",
                "db_run",
                "db_query",
                "datatable_copy",
                "db_clean",
                "deploy",
                "adopt_report",
                "adopt_apply",
                "rename",
                "move_member",
                "new_building_block",
                "retemplate",
                "config_table",
                "logs",
                "log_level",
                "repo",
                "repo_write",
                "extensions",
                "extension_write",
                "export",
                "package",
                "import",
                "settings",
                "catalog",
                "impact",
                "unused",
                "docs",
                "guide",
                "help_search",
                "help_page",
                "javadoc",
                "call",
            ]
        );
        for name in [
            "push",
            "entity_delete",
            "entity_restore",
            "entity_carry",
            "db_run",
            "db_clean",
            "datatable_copy",
            "deploy",
            "rename",
            "move_member",
            "new_building_block",
            "retemplate",
            "config_table",
            "log_level",
            "repo_write",
            "extension_write",
            "export",
            "import",
            "call",
        ] {
            let tool = tools.iter().find(|t| t["name"] == name).unwrap();
            assert_eq!(
                tool["inputSchema"]["properties"]["dry_run"]["default"], true,
                "{name} writes to a server"
            );
        }
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
        let call = tools.iter().find(|t| t["name"] == "call").unwrap();
        assert_eq!(
            call["annotations"]["readOnlyHint"], false,
            "a service call may write"
        );
        assert_eq!(
            call["inputSchema"]["properties"]["dry_run"]["default"],
            true
        );
        let entity_delete = tools.iter().find(|t| t["name"] == "entity_delete").unwrap();
        for parameter in [
            "allow_repository_defined",
            "allow_outside_dependents",
            "allow_file_repository_data_loss",
        ] {
            assert_eq!(
                entity_delete["inputSchema"]["properties"][parameter]["default"], false,
                "{parameter}"
            );
        }
        assert!(
            entity_delete["inputSchema"]["properties"]["force"]["description"]
                .as_str()
                .unwrap()
                .contains("Deprecated")
        );
        let rename = tools.iter().find(|t| t["name"] == "rename").unwrap();
        assert_eq!(
            rename["inputSchema"]["required"],
            json!(["kind", "old", "new"])
        );
        assert_eq!(
            rename["inputSchema"]["properties"]["kind"]["enum"],
            json!(["entity", "prefix", "field", "service", "param", "table", "property"])
        );
        assert_eq!(
            rename["inputSchema"]["properties"]["scope"]["type"],
            "string"
        );
        assert_eq!(
            rename["inputSchema"]["properties"]["service"]["type"],
            "string"
        );
        assert_eq!(
            rename["inputSchema"]["properties"]["include_outside"]["default"],
            false
        );
        assert_eq!(
            rename["inputSchema"]["properties"]["skip_checks"]["default"],
            false
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rename_plans_then_applies_and_refuses_bad_requests() {
        let root = solution_dir();
        let original = std::fs::read(root.join("Things/P.T.xml")).unwrap();
        let call = |arguments: Value| {
            converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rename","arguments":arguments}}),
                ],
            )
        };

        let dry = call(json!({"kind":"entity","old":"P.T","new":"P.U"}));
        let result = &dry[0]["result"]["structuredContent"];
        assert_eq!(result["applied"], false);
        assert_eq!(result["moves"]["count"], 1);
        assert_eq!(
            std::fs::read(root.join("Things/P.T.xml")).unwrap(),
            original
        );
        assert!(!root.join("Things/P.U.xml").exists());

        let applied = call(json!({"kind":"entity","old":"P.T","new":"P.U","dry_run":false}));
        let result = &applied[0]["result"]["structuredContent"];
        assert_eq!(result["applied"], true);
        assert_eq!(result["verification"]["sync_problems"], json!([]));
        assert_eq!(result["verification"]["blocking_gates"], json!([]));
        assert!(!root.join("Things/P.T.xml").exists());
        assert!(root.join("Things/P.U.xml").exists());

        for arguments in [
            json!({"kind":"entity","old":"Missing","new":"P.V"}),
            json!({"kind":"other","old":"P.U","new":"P.V"}),
        ] {
            let refused = call(arguments);
            assert_eq!(refused[0]["result"]["isError"], true);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn field_rename_requires_scope_and_is_a_dry_run_by_default() {
        let root = solution_dir();
        std::fs::create_dir_all(root.join("DataShapes")).unwrap();
        std::fs::create_dir_all(root.join("src/P.D")).unwrap();
        let shape = "<Entities><DataShapes><DataShape name=\"P.D\" projectName=\"P\"><FieldDefinitions><FieldDefinition name=\"Period\" baseType=\"STRING\" ordinal=\"1\" description=\"\"/></FieldDefinitions></DataShape></DataShapes></Entities>";
        std::fs::write(root.join("DataShapes/P.D.xml"), shape).unwrap();
        let fields = datashape::extract(shape.as_bytes()).unwrap();
        std::fs::write(
            root.join("src/P.D/fields.json"),
            datashape::to_sidecar(&fields),
        )
        .unwrap();
        std::fs::write(root.join("Things/P.T.xml"), "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><ConfigurationTables><ConfigurationTable dataShapeName=\"P.D\" name=\"T\"><DataShape><FieldDefinitions><FieldDefinition name=\"Period\"/></FieldDefinitions></DataShape><Rows><Row><Period>x</Period></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>").unwrap();
        let original = std::fs::read(root.join("Things/P.T.xml")).unwrap();
        let call = |arguments: Value| {
            converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rename","arguments":arguments}}),
                ],
            )
        };
        let missing = call(json!({"kind":"field","old":"Period","new":"PeriodKey"}));
        assert_eq!(missing[0]["result"]["isError"], true);
        assert!(missing[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("scope is required"));
        // The command line refuses --text for a member rename; the tool refuses its twin.
        let text_pass = call(
            json!({"kind":"field","scope":"P.D","old":"Period","new":"PeriodKey","include_outside":true}),
        );
        assert_eq!(text_pass[0]["result"]["isError"], true);
        assert!(text_pass[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no text pass"));
        let dry = call(json!({"kind":"field","scope":"P.D","old":"Period","new":"PeriodKey"}));
        let result = &dry[0]["result"]["structuredContent"];
        assert_eq!(result["applied"], false);
        assert_eq!(result["spec"]["scope"], "P.D");
        assert_eq!(
            std::fs::read(root.join("Things/P.T.xml")).unwrap(),
            original
        );
        let refused = call(json!({"kind":"entity","scope":"P.D","old":"P.T","new":"P.U"}));
        assert_eq!(refused[0]["result"]["isError"], true);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn service_rename_requires_scope_and_is_a_dry_run_by_default() {
        let root = solution_dir();
        std::fs::create_dir_all(root.join("ThingShapes")).unwrap();
        std::fs::create_dir_all(root.join("src/P.Shape/services/Run")).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\ncollections = [\"ThingShapes\"]\n",
        )
        .unwrap();
        let shape = "<Entities><ThingShapes><ThingShape name=\"P.Shape\" projectName=\"P\"><ServiceDefinitions><ServiceDefinition name=\"Run\"/></ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[me.Run();]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></ThingShapes></Entities>";
        std::fs::write(root.join("ThingShapes/P.Shape.xml"), shape).unwrap();
        std::fs::write(
            root.join("src/P.Shape/services/Run/definition.xml"),
            "<ServiceDefinition name=\"Run\"/>\n",
        )
        .unwrap();
        std::fs::write(root.join("src/P.Shape/services/Run/script.js"), "me.Run();").unwrap();
        let original = std::fs::read(root.join("ThingShapes/P.Shape.xml")).unwrap();
        let call = |arguments: Value| {
            converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rename","arguments":arguments}}),
                ],
            )
        };
        let missing =
            call(json!({"kind":"service","old":"Run","new":"Execute","skip_checks":true}));
        assert_eq!(missing[0]["result"]["isError"], true);
        assert!(missing[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("scope is required"));
        let dry = call(
            json!({"kind":"service","scope":"P.Shape","old":"Run","new":"Execute","skip_checks":true}),
        );
        assert_eq!(
            dry[0]["result"]["structuredContent"]["spec"]["kind"],
            "service"
        );
        assert_eq!(dry[0]["result"]["structuredContent"]["applied"], false);
        assert_eq!(
            std::fs::read(root.join("ThingShapes/P.Shape.xml")).unwrap(),
            original
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn table_rename_requires_scope_and_is_a_dry_run_by_default() {
        let root = solution_dir();
        let entity = "<Entities><Things><Thing name=\"P.T\" projectName=\"P\" thingTemplate=\"GenericThing\"><ConfigurationTableDefinitions><ConfigurationTableDefinition dataShapeName=\"P.Limits_CT\" name=\"Limits_CT\"/></ConfigurationTableDefinitions><ConfigurationTables><ConfigurationTable dataShapeName=\"P.Limits_CT\" name=\"Limits_CT\"><DataShape/><Rows/></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>";
        std::fs::write(root.join("Things/P.T.xml"), entity).unwrap();
        let original = std::fs::read(root.join("Things/P.T.xml")).unwrap();
        let call = |arguments: Value| {
            converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rename","arguments":arguments}}),
                ],
            )
        };
        let missing =
            call(json!({"kind":"table","old":"Limits_CT","new":"Bounds_CT","skip_checks":true}));
        assert_eq!(missing[0]["result"]["isError"], true);
        assert!(missing[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("scope is required"));
        let dry = call(
            json!({"kind":"table","scope":"P.T","old":"Limits_CT","new":"Bounds_CT","skip_checks":true}),
        );
        assert_eq!(
            dry[0]["result"]["structuredContent"]["spec"]["kind"],
            "table"
        );
        assert_eq!(
            dry[0]["result"]["structuredContent"]["spec"]["scope"],
            "P.T"
        );
        assert_eq!(dry[0]["result"]["structuredContent"]["applied"], false);
        assert_eq!(
            std::fs::read(root.join("Things/P.T.xml")).unwrap(),
            original
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn projects_and_check_answer_offline_with_structured_content() {
        let root = solution_dir();
        let responses = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects","arguments":{}}}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"check","arguments":{}}}),
            ],
        );
        let projects = &responses[1]["result"]["structuredContent"];
        assert_eq!(projects["projects"][0]["name"], "P");
        assert_eq!(projects["projects"][0]["entities"], 1);
        let check = &responses[2]["result"]["structuredContent"];
        assert!(check["gates"].as_array().unwrap().len() >= 6);
        assert!(
            check.get("failures").is_none(),
            "findings are detail, not summary"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_answers_offline_with_counts_and_services() {
        let root = solution_dir();
        std::fs::write(
            root.join("Things/P.T.xml"),
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><ThingShape><ServiceDefinitions><ServiceDefinition name=\"Run\" description=\"Does work\"><ResultType baseType=\"STRING\"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>",
        )
        .unwrap();
        let responses = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"catalog","arguments":{}}}),
            ],
        );
        let catalog = &responses[0]["result"]["structuredContent"];
        assert_eq!(catalog["service_count"], 1);
        assert_eq!(catalog["entity_count"], 1);
        assert_eq!(catalog["services"][0]["entity"], "P.T");
        assert_eq!(catalog["services"][0]["name"], "Run");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn impact_answers_offline_and_read_only_with_the_strength_of_each_reference() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders");
        let call = |arguments: Value| {
            let responses = converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"impact","arguments":arguments}}),
                ],
            );
            responses[0]["result"].clone()
        };
        let summary = call(json!({"entity":"Audit","member":"Record"}));
        assert_eq!(summary["isError"], false, "{summary}");
        let report = &summary["structuredContent"];
        assert_eq!(report["entity"], "Things/Acme.Orders.Audit");
        assert_eq!(report["counts"]["resolved"], 2);
        assert_eq!(report["complete"], true);
        assert!(
            report["dependents"][0].get("path").is_none(),
            "a summary has no chains"
        );
        let detailed = call(json!({"entity":"Audit","member":"Record","detail":true}));
        assert_eq!(
            detailed["structuredContent"]["dependents"][1]["path"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        let strong = call(json!({"entity":"OrderLine_DS","min_confidence":"structural"}));
        assert!(strong["structuredContent"]["dependents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["confidence"] == "structural"));
        let dot = call(json!({"entity":"Audit","member":"Record","format":"dot"}));
        assert!(
            dot["structuredContent"]["dot"]
                .as_str()
                .unwrap()
                .starts_with("digraph impact {"),
            "{dot}"
        );
        // A bad confidence is refused by the schema before the tool runs, and nothing was written.
        let bad = call(json!({"entity":"Audit","min_confidence":"sure"}));
        assert_eq!(bad["isError"], true);
        assert!(
            !root.join(".twaco").exists(),
            "a read-only tool leaves no trace"
        );
    }

    #[test]
    fn docs_returns_the_document_and_its_markdown_and_writes_nothing() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders");
        let call = |arguments: Value| {
            let responses = converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"docs","arguments":arguments}}),
                ],
            );
            responses[0]["result"].clone()
        };
        let manager_of = |result: &Value| {
            result["structuredContent"]["document"]["entities"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entity| entity["entity"] == "Things/Acme.Orders.Manager")
                .unwrap()
                .clone()
        };
        let summary = call(json!({}));
        assert_eq!(summary["isError"], false, "{summary}");
        let content = &summary["structuredContent"];
        assert_eq!(content["document"]["complete"], true);
        assert_eq!(content["document"]["solution"], "Acme.Orders");
        let manager = manager_of(&summary);
        assert_eq!(manager["service_count"], 5, "a summary counts the services");
        assert!(manager.get("services").is_none());
        assert!(content["markdown"]
            .as_str()
            .unwrap()
            .starts_with("# Acme.Orders: solution documentation"));
        let detail = call(json!({"detail": true}));
        assert_eq!(
            manager_of(&detail)["services"].as_array().map(Vec::len),
            Some(5)
        );
        assert!(
            !root.join(".twaco").exists(),
            "a read-only tool leaves no trace"
        );
    }

    #[test]
    fn unused_reports_what_no_entry_point_reaches_and_deletes_nothing() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders");
        let call = |arguments: Value| {
            let responses = converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"unused","arguments":arguments}}),
                ],
            );
            responses[0]["result"].clone()
        };
        let report = call(json!({}));
        assert_eq!(report["isError"], false, "{report}");
        let report = &report["structuredContent"];
        let unused: Vec<&str> = report["unused"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["entity"].as_str().unwrap())
            .collect();
        assert_eq!(
            unused,
            [
                "DataShapes/Acme.Orders.Retired_DS",
                "ThingShapes/Acme.Orders.Legacy_TS"
            ]
        );
        assert!(
            report["roots"][0].get("entities").is_none(),
            "a summary counts the entry points"
        );
        assert_eq!(report["complete"], true);
        let narrowed = call(json!({"collection":"Things"}));
        assert_eq!(
            narrowed["structuredContent"]["unused"]
                .as_array()
                .map(Vec::len),
            Some(0)
        );
        // The schema refuses a collection that is not judged, and nothing is written.
        assert_eq!(call(json!({"collection":"Mashups"}))["isError"], true);
        assert!(
            !root.join(".twaco").exists(),
            "a read-only tool leaves no trace"
        );
    }

    #[test]
    fn types_generate_and_check_return_the_mcp_shapes() {
        let root = solution_dir();
        let generated = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"types","arguments":{}}}),
            ],
        );
        let result = &generated[0]["result"]["structuredContent"];
        assert_eq!(result["ok"], true);
        assert_eq!(result["entities"], 1);
        assert_eq!(result["data_shapes"], 0);
        assert!(result["files_written"].as_u64().unwrap() >= 4);

        let entity = concat!(
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><ThingShape>",
            "<ServiceDefinitions><ServiceDefinition name=\"Run\"><ParameterDefinitions></ParameterDefinitions>",
            "<ResultType baseType=\"NOTHING\"/></ServiceDefinition></ServiceDefinitions>",
            "</ThingShape></Thing></Things></Entities>"
        );
        std::fs::write(root.join("Things/P.T.xml"), entity).unwrap();
        std::fs::create_dir_all(root.join("src/P.T/services/Run")).unwrap();
        std::fs::write(root.join("src/P.T/services/Run/script.js"), "run();").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        struct FakeCompiler;
        impl types::CompilerRunner for FakeCompiler {
            fn run(
                &self,
                _program: &std::ffi::OsStr,
                _arguments: &[std::ffi::OsString],
                _current_dir: &Path,
            ) -> std::io::Result<types::CompilerOutput> {
                Ok(types::CompilerOutput {
                    success: false,
                    stdout:
                        b".twaco/types/check/s0000.js(3,4): error TS2554: Expected 2 arguments.\n"
                            .to_vec(),
                    stderr: Vec::new(),
                })
            }
        }
        let checked =
            types_tool_with_compiler(&solution, &json!({"action":"check"}), Some(&FakeCompiler))
                .unwrap();
        assert_eq!(checked["ok"], false);
        assert_eq!(checked["findings"], 1);
        assert_eq!(checked["services_with_findings"], 1);
        assert_eq!(checked["services"], 1);
        assert_eq!(checked["by_code"]["TS2554"], 1);
        assert_eq!(checked["first"].as_array().unwrap().len(), 1);
        assert!(checked.get("findings_list").is_none());
        let detailed = types_tool_with_compiler(
            &solution,
            &json!({"action":"check", "detail":true}),
            Some(&FakeCompiler),
        )
        .unwrap();
        assert_eq!(detailed["findings_list"].as_array().unwrap().len(), 1);
        assert!(detailed.get("first").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sync_writes_a_sidecar_edit_and_refuses_while_the_workspace_is_locked() {
        let root = solution_dir();
        let entity = "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><ThingShape>\
<ServiceDefinitions><ServiceDefinition name=\"S\"></ServiceDefinition></ServiceDefinitions>\
<ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\">\
<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>\
<code><![CDATA[\nold();\n]]></code></Row></Rows></ConfigurationTable></ConfigurationTables>\
</ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>";
        std::fs::write(root.join("Things/P.T.xml"), entity).unwrap();
        let dir = root.join("src/P.T/services/S");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("definition.xml"),
            "<ServiceDefinition name=\"S\"></ServiceDefinition>\n",
        )
        .unwrap();
        std::fs::write(dir.join("script.js"), "new();").unwrap();
        let sync = |arguments: Value| {
            let responses = converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"sync","arguments":arguments}}),
                ],
            );
            responses[0]["result"].clone()
        };

        let checked = sync(json!({"check": true, "all": true}));
        let text: Value =
            serde_json::from_str(checked["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text["changed"], 1);
        assert!(
            std::fs::read_to_string(root.join("Things/P.T.xml"))
                .unwrap()
                .contains("old();"),
            "check writes nothing"
        );

        let held = lock::acquire(&root, "deploy", &[]).unwrap();
        let refused = sync(json!({"all": true}));
        assert_eq!(refused["isError"], true);
        assert!(refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("another twaco command"));
        drop(held);

        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        types::write(&solution).unwrap();
        let written = sync(json!({"all": true}));
        assert_eq!(written["isError"], false);
        let content: Value =
            serde_json::from_str(written["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(content.get("types_refreshed").is_some());
        assert!(std::fs::read_to_string(root.join("Things/P.T.xml"))
            .unwrap()
            .contains("new();"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_bad_message_gets_its_error_and_the_server_reads_on() {
        let root = solution_dir();
        let mut input = Vec::new();
        for line in [
            &b"42"[..],
            b"[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]",
            b"{\"id\":2}",
            b"{\"id\":3,\"method\":\"ping\"}",
            b"{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"ping\"}",
            b"{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"ping\",\"params\":[1]}",
            b"{\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{}}",
            b"\xff\xfe not UTF-8",
            b"{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"ping\"}",
        ] {
            input.extend_from_slice(line);
            input.push(b'\n');
        }
        let mut output = Vec::new();
        serve(&root, &input[..], &mut output).unwrap();
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let summary: Vec<(Value, Value)> = responses
            .iter()
            .map(|r| (r["id"].clone(), r["error"]["code"].clone()))
            .collect();
        assert_eq!(
            summary,
            [
                (Value::Null, json!(-32600)),
                (Value::Null, json!(-32600)),
                (json!(2), json!(-32600)),
                (json!(3), json!(-32600)),
                (Value::Null, json!(-32600)),
                (json!(4), json!(-32602)),
                (Value::Null, json!(-32700)),
                (json!(6), Value::Null),
            ],
            "a client's response gets no answer, and the ping after the bad line does"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arguments_of_the_wrong_shape_are_refused_before_the_tool_runs() {
        let root = solution_dir();
        std::fs::create_dir_all(root.join("DataShapes")).unwrap();
        std::fs::write(
            root.join("DataShapes/P.D.xml"),
            "<Entities><DataShapes><DataShape name=\"P.D\" projectName=\"P\"></DataShape></DataShapes></Entities>",
        )
        .unwrap();
        let before = std::fs::read(root.join("Things/P.T.xml")).unwrap();
        for (tool, arguments, why) in [
            (
                "sync",
                json!({"check": "true", "all": true}),
                "`check` must be a boolean",
            ),
            ("sync", json!({"entity": 17}), "`entity` must be a string"),
            ("sync", json!({}), "name an entity, or pass all: true"),
            ("extract", json!({"entity": "P.T", "all": true}), "not both"),
            (
                "status",
                json!({"record": true}),
                "name an entity, or pass all: true",
            ),
            (
                "deploy",
                json!({"dry_run": false, "only": "P.T"}),
                "`only` must be an array of strings",
            ),
            (
                "deploy",
                json!({"dry_run": null}),
                "`dry_run` must be a boolean",
            ),
            (
                "push",
                json!({"entity": "P.T", "dryrun": false}),
                "takes no argument `dryrun`",
            ),
            ("push", json!({}), "`entity` is required"),
            (
                "call",
                json!({"target": "T", "service": "S", "timeout_seconds": 0}),
                "at least 1",
            ),
            (
                "config_table",
                json!({"thing": "P.T", "table": "C", "action": "drop"}),
                "must be one of",
            ),
            ("types", json!({"action": "other"}), "must be one of"),
            (
                "config_table",
                json!({"thing": "P.D", "table": "C"}),
                "only a Thing has configuration tables",
            ),
        ] {
            let responses = converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":arguments}}),
                ],
            );
            let result = &responses[0]["result"];
            let text = result["content"][0]["text"].as_str().unwrap_or_default();
            assert_eq!(result["isError"], true, "{tool} {arguments}: {text}");
            assert!(text.contains(why), "{tool} {arguments}: {text}");
        }
        assert_eq!(std::fs::read(root.join("Things/P.T.xml")).unwrap(), before);
        assert!(!root.join(".twaco/baseline.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_status_with_an_unreadable_entity_file_records_nothing() {
        let root = solution_dir();
        std::fs::write(root.join("Things/Broken.xml"), "<Entities><Things><Thing").unwrap();
        let responses = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"status","arguments":{"all":true,"record":true}}}),
            ],
        );
        let result = &responses[0]["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        assert_eq!(result["isError"], true, "{text}");
        assert!(text.contains("could not be read"), "{text}");
        assert!(!root.join(".twaco/baseline.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_status_whose_reads_failed_records_nothing() {
        let root = solution_dir();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        // Port 9 (discard) is closed on a development machine: every read fails at once.
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://127.0.0.1:9/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        let responses = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"status","arguments":{"all":true,"record":true}}}),
            ],
        );
        let result = &responses[0]["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        assert_eq!(result["isError"], true, "{text}");
        assert!(text.contains("nothing was recorded"), "{text}");
        assert!(!root.join(".twaco/baseline.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn writing_tools_take_the_lock_before_they_read_anything() {
        // Each call would fail on what it reads (an unknown entity, no profile), so being
        // refused for the lock instead proves the lock came first.
        let root = solution_dir();
        let held = lock::acquire(&root, "sync", &[]).unwrap();
        for (tool, arguments) in [
            (
                "push",
                json!({"entity": "No.Such.Entity", "dry_run": false}),
            ),
            ("deploy", json!({"dry_run": false, "profile": "missing"})),
            (
                "status",
                json!({"entity": "No.Such.Entity", "record": true}),
            ),
            ("sync", json!({"entity": "No.Such.Entity"})),
            ("extract", json!({"entity": "No.Such.Entity"})),
            ("types", json!({"action": "platform", "profile": "missing"})),
        ] {
            let responses = converse(
                &root,
                &[
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":arguments}}),
                ],
            );
            let text = responses[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(text.contains("another twaco command"), "{tool}: {text}");
        }
        drop(held);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_held_workspace_lock_has_its_stable_code() {
        let root = solution_dir();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire_for(&solution, "test holder").unwrap();
        let replies = converse(
            &root,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "sync", "arguments": { "entity": "P.T" } },
            })],
        );
        let result = &replies[0]["result"];
        assert_eq!(result["isError"], true);
        let body: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["code"], "workspace_locked");
        assert!(
            body["error"].as_str().is_some_and(
                |message| message.contains("another twaco command is changing this workspace")
            ),
            "{body}"
        );
        drop(held);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_service_call_is_a_dry_run_unless_asked_and_needs_no_server_to_be_one() {
        let root = solution_dir();
        let responses = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"call","arguments":{"target":"T","service":"Reset","parameters":{"x":1}}}}),
            ],
        );
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false);
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text["dry_run"], true);
        assert_eq!(text["would_call"]["service"], "Reset");
        let _ = std::fs::remove_dir_all(root);
    }
}
