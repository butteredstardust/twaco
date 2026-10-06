//! The successful result of each tool that publishes an `outputSchema`.
//!
//! These describe the JSON the adapters return; they do not build it. A field every result has is
//! required. Anything a result has only sometimes (a plan versus an applied run, a refusal, a
//! detail the caller asked for) is optional, and the objects stay open so that the notices and
//! `duration_ms` every result may carry are not refused. Tests check each result a registered
//! tool returns against its schema, so a drift between this file and an adapter fails there.

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// What every state-changing tool may add: what taking the workspace lock did.
type Notices = Option<Vec<String>>;

#[derive(Serialize, JsonSchema)]
pub(crate) struct ProjectsResult {
    solution: String,
    projects: Vec<ProjectEntry>,
    unreadable: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
struct ProjectEntry {
    name: String,
    root: String,
    depends_on: Vec<String>,
    entities: u64,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct CheckResult {
    ok: bool,
    gates: Vec<GateEntry>,
    findings: u64,
    broken: u64,
    /// With detail: every finding.
    failures: Option<Vec<Value>>,
}

#[derive(Serialize, JsonSchema)]
struct GateEntry {
    gate: String,
    examined: u64,
    findings: u64,
    broken: Option<String>,
    advisory: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct StatusResult {
    ok: bool,
    /// How many baselines were recorded; absent unless asked to record.
    recorded: Option<u64>,
    entities: u64,
    counts: BTreeMap<String, u64>,
    failures: Vec<String>,
    unreadable: Vec<String>,
    /// Without detail: only what needs attention.
    attention: Option<Vec<StatusEntry>>,
    /// With detail: every entity.
    statuses: Option<Vec<StatusEntry>>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
struct StatusEntry {
    entity: String,
    verdict: String,
    working: Option<Value>,
    server: Option<Value>,
    baseline_local: Option<Value>,
    baseline_server: Option<Value>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct SyncResult {
    ok: bool,
    check: bool,
    checked: u64,
    changed: u64,
    failed: u64,
    changes: Vec<String>,
    errors: Vec<String>,
    types_refreshed: Option<u64>,
    types_warning: Option<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ExtractResult {
    ok: bool,
    parts: u64,
    entities: u64,
    failed: u64,
    written: Vec<String>,
    errors: Vec<String>,
    types_refreshed: Option<u64>,
    types_warning: Option<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct FmtResult {
    ok: bool,
    check: bool,
    scripts: u64,
    changed: Vec<String>,
    failed: u64,
    errors: Vec<String>,
    notices: Notices,
}

/// The three actions answer with different members; `ok` is the one they share.
#[derive(Serialize, JsonSchema)]
pub(crate) struct TypesResult {
    ok: bool,
    /// generate
    entities: Option<u64>,
    data_shapes: Option<u64>,
    services: Option<u64>,
    files_written: Option<u64>,
    skipped: Option<Vec<Value>>,
    gitignore_note: Option<String>,
    /// check
    findings: Option<u64>,
    services_with_findings: Option<u64>,
    seconds: Option<f64>,
    by_code: Option<BTreeMap<String, u64>>,
    first: Option<Vec<Value>>,
    findings_list: Option<Vec<Value>>,
    /// platform
    templates: Option<u64>,
    shapes: Option<u64>,
    resources: Option<u64>,
    notices: Notices,
}

/// A plan and an applied push answer with different members; `entity` and `dry_run` are shared.
#[derive(Serialize, JsonSchema)]
pub(crate) struct PushResult {
    entity: String,
    dry_run: bool,
    force: Option<bool>,
    would: Option<String>,
    refusal: Option<String>,
    code: Option<String>,
    pushed: Option<bool>,
    created: Option<bool>,
    note: Option<String>,
    backup: Option<String>,
    notices: Notices,
}

/// A deploy that ran, one the offline gates stopped, and one the server refused each answer
/// differently; `ok` and the stage it reached tell them apart.
#[derive(Serialize, JsonSchema)]
pub(crate) struct DeployResult {
    ok: bool,
    /// Absent when the deploy completed; otherwise where it stopped.
    stage: Option<String>,
    dry_run: Option<bool>,
    note: Option<String>,
    blocking_gates: Option<Vec<Value>>,
    projects: Option<Vec<Value>>,
    scripts_parsed: Option<u64>,
    entities: Option<Value>,
    calls: Option<Vec<Value>>,
    backup: Option<String>,
    imported: Option<Value>,
    kept: Option<u64>,
    changed_by_deploy_step: Option<Vec<String>>,
    plans: Option<Vec<Value>>,
    failures: Option<Vec<Value>>,
    refused: Option<Vec<Value>>,
    not_kept: Option<Vec<Value>>,
    notices: Notices,
}
