//! The successful result of each tool, published as its `outputSchema`.
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
pub(crate) struct InitFromServerResult {
    projects: Vec<String>,
    files: usize,
    entities: BTreeMap<String, usize>,
    toml: String,
    notes: Vec<String>,
    applied: bool,
    added: bool,
    recorded: usize,
    not_in_sync: Vec<String>,
    localization_files: usize,
    localization_tokens: usize,
    localization_failure: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ContractDiffResult {
    summary: String,
    breaking: Vec<crate::core::contract::Change>,
    compatible: Vec<crate::core::contract::Change>,
    skipped: Vec<String>,
    limits: String,
    text: String,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct RequirementsInitResult {
    summary: String,
    dry_run: bool,
    written: bool,
    file: String,
    draft: String,
    requirements: RequirementsCheckResult,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct RequirementsCheckResult {
    summary: String,
    ok: bool,
    entities: usize,
    collections: BTreeMap<String, usize>,
    extensions: usize,
    projects: usize,
    platform: Option<String>,
    server_checked: bool,
    total_findings: usize,
    findings: Vec<RequirementsFinding>,
}

#[derive(Serialize, JsonSchema)]
struct RequirementsFinding {
    rule: String,
    requirement: String,
    blocking: bool,
    message: String,
    installed: Option<String>,
    referrers: Option<Vec<String>>,
}

/// `network` list or show.  The action determines whether `networks` or the hierarchy members
/// are populated.
#[derive(Serialize, JsonSchema)]
pub(crate) struct NetworkResult {
    networks: Option<Vec<Value>>,
    name: Option<String>,
    project: Option<String>,
    nodes: Option<usize>,
    roots: Option<usize>,
    depth: Option<usize>,
    templates: Option<Vec<Value>>,
    tree: Option<Vec<Value>>,
    connections: Option<Vec<Value>>,
    problems: Option<Vec<String>>,
    note: Option<String>,
    matches: Option<bool>,
    server_only: Option<Vec<Value>>,
    repository_only: Option<Vec<Value>>,
    different: Option<Vec<Value>>,
}
#[derive(Serialize, JsonSchema)]
pub(crate) struct NetworkWriteResult {
    dry_run: bool,
    applied: bool,
    file: String,
    changed: Option<bool>,
    removed: Option<Vec<String>>,
    created: Option<Vec<Value>>,
    added: Option<Vec<String>>,
    moved: Option<Vec<String>>,
    noops: Option<Vec<String>>,
    connections: Option<usize>,
    connections_added: Option<usize>,
    connections_removed: Option<usize>,
    connections_moved: Option<usize>,
    outside_repository: Option<usize>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ProjectsResult {
    solution: String,
    projects: Vec<ProjectEntry>,
    unreadable: Vec<String>,
    misfiled: Vec<MisfiledEntity>,
}

#[derive(Serialize, JsonSchema)]
struct ProjectEntry {
    name: String,
    root: String,
    depends_on: Vec<String>,
    entities: u64,
}

#[derive(Serialize, JsonSchema)]
struct MisfiledEntity {
    name: String,
    project: String,
    found_under: String,
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
    skipped: Option<String>,
    advisory: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct StatusResult {
    ok: bool,
    /// How many baselines were recorded; null unless asked to record. Counts and verdicts
    /// describe the state after recording, including the matching entities now in-sync.
    recorded: Option<u64>,
    entities: u64,
    counts: BTreeMap<String, u64>,
    failures: Vec<String>,
    unreadable: Vec<String>,
    /// The export used as the ancestor instead of the baseline, when one was given.
    ancestor: Option<StatusAncestor>,
    /// Without detail: only what needs attention.
    attention: Option<Vec<StatusEntry>>,
    /// With detail: every entity.
    statuses: Option<Vec<StatusEntry>>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct PullResult {
    dry_run: bool,
    entities: Vec<String>,
    recorded: u64,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
struct StatusAncestor {
    file: String,
    entities: u64,
}

#[derive(Serialize, JsonSchema)]
struct StatusEntry {
    server_owned: Option<Vec<String>>,
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
    server_owned: Vec<String>,
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

/// A deploy that ran, one the gates stopped (`blocking_gates` says what each found), and one the server refused each answer
/// differently; `ok` and the stage it reached tell them apart.
#[derive(Serialize, JsonSchema)]
pub(crate) struct DeployResult {
    /// Each existing entity whose import preserves server-owned parts, with its part labels.
    server_owned: Option<Vec<ServerOwnedEntity>>,
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
    network_connections: Option<Vec<Value>>,
    /// Agents imported before the bundles without the items the deploy's namespaces drop.
    agents_narrowed_first: Option<Vec<Value>>,
    backup: Option<String>,
    imported: Option<Value>,
    kept: Option<u64>,
    changed_by_deploy_step: Option<Vec<String>>,
    /// The localization tables a whole-solution deploy carries: each table that differs.
    localization: Option<Vec<Value>>,
    /// With stage `localization`: whether the entities were imported before the tables failed.
    entities_deployed: Option<bool>,
    plans: Option<Vec<Value>>,
    failures: Option<Vec<Value>>,
    refused: Option<Vec<Value>>,
    not_kept: Option<Vec<Value>>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
struct ServerOwnedEntity {
    entity: String,
    /// Owned parts the import carries from the server.
    parts: Vec<String>,
    /// Owned parts holding an encrypted value, which the import clears; a deploy hook sets them.
    clears: Vec<String>,
}

// ---- entities and permissions ---------------------------------------------------------------

/// A plan and an applied delete share every member; `plan` or `applied` says which it was.
#[derive(Serialize, JsonSchema)]
pub(crate) struct EntityDeleteResult {
    ok: bool,
    entities: Vec<Value>,
    dependency_limit: String,
    plan: Option<bool>,
    applied: Option<bool>,
    backup: Option<String>,
    /// The date the rename ledger was marked with, when an applied delete changed it.
    ledger_marked: Option<String>,
    /// Present when the deprecated `force` was used.
    deprecated: Option<String>,
    notices: Notices,
}

/// With no set: the backup sets. With one: what restoring it does, or did.
#[derive(Serialize, JsonSchema)]
pub(crate) struct EntityRestoreResult {
    ok: bool,
    sets: Option<Vec<BackupSetEntry>>,
    set: Option<String>,
    entities: Option<Vec<Value>>,
    plan: Option<bool>,
    applied: Option<bool>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
struct BackupSetEntry {
    id: String,
    created: String,
    reason: String,
    entities: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct EntityCarryResult {
    ok: bool,
    entities: Vec<Value>,
    plan: Option<bool>,
    applied: Option<bool>,
    ledger_marked: Option<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct PermissionsResult {
    ok: bool,
    applied: bool,
    differs: u64,
    entities: Vec<Value>,
    notices: Notices,
}

/// Entities and the platform grants answer with different members; `ok` and `applied` are shared.
#[derive(Serialize, JsonSchema)]
pub(crate) struct PermissionsPushResult {
    ok: bool,
    applied: bool,
    /// entities
    differs: Option<u64>,
    entities: Option<Vec<Value>>,
    /// How many baselines an applied push recorded.
    recorded: Option<u64>,
    /// platform
    missing: Option<u64>,
    items: Option<Vec<Value>>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct PermissionsAuditResult {
    ok: bool,
    server: bool,
    errors: u64,
    warnings: u64,
    projects: Vec<Value>,
    without_policy: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct PermissionsApplyResult {
    ok: bool,
    remaining_errors: u64,
    files: u64,
    projects: Vec<Value>,
    without_policy: Vec<String>,
    plan: Option<bool>,
    applied: Option<bool>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct PermissionsInitResult {
    ok: bool,
    drafts: Vec<PermissionsDraft>,
    plan: Option<bool>,
    applied: Option<bool>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
struct PermissionsDraft {
    project: String,
    path: String,
    source: String,
    text: String,
    notes: Vec<String>,
}

// ---- data -----------------------------------------------------------------------------------

/// What `db_run` and `db_query` report: the SQL, where it ran and, once run, what it returned.
#[derive(Serialize, JsonSchema)]
pub(crate) struct DbResult {
    applied: bool,
    mode: Value,
    thing: String,
    jdbc_url: String,
    user: String,
    bytes: u64,
    sql: String,
    temporary_thing: Option<String>,
    /// The server's reply; a query adds `total_rows` and `columns` to it.
    result: Option<Value>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct DatatableCopyResult {
    ok: bool,
    report: Value,
    plan: Option<bool>,
    applied: Option<bool>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct DbCleanResult {
    ok: bool,
    things: Vec<Value>,
    plan: Option<bool>,
    applied: Option<bool>,
    notices: Notices,
}

/// Each action answers with different members; `thing` and `table` are shared.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ConfigTableResult {
    thing: String,
    table: String,
    /// read, diff, backup
    rows: Option<u64>,
    /// read
    primary_key: Option<Value>,
    first_rows: Option<Vec<Value>>,
    values: Option<Vec<Value>>,
    /// diff, compare
    identical: Option<bool>,
    /// `key`, or `position` when the table has no key or rows share one.
    matching: Option<String>,
    matching_reason: Option<String>,
    differences: Option<Vec<Value>>,
    /// backup
    backup: Option<String>,
    next: Option<String>,
    /// restore
    dry_run: Option<bool>,
    rows_written: Option<Value>,
    rows_removed: Option<Value>,
    note: Option<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct LogsResult {
    ok: bool,
    log: String,
    from: String,
    to: String,
    entries: u64,
    truncated: bool,
    by_level: BTreeMap<String, u64>,
    top_origins: Vec<Value>,
    repeated: Vec<Value>,
    note: Option<String>,
    truncated_note: Option<String>,
    /// Without detail: the newest entries.
    newest: Option<Vec<Value>>,
    /// With detail: every entry.
    entries_list: Option<Vec<Value>>,
}

/// A read answers with `levels`; a change with what it does, or did, and how to undo it.
#[derive(Serialize, JsonSchema)]
pub(crate) struct LogLevelResult {
    ok: bool,
    log: String,
    levels: Option<Value>,
    dry_run: Option<bool>,
    change: Option<Value>,
    before: Option<Value>,
    after: Option<Value>,
    undo: Option<Value>,
    note: Option<String>,
    notices: Notices,
}

/// A plan names the call; a call that ran answers with its result, summarised by shape.
#[derive(Serialize, JsonSchema)]
pub(crate) struct CallResult {
    dry_run: bool,
    would_call: Option<Value>,
    note: Option<String>,
    /// A reply that is not an InfoTable, whole, or `void`.
    result: Option<Value>,
    /// An InfoTable reply, without detail.
    rows: Option<u64>,
    fields: Option<Vec<String>>,
    first_rows: Option<Vec<Value>>,
    /// A long reply, without detail.
    result_bytes: Option<u64>,
    result_head: Option<String>,
    /// With with_logs: the entries logged during the call, or why they could not be read.
    logs: Option<Value>,
    /// Why a call without dry_run ran: "platform" (a built-in platform read) or "twaco.toml" (listed in [call] read_only).
    read_only: Option<String>,
    notices: Notices,
}

// ---- server content -------------------------------------------------------------------------

/// Each action answers with different members; `ok` is the one they share.
#[derive(Serialize, JsonSchema)]
pub(crate) struct RepoResult {
    ok: bool,
    /// list
    repositories: Option<Vec<Value>>,
    repository: Option<String>,
    /// ls
    folders: Option<Vec<Value>>,
    files: Option<Vec<Value>>,
    file_count: Option<u64>,
    /// get
    path: Option<String>,
    size: Option<u64>,
    sha256: Option<String>,
    text: Option<String>,
    truncated: Option<bool>,
    binary: Option<bool>,
    /// status
    local: Option<String>,
    counts: Option<BTreeMap<String, u64>>,
    in_sync: Option<bool>,
    attention: Option<Vec<Value>>,
    note: Option<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct RepoWriteResult {
    ok: bool,
    repository: String,
    dry_run: bool,
    /// push, pull
    local: Option<String>,
    copied: Option<Value>,
    same: Option<Value>,
    left_alone: Option<Value>,
    /// put, mkdir, rm, mv
    change: Option<Value>,
    applied: Option<bool>,
    /// mv, rm: what happened to the copy under filerepository/
    local_copy: Option<String>,
    note: Option<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct LocalizationResult {
    root: String,
    tables: Vec<Value>,
    problem_counts: Value,
    unreadable: Vec<Value>,
    /// With detail: the tokens that are not the same. Differing tokens carry `different`,
    /// `local`, and `server`.
    compared: Option<Vec<Value>>,
    /// With detail: each problem.
    problems: Option<Vec<Value>>,
    notices: Notices,
}

/// Pull and the local edits answer with `files`; push with `tables`.
#[derive(Serialize, JsonSchema)]
pub(crate) struct LocalizationWriteResult {
    dry_run: bool,
    files: Option<Vec<Value>>,
    tables: Option<Vec<Value>>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct UsersResult {
    ready: bool,
    users: Vec<Value>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct UsersWriteResult {
    dry_run: bool,
    steps: Vec<String>,
    users: Vec<Value>,
}

/// A planned or created key, or one read back. `key` is the key itself, only when asked for.
#[derive(Serialize, JsonSchema)]
pub(crate) struct AppKeyResult {
    dry_run: bool,
    created: Option<bool>,
    name: String,
    user: Option<String>,
    expires: Option<String>,
    project: Option<String>,
    whitelist: Option<Vec<String>>,
    saved_to: Option<String>,
    key: Option<String>,
}

/// A dry run lists the cases; a run answers with counts and each call.
#[derive(Serialize, JsonSchema)]
pub(crate) struct TestResult {
    dry_run: bool,
    cases: Option<Vec<Value>>,
    passed: Option<u64>,
    failed: Option<u64>,
    calls: Option<Vec<Value>>,
}

/// list answers with `packages`; show with one package and its extensions.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ExtensionsResult {
    ok: bool,
    packages: Option<Vec<Value>>,
    package: Option<Value>,
    extensions: Option<Value>,
    in_use: Option<Value>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ExtensionWriteResult {
    ok: bool,
    dry_run: bool,
    change: Value,
    applied: bool,
    note: Option<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct BundleResult {
    ok: bool,
    /// current, out_of_date, missing or written.
    state: String,
    bundle: String,
    /// When the bundle was built: current or written.
    entities: Option<u64>,
    files: Option<u64>,
    next: Option<String>,
    notes: Option<Vec<Value>>,
    deleted: Option<Vec<String>>,
    unmatched_excludes: Option<Vec<String>>,
    left_out_references: Option<Vec<String>>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct SearchResult {
    ok: bool,
    count: u64,
    more: bool,
    entities: Vec<Value>,
    expression: Option<Value>,
    note: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct EntityGetResult {
    ok: bool,
    entity: String,
    bytes: u64,
    xml: String,
    truncated: Option<bool>,
    note: Option<String>,
}

/// An XML export answers with the file it wrote; a source-control export with its plan.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ExportResult {
    ok: bool,
    /// XML
    out: Option<String>,
    bytes: Option<u64>,
    entities: Option<Vec<Value>>,
    /// source control
    dry_run: Option<bool>,
    change: Option<Value>,
    download: Option<Value>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct PackageResult {
    ok: bool,
    out: String,
    bytes: u64,
    detail: PackageDetail,
}

/// What the package holds; which members it has depends on the action.
#[derive(Serialize, JsonSchema)]
struct PackageDetail {
    entities: Option<u64>,
    files: Option<u64>,
    editable: Option<bool>,
    version: Option<String>,
    /// A solution's extension: each project's package.
    projects: Option<Vec<Value>>,
    deleted: Option<Vec<String>>,
    unmatched_excludes: Option<Vec<String>>,
    left_out_references: Option<Vec<String>>,
}

/// A file's import answers with what it adds, replaces, and preserves; a source-control one
/// with the server's diff.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ImportResult {
    ok: bool,
    dry_run: bool,
    /// file
    adds: Option<Vec<String>>,
    replaces: Option<Vec<String>>,
    applied: Option<bool>,
    /// Existing XML entities whose import carries server-owned parts.
    server_owned: Option<Vec<ServerOwnedEntity>>,
    /// source control
    entities: Option<Value>,
    differ: Option<Vec<Value>>,
    still_differ: Option<Vec<Value>>,
    notices: Notices,
}

// ---- knowledge and reference ----------------------------------------------------------------

/// Each action answers with different members; `ok` is the one they share.
#[derive(Serialize, JsonSchema)]
pub(crate) struct SettingsResult {
    ok: bool,
    /// list
    subsystems: Option<Vec<Value>>,
    /// show
    subsystem: Option<String>,
    running: Option<Value>,
    tables: Option<Vec<Value>>,
    /// search
    matches: Option<Vec<Value>>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct CatalogResult {
    ok: bool,
    service_count: u64,
    entity_count: u64,
    services: Vec<Value>,
    note: Option<String>,
    skipped: Option<Vec<Value>>,
}

/// The JSON form and the Graphviz form share the subject and how complete the reading was.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ImpactResult {
    entity: String,
    complete: bool,
    unreadable: Vec<Value>,
    unparsed_scripts: Vec<String>,
    limits: String,
    /// format dot
    dot: Option<String>,
    /// format json
    project: Option<String>,
    member: Option<String>,
    min_confidence: Option<String>,
    depth: Option<u64>,
    dependents: Option<Vec<Value>>,
    counts: Option<BTreeMap<String, u64>>,
    inherited_by: Option<Vec<Value>>,
    projects: Option<Vec<Value>>,
    notes: Option<Vec<String>>,
    /// server: true
    server: Option<Value>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct TraceResult {
    service: String,
    /// Where the definition was read, when it is inherited.
    defined_in: Option<String>,
    parameters: Vec<Value>,
    returns: String,
    mashups: Vec<Value>,
    scripts: Vec<Value>,
    calls: Vec<String>,
    notes: Vec<String>,
}

/// Unused entities, or with `fields` unused DataShape fields; `judged` and `limits` are shared.
#[derive(Serialize, JsonSchema)]
pub(crate) struct UnusedResult {
    judged: u64,
    limits: String,
    /// entities
    min_confidence: Option<String>,
    roots: Option<Vec<Value>>,
    reachable: Option<u64>,
    unused: Option<Vec<Value>>,
    keep_unmatched: Option<Vec<String>>,
    complete: Option<bool>,
    unreadable: Option<Vec<Value>>,
    unparsed_scripts: Option<Vec<String>>,
    /// fields
    bound_whole: Option<u64>,
    data_shapes: Option<Vec<Value>>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct DocsResult {
    document: Value,
    markdown: String,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct DoctorResult {
    ok: bool,
    failed: u64,
    items: Vec<DoctorItem>,
}

#[derive(Serialize, JsonSchema)]
struct DoctorItem {
    /// ok, warn or fail.
    health: String,
    subject: String,
    detail: String,
}

/// Each action answers with different members; `ok` is the one they share.
#[derive(Serialize, JsonSchema)]
pub(crate) struct GuideResult {
    ok: bool,
    /// list
    topics: Option<Vec<Value>>,
    /// search
    results: Option<Vec<Value>>,
    next: Option<String>,
    /// read
    topic: Option<String>,
    markdown: Option<String>,
    title: Option<String>,
    /// A topic too long to read whole: its headings.
    sections: Option<Vec<Value>>,
    note: Option<String>,
    /// Topics of the solution that could not be read.
    problems: Option<Vec<Value>>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct HelpSearchResult {
    ok: bool,
    version: String,
    matched: u64,
    results: Vec<Value>,
    unknown_words: Option<Vec<String>>,
    note: Option<String>,
    notes: Option<Vec<String>>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct HelpPageResult {
    ok: bool,
    version: String,
    title: String,
    url: String,
    headings: Vec<String>,
    markdown: String,
    truncated: Option<bool>,
    note: Option<String>,
    notes: Option<Vec<String>>,
}

/// What `ext_deploy` built and sent.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ExtDeployResult {
    /// False when the installed package does not list an entity metadata.xml declares.
    ok: bool,
    dry_run: bool,
    /// The [[extension]] entry, and its folder relative to the solution.
    extension: String,
    root: String,
    package: String,
    /// The version sent: the bumped one once applied.
    version: String,
    /// The server's version, when it has the package.
    installed: Option<String>,
    /// `x.y.z -> x.y.z'`: done when applied, else what dry_run false would do.
    bump: Option<String>,
    /// The build command run.
    build: Option<String>,
    /// The package zip, relative to the solution.
    zip: String,
    plan: String,
    /// Whether the server validated the zip; not while it carries the installed version.
    validated: bool,
    applied: bool,
    /// Installed at the server's next restart.
    queued: bool,
    /// Entities metadata.xml declares that the installed package does not list.
    missing: Vec<String>,
}

/// The files `ext_new` or `ext_add` writes, or wrote.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ExtScaffoldResult {
    ok: bool,
    dry_run: bool,
    applied: bool,
    /// The extension project's folder.
    root: String,
    summary: String,
    /// Each with path and action (create, or update for metadata.xml).
    files: Vec<Value>,
    notes: Vec<String>,
    /// When applied: ext_check's report on the result.
    check: Option<Value>,
}

/// A Java extension project's findings, offline.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ExtCheckResult {
    ok: bool,
    /// No finding.
    passed: bool,
    summary: String,
    package: Option<String>,
    version: Option<String>,
    classes: usize,
    /// Each with file, line (0: the whole file), rule and message.
    findings: Vec<Value>,
    notes: Vec<String>,
}

/// search answers with `results`; class with the class's page.
#[derive(Serialize, JsonSchema)]
pub(crate) struct JavadocResult {
    ok: bool,
    summary: String,
    version: String,
    results: Option<Vec<Value>>,
    title: Option<String>,
    path: Option<String>,
    url: Option<String>,
    markdown: Option<String>,
    /// action service: the reference, normalised
    asked: Option<String>,
    /// action service: the class whose page documents the member
    declared_in: Option<String>,
    /// action service: whether that class is one the reference's class extends
    inherited: Option<bool>,
    /// action service: the named class is not in the Javadoc, and a more general one answered
    note: Option<String>,
}

// ---- refactoring and authoring --------------------------------------------------------------

#[derive(Serialize, JsonSchema)]
pub(crate) struct MoveMemberResult {
    ok: bool,
    action: Value,
    member: Value,
    from: String,
    to: String,
    name: String,
    /// The member's name where it lands.
    #[serde(rename = "as")]
    final_name: Value,
    files: Vec<String>,
    /// The references that stop resolving: how many, and the first few.
    callers: Value,
    notes: Vec<String>,
    out_of_step: Vec<Value>,
    plan: Option<bool>,
    applied: Option<bool>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct NewBuildingBlockResult {
    ok: bool,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    root: Value,
    files: Vec<String>,
    twaco_toml: Value,
    notes: Vec<Value>,
    plan: Option<bool>,
    applied: Option<bool>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct EntityNewResult {
    applied: bool,
    dry_run: bool,
    kind: String,
    name: String,
    project: String,
    files: Vec<String>,
    /// A timer's period.
    every_ms: Option<u64>,
    /// A scheduler's schedule.
    cron: Option<String>,
    /// The first line of the script it runs.
    runs: Option<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct EntityEditResult {
    entity: String,
    summary: String,
    applied: bool,
    dry_run: bool,
    files: Vec<String>,
    warnings: Vec<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct RetemplateResult {
    ok: bool,
    entity: String,
    collection: String,
    file: String,
    affected: Vec<String>,
    gained: Vec<Value>,
    lost: Vec<Value>,
    /// The losses that refuse the change unless accept_loss is true.
    needs_accept_loss: Vec<String>,
    notes: Vec<String>,
    plan: Option<bool>,
    applied: Option<bool>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ReprojectResult {
    applied: bool,
    dry_run: bool,
    to: String,
    moves: Vec<Value>,
    /// References the move puts outside the deploy order.
    deploy_order: Vec<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct MashupDescribeResult {
    mashup: String,
    file: String,
    mashup_type: String,
    parameters: Vec<Value>,
    containers: Option<Vec<Value>>,
    functions: Option<Vec<Value>>,
    widgets: Vec<Value>,
    services: Vec<Value>,
    bindings: Vec<Value>,
    events: Vec<Value>,
    warnings: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct MashupValidateResult {
    problems: u64,
    mashups: Vec<MashupValidateEntry>,
}

#[derive(Serialize, JsonSchema)]
struct MashupValidateEntry {
    mashup: String,
    file: String,
    problems: Vec<Value>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct MashupNewResult {
    applied: bool,
    dry_run: bool,
    mashup: String,
    project: String,
    template: String,
    layout: String,
    slots: Vec<String>,
    master: bool,
    theme: Option<String>,
    visibility: Vec<String>,
    files: Vec<String>,
    widgets: Vec<Value>,
    menus: Vec<Value>,
    notices: Notices,
}

/// The templates, or with a name one template's slot tree.
#[derive(Serialize, JsonSchema)]
pub(crate) struct MashupTemplatesResult {
    templates: Option<Vec<Value>>,
    name: Option<String>,
    aliases: Option<Vec<String>>,
    summary: Option<String>,
    slots: Option<Vec<String>>,
    page: Option<Value>,
    master_slot: Option<String>,
    children: Option<Vec<Value>>,
}

/// What `mashup_add` and `mashup_add_contained` report: the widget placed and where.
#[derive(Serialize, JsonSchema)]
pub(crate) struct MashupAddResult {
    applied: bool,
    dry_run: bool,
    mashup: String,
    widget: String,
    id: String,
    containers: Vec<Value>,
    flipped: Option<Value>,
    wrapped: Vec<Value>,
    /// A contained mashup: the child and its visible parameters.
    child: Option<String>,
    parameters: Option<Vec<Value>>,
    files: Vec<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct MashupBindResult {
    applied: bool,
    dry_run: bool,
    mashup: String,
    from: String,
    to: String,
    /// The data call the binding created, when it created one.
    new_source: Option<String>,
    warnings: Vec<String>,
    refreshed: Vec<Value>,
    files: Vec<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct MashupOnResult {
    applied: bool,
    dry_run: bool,
    mashup: String,
    event: String,
    invoke: String,
    new_source: Option<String>,
    files: Vec<String>,
    notices: Notices,
}

/// The catalogue, or with a widget that widget's contract.
#[derive(Serialize, JsonSchema)]
pub(crate) struct WidgetsResult {
    widgets: Option<Vec<Value>>,
    #[serde(rename = "type")]
    kind: Option<String>,
    type_display_name: Option<String>,
    aliases: Option<Vec<String>>,
    container: Option<bool>,
    bindable: Option<bool>,
    auto_resize: Option<bool>,
    properties: Option<Vec<Value>>,
    events: Option<Vec<String>>,
    services: Option<Vec<String>>,
    defaults_count: Option<u64>,
    note: Option<String>,
}

/// What `mashup_param` and `mashup_event` report: a parameter and every mashup it touched.
#[derive(Serialize, JsonSchema)]
pub(crate) struct MashupParameterResult {
    applied: bool,
    dry_run: bool,
    action: String,
    mashup: String,
    name: String,
    base_type: String,
    direction: String,
    containers_refreshed: Vec<Value>,
    bindings_retyped: Vec<String>,
    bindings_removed: Vec<String>,
    files: Vec<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct MashupRecipeResult {
    applied: bool,
    dry_run: bool,
    mashup: String,
    recipe: String,
    added: Vec<String>,
    wired: Vec<String>,
    files: Vec<String>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct AdoptReportResult {
    notes: Option<Vec<String>>,
    suggestions: Option<Vec<Value>>,
    reverts: Option<u64>,
    services: Option<Vec<Value>>,
    unmatched_services: Option<Vec<Value>>,
    new: Option<Vec<Value>>,
    absent: Option<Vec<String>>,
    changed: Option<Vec<Value>>,
    type_changes: Option<Vec<Value>>,
    retyped_arguments: Option<Vec<Value>>,
    retyped_defaults: Option<Vec<Value>>,
    identical: Option<u64>,
    base: Option<Value>,
    entities: Option<Vec<Value>>,
    notices: Option<Notices>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct AdoptApplyResult {
    /// What was written, one line each.
    applied: Vec<String>,
    reverts_not_applied: u64,
    /// Literal arguments and defaults of retyped service parameters that the new base type does
    /// not take: left as written, for a person to change.
    unmapped: Vec<Value>,
    base: Value,
    entities: Vec<Value>,
    services: Vec<Value>,
    next: String,
    types_refreshed: Option<u64>,
    types_warning: Option<String>,
    notes: Option<Vec<String>>,
    notices: Notices,
}

/// list answers with `handoffs`; record with its plan, or the handoff it recorded.
#[derive(Serialize, JsonSchema)]
pub(crate) struct HandoffResult {
    ok: bool,
    handoffs: Option<Vec<Value>>,
    plan: Option<bool>,
    name: Option<String>,
    files: Option<Vec<String>>,
    recorded: Option<Value>,
    notices: Notices,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct RenameResult {
    spec: Value,
    /// Pass it back with dry_run false to apply exactly this plan.
    plan_digest: String,
    moves: Value,
    tables: Value,
    counts: Value,
    review: Vec<Value>,
    named: Value,
    outside_applied: bool,
    applied: bool,
    verification: Option<Value>,
    follow_up: Value,
    sql: Option<Value>,
    skipped: u64,
    notices: Notices,
}
