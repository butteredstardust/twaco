use super::outputs;
use super::requests::common::{parse, schema, NoArguments};
use super::requests::{
    block as block_requests, content as content_requests, data as data_requests,
    entity as entity_requests, info as info_requests, init as init_requests,
    network as network_requests, refactor as refactor_requests,
    requirements as requirements_requests, source as source_requests,
};
use super::{
    block, content, data, entity, info, init, network, refactor, requirements, source, Protocol,
    ToolError,
};
use crate::core::config::Solution;
use crate::core::progress::Progress;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::LazyLock;

type Call = Box<dyn Fn(&Path, &Value, &dyn Progress) -> Result<Value, ToolError> + Send + Sync>;

/// A typed MCP tool: its published definition, how its arguments are read and where they go.
/// The schema, the parser and the route are built from one request type, so they cannot drift
/// apart.
pub(crate) struct Tool {
    name: &'static str,
    description: &'static str,
    read_only: bool,
    input_schema: Value,
    output_schema: Option<Value>,
    #[cfg(test)]
    check: fn(&Value, &Value) -> Result<(), String>,
    call: Call,
}

impl Tool {
    /// The tool's entry in `tools/list`. An output schema is published only to clients that
    /// negotiated a protocol revision that has them.
    fn definition(&self, protocol: Protocol) -> Value {
        let mut definition = json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": annotations(self.name, self.read_only),
        });
        if protocol.has_structured_output() {
            if let Some(output_schema) = &self.output_schema {
                definition["outputSchema"] = output_schema.clone();
            }
        }
        definition
    }

    /// Publish an output schema for a stable shape of this tool's result.
    fn with_output<T: JsonSchema>(mut self) -> Self {
        self.output_schema = Some(schema::<T>());
        self
    }
}

/// Tools that may reach beyond the solution on some path: a ThingWorx server, or the
/// Platform help and Javadoc sites. `check` and `types` do so only when asked (`live`,
/// `platform`), which is still "may".
const OPEN_WORLD: &[&str] = &[
    "init_from_server",
    "types",
    "check",
    "status",
    "entity_pull",
    "push",
    "entity_delete",
    "entity_restore",
    "entity_carry",
    "permissions",
    "permissions_audit",
    "requirements_check",
    "permissions_push",
    "db_run",
    "db_query",
    "datatable_copy",
    "db_clean",
    "deploy",
    "config_table",
    "logs",
    "log_level",
    "repo",
    "repo_write",
    "localization",
    "localization_write",
    "users",
    "users_write",
    "appkey",
    "test",
    "extensions",
    "extension_write",
    "ext_deploy",
    "impact",
    "search",
    "model_resolve",
    "entity_inspect",
    "access_explain",
    "entity_get",
    "export",
    "import",
    "settings",
    "doctor",
    "help_search",
    "help_page",
    "javadoc",
    "call",
    "network",
    "network_write",
    "block",
    "ext_build",
];

/// Writing tools whose changes only add or regenerate: they create files that did not exist,
/// or rewrite derived files (declarations, the bundle, the baseline, formatting) that the
/// repository can produce again. Every other writing tool may discard something.
const ADDITIVE: &[&str] = &[
    "init_from_server",
    "types",
    "fmt",
    "status",
    "bundle",
    "handoff",
    "new_building_block",
    "entity_new",
    "entity_pull",
    "mashup_new",
    "permissions_init",
    "requirements_init",
    "ext_new",
    "ext_add",
];

/// Writing tools that, called again with the same arguments, change nothing more.
const IDEMPOTENT: &[&str] = &[
    "types",
    "fmt",
    "extract",
    "sync",
    "status",
    "bundle",
    "entity_pull",
];

/// The MCP behaviour hints for a tool. Clients use them to decide what to ask a person before
/// running it, so a hint errs towards caution: unlisted means destructive, not idempotent.
fn annotations(name: &str, read_only: bool) -> Value {
    let mut hints = json!({
        "readOnlyHint": read_only,
        "openWorldHint": OPEN_WORLD.contains(&name),
    });
    // The spec gives these meaning only for a tool that writes.
    if !read_only {
        hints["destructiveHint"] = json!(!ADDITIVE.contains(&name));
        hints["idempotentHint"] = json!(IDEMPOTENT.contains(&name));
    }
    hints
}

#[cfg(test)]
pub(crate) const HINT_LISTS: [(&str, &[&str]); 3] = [
    ("OPEN_WORLD", OPEN_WORLD),
    ("ADDITIVE", ADDITIVE),
    ("IDEMPOTENT", IDEMPOTENT),
];

/// Read the arguments into a request, answering a bad one as a tool error the agent can correct.
fn read<T: DeserializeOwned>(schema: &Value, arguments: &Value) -> Result<T, ToolError> {
    parse(schema, arguments)
        .map_err(|error| ToolError::invalid(format!("{error}; nothing was done")))
}

/// Read the arguments, then write the request back out: what a request holds must fit the
/// schema that was published for it.
#[cfg(test)]
fn check<T: DeserializeOwned + Serialize>(schema: &Value, arguments: &Value) -> Result<(), String> {
    let request = parse::<T>(schema, arguments)?;
    let encoded = serde_json::to_value(request).map_err(|error| error.to_string())?;
    if jsonschema::draft202012::is_valid(schema, &encoded) {
        Ok(())
    } else {
        Err(format!(
            "{encoded} does not fit the schema it was read with"
        ))
    }
}

fn tool<T: DeserializeOwned + Serialize + JsonSchema + 'static>(
    name: &'static str,
    description: &'static str,
    read_only: bool,
    call: impl Fn(&Path, T) -> Result<Value, ToolError> + Send + Sync + 'static,
) -> Tool {
    tool_with_progress(name, description, read_only, move |root, request, _| {
        call(root, request)
    })
}

/// A typed tool that reports progress while it runs.
fn tool_with_progress<T: DeserializeOwned + Serialize + JsonSchema + 'static>(
    name: &'static str,
    description: &'static str,
    read_only: bool,
    call: impl Fn(&Path, T, &dyn Progress) -> Result<Value, ToolError> + Send + Sync + 'static,
) -> Tool {
    let input_schema = schema::<T>();
    let parser = input_schema.clone();
    Tool {
        name,
        description,
        read_only,
        input_schema,
        output_schema: None,
        #[cfg(test)]
        check: check::<T>,
        call: Box::new(move |root, arguments, progress| {
            call(root, read(&parser, arguments)?, progress)
        }),
    }
}

/// A tool that runs inside the solution found from the root.
fn solution_tool<T: DeserializeOwned + Serialize + JsonSchema + 'static>(
    name: &'static str,
    description: &'static str,
    read_only: bool,
    route: fn(&Solution, T) -> Result<Value, ToolError>,
) -> Tool {
    tool(name, description, read_only, move |root, request| {
        let solution = Solution::discover(root).map_err(ToolError::coded)?;
        route(&solution, request)
    })
}

/// A tool that runs inside the solution found from the root, and reports progress.
fn solution_progress_tool<T: DeserializeOwned + Serialize + JsonSchema + 'static>(
    name: &'static str,
    description: &'static str,
    read_only: bool,
    route: fn(&Solution, T, &dyn Progress) -> Result<Value, ToolError>,
) -> Tool {
    tool_with_progress(
        name,
        description,
        read_only,
        move |root, request, progress| {
            let solution = Solution::discover(root).map_err(ToolError::coded)?;
            route(&solution, request, progress)
        },
    )
}

/// A tool that needs the solution's root but no solution.
fn root_tool<T: DeserializeOwned + Serialize + JsonSchema + 'static>(
    name: &'static str,
    description: &'static str,
    read_only: bool,
    route: fn(&Path, T) -> Result<Value, ToolError>,
) -> Tool {
    tool(name, description, read_only, move |root, request| {
        route(root, request)
    })
}

/// A tool that needs neither a solution nor its root.
fn bare_tool<T: DeserializeOwned + Serialize + JsonSchema + 'static>(
    name: &'static str,
    description: &'static str,
    read_only: bool,
    route: fn(T) -> Result<Value, ToolError>,
) -> Tool {
    tool(name, description, read_only, move |_, request| {
        route(request)
    })
}

static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(|| {
    vec![
        root_tool::<init_requests::InitFromServerRequest>(
            "init_from_server",
            "Start a repository in an empty root, or add server projects to the solution already there. apply defaults to false, so the result is a plan until apply is true. No path argument: it always uses the MCP server root.",
            false,
            init::init_from_server_tool,
        )
        .with_output::<outputs::InitFromServerResult>(),
        solution_tool::<NoArguments>(
            "projects",
            "The solution's ThingWorx projects, their roots, their deploy order and how many entity documents each holds.",
            true,
            |solution, _| super::projects(solution),
        )
        .with_output::<outputs::ProjectsResult>(),
        solution_progress_tool::<source_requests::TypesRequest>(
            "types",
            "Generate editor declarations, type-check every service, or fetch and cache platform declarations. All actions take the workspace lock.",
            false,
            source::types_tool,
        )
        .with_output::<outputs::TypesResult>(),
        solution_progress_tool::<source_requests::CheckRequest>(
            "check",
            "Run every gate of the solution (line endings, sidecars in sync, formatting, script traps, code order, project validation, structure, declared hooks). The structure gate is offline and advisory when an XSD source is configured. With live: true, every service script is also parsed by the ThingWorx server, and an unreachable server fails the check. live defaults to the solution's [gates] live.",
            true,
            source::check_tool,
        )
        .with_output::<outputs::CheckResult>(),
        solution_progress_tool::<entity_requests::StatusRequest>(
            "status",
            "Compare entities with the server and the recorded baseline: in-sync, local-changed, server-changed, both-changed, not-on-server, or no baseline yet. Lists every entity that needs attention.",
            false,
            entity::status_tool,
        )
        .with_output::<outputs::StatusResult>(),
        solution_progress_tool::<entity_requests::PullRequest>(
            "entity_pull",
            "Take server-changed entity exports into the repository, extract their sidecars and record their baselines. A dry run unless dry_run is false.",
            false,
            entity::pull_tool,
        )
        .with_output::<outputs::PullResult>(),
        solution_tool::<source_requests::SyncRequest>(
            "sync",
            "Write sidecars back into their entity XML: service scripts, DataShape fields, mashup content, DataTable configuration. This is how an edit to a script.js takes effect. check: true reports what would change and writes nothing. Takes the workspace lock while it writes.",
            false,
            source::sync_tool,
        )
        .with_output::<outputs::SyncResult>(),
        solution_tool::<source_requests::ExtractRequest>(
            "extract",
            "Entity XML to sidecars: service scripts, DataShape fields, mashup content, DataTable configuration. Overwrites the sidecars of the entities chosen; takes the workspace lock.",
            false,
            source::extract_tool,
        )
        .with_output::<outputs::ExtractResult>(),
        solution_tool::<source_requests::FmtRequest>(
            "fmt",
            "Format every service script sidecar with the built-in formatter. check: true reports which would change and writes nothing.",
            false,
            source::fmt_tool,
        )
        .with_output::<outputs::FmtResult>(),
        solution_progress_tool::<entity_requests::PushRequest>(
            "push",
            "Import one entity's file to the server. Refuses when the server changed since the last sync, was deleted there, or has no baseline, unless force is true. Reads the entity back and records a baseline only for what the server kept. A dry run unless dry_run is false.",
            false,
            entity::push_tool,
        )
        .with_output::<outputs::PushResult>(),
        solution_tool::<entity_requests::EntityDeleteRequest>(
            "entity_delete",
            "Delete server entities in dependency-safe order. Accepts Collection/Name or a bare name resolved on the server; renamed adds undeleted entity/prefix entries from .twaco/renames.json. allow_repository_defined accepts a repository definition that deployment would recreate; allow_outside_dependents accepts structural dependents outside the delete set; allow_file_repository_data_loss accepts deleting a FileRepository Thing and all its files. A FileRepository delete needs its own acknowledgement. force is deprecated: it means the first two acknowledgements and never FileRepository data loss. A dry run unless dry_run is false; every applied delete is confirmed absent.",
            false,
            entity::entity_delete_tool,
        )
        .with_output::<outputs::EntityDeleteResult>(),
        solution_tool::<entity_requests::EntityRestoreRequest>(
            "entity_restore",
            "List the backup sets taken before deletes and forced overwrites (no set given), or import one back: all its entities, or the named ones. A dry run unless dry_run is false; each import is confirmed on the server.",
            false,
            entity::entity_restore_tool,
        )
        .with_output::<outputs::EntityRestoreResult>(),
        solution_tool::<entity_requests::EntityCarryRequest>(
            "entity_carry",
            "Copy run-time, design-time and visibility permissions from renamed (old) entities to the ones that replaced them, mapping principals through the rename ledger. pairs is a flat list [old, new, old, new, ...] of Collection/Name; renamed adds every pending ledger entity. A dry run unless dry_run is false; every write is read back and the ledger marked carried.",
            false,
            entity::entity_carry_tool,
        )
        .with_output::<outputs::EntityCarryResult>(),
        solution_progress_tool::<entity_requests::PermissionsRequest>(
            "permissions",
            "Compare entities' run-time, design-time and visibility permissions (and the instance permissions of a ThingShape or ThingTemplate) in the repository with the server's. An import only adds: it never removes a grant the server has, and never changes the server's allow or deny for a principal it already lists. Each difference is server-only, repository-only or flipped (allow/deny differs). A permission set the entity XML does not declare is not compared. Read-only.",
            true,
            entity::permissions_tool,
        )
        .with_output::<outputs::PermissionsResult>(),
        solution_tool::<requirements_requests::InitRequest>(
            "requirements_init",
            "Draft requirements.toml offline from the solution's external entities, Project dependsOn extensions and projects, and platform version. Returns the draft; dry_run defaults to true. Apply creates a new file under the workspace lock and refuses an existing requirements.toml; use requirements_check to see what changed.",
            false,
            requirements::init_tool,
        ).with_output::<outputs::RequirementsInitResult>(),
        solution_progress_tool::<requirements_requests::CheckRequest>(
            "requirements_check",
            "Check requirements.toml against the repository. Undeclared dependencies and lower declared versions block; stale entries are notes. With server true, also verify the platform version, installed extensions, projects and external entities with read-only server requests. Server failures block. Summary first; detail includes every finding and referrer.",
            true,
            requirements::check_tool,
        ).with_output::<outputs::RequirementsCheckResult>(),
        solution_tool::<entity_requests::PermissionsInitRequest>(
            "permissions_init",
            "Draft a permissions.toml for each project without one, from what the project grants today, so that permissions_apply then changes nothing but what each draft's notes name: each rule names its roles outright; roles come from the permission helper's RoleGroupsAndOrganizations when the project has one, else from the groups its run-time blocks grant. from_helper takes the grants from the helper's tables instead of the entity XML (a matrix edited in the helper's mashup). An entity with a deny or a non-group principal in its run-time block, or a visibility deny of a role's unit, is left unmanaged, with a note. A dry run (the drafts' text) unless dry_run is false; existing files are never overwritten.",
            false,
            entity::permissions_init_tool,
        )
        .with_output::<outputs::PermissionsInitResult>(),
        solution_progress_tool::<entity_requests::PermissionsAuditRequest>(
            "permissions_audit",
            "Check each project's permissions.toml (its root folder) against the entity XML. Errors: run-time or visibility blocks the policy would change (permissions apply writes them), services of a strict entity no rule classifies, a group or user in visibility (the server answers 500), an Organization in run-time permissions. Warnings: principals under a project of the solution that no entity defines, rules or patterns that match nothing. Notes: explicit denies. With server: true (and a profile), also the server against the repository: each entity's permission sets, the permission helper's tables, the policy's platform grants and memberships, and each role's organizational unit. detail lists the grants behind each finding. Read-only.",
            true,
            entity::permissions_audit_tool,
        )
        .with_output::<outputs::PermissionsAuditResult>(),
        solution_progress_tool::<entity_requests::PermissionsApplyRequest>(
            "permissions_apply",
            "Write each project's permissions.toml into its entity XML: the run-time block of each Thing, the instance run-time block of each ThingShape and ThingTemplate, and the role principals of each visibility block. Only blocks that differ change; the rest of each file is untouched. Refused while a strict entity has a service no rule classifies. remaining lists audit findings the write does not settle. A dry run unless dry_run is false; the files are written in one transaction. Deploy them, then permissions_push, since an import never removes a grant.",
            false,
            entity::permissions_apply_tool,
        )
        .with_output::<outputs::PermissionsApplyResult>(),
        solution_progress_tool::<entity_requests::PermissionsPushRequest>(
            "permissions_push",
            "Make the server's permission sets exactly the repository's: every differing run-time, design-time or visibility set is written whole and read back. Removes server-only grants and corrects flipped allow/deny, which an import cannot do. A set the entity XML does not declare is never written. A dry run unless dry_run is false; an applied push records the baseline of each pushed entity that then matches the server. With platform: true (and no entities), instead add the permissions.toml [[platform]] grants and memberships the server lacks (what DeployComponent does on entities the project does not own); nothing is ever removed.",
            false,
            entity::permissions_push_tool,
        )
        .with_output::<outputs::PermissionsPushResult>(),
        solution_tool::<data_requests::DbRunRequest>(
            "db_run",
            "Run one SQL script as an atomic SQLCommand through a throwaway Database Thing. Plans by default and shows the SQL and sanitized connection target; pass dry_run: false to execute.",
            false,
            data::db_run_tool,
        )
        .with_output::<outputs::DbResult>(),
        solution_tool::<data_requests::DbQueryRequest>(
            "db_query",
            "Run read-only SQLQuery through a throwaway Database Thing. Returns columns and the first 20 rows unless detail is true.",
            true,
            data::db_query_tool,
        )
        .with_output::<outputs::DbResult>(),
        solution_tool::<data_requests::DatatableCopyRequest>(
            "datatable_copy",
            "Copy the rows of one DataTable into the DataTable that replaced it, mapping fields by name, by the rename ledger, or by map ({old: new}). Refuses unmapped fields (unless drop_unmapped), type changes, and a non-empty target (unless append). A dry run unless dry_run is false; the target is read back and compared. Each row's source, tags and timestamp are not carried.",
            false,
            data::datatable_copy_tool,
        )
        .with_output::<outputs::DatatableCopyResult>(),
        solution_tool::<data_requests::DbCleanRequest>(
            "db_clean",
            "Find, and with dry_run false delete, the temporary ZZ.Twaco.Sql.* Database Things an interrupted db_run or db_query left on the server. Only names twaco generates, on Database Things, are touched.",
            false,
            data::db_clean_tool,
        )
        .with_output::<outputs::DbCleanResult>(),
        solution_progress_tool::<source_requests::DeployRequest>(
            "deploy",
            "Deploy the solution: the gates (requirements checked against the server), one bundle per project in dependency order, every script parsed by the server (fails closed), a conflict check per entity, then import, read-back, and the project's deploy and post-import services. A dry run (a plan) unless dry_run is false.",
            false,
            source::deploy_tool,
        )
        .with_output::<outputs::DeployResult>(),
        solution_tool::<refactor_requests::AdoptReportRequest>(
            "adopt_report",
            "Compare a designer's or backend collaborator's Composer <Entities> export with the repository, three-way against its base. Returns which UI and backend entities/services are theirs, stale or conflicting. Writes nothing; reverts is what `twaco adopt --fail-on-revert` fails on.",
            true,
            refactor::adopt_tool,
        )
        .with_output::<outputs::AdoptReportResult>(),
        solution_tool::<refactor_requests::AdoptApplyRequest>(
            "adopt_apply",
            "Apply a designer's or backend collaborator's export, three-way against its base: writes selected UI content and safe backend entity/service changes; conflicts need take. Backend changes are folded into entity XML; mashup sidecars may need sync. Takes the workspace lock.",
            false,
            refactor::adopt_apply_tool,
        )
        .with_output::<outputs::AdoptApplyResult>(),
        solution_tool::<refactor_requests::HandoffRequest>(
            "handoff",
            "List recorded collaborator handoffs, or record delivered <Entities> exports as a future three-way adopt base. Record is a dry run unless dry_run is false; applying copies files under .twaco/handoffs and takes the workspace lock.",
            false,
            refactor::handoff_tool,
        )
        .with_output::<outputs::HandoffResult>(),
        solution_tool::<refactor_requests::RenameRequest>(
            "rename",
            "Rename one entity, a dotted project/building-block prefix, a DataShape field, a declared service, one service parameter, or a configuration table. A dry run unless dry_run is false; an apply takes the workspace lock. The result carries a plan_digest: pass it back with dry_run false to apply exactly the plan that was reviewed.",
            false,
            refactor::rename_tool,
        )
        .with_output::<outputs::RenameResult>(),
        solution_tool::<refactor_requests::MoveMemberRequest>(
            "move_member",
            "Move or copy a service or a property from one Thing, template or shape to another in the repository: the definition (and a service's implementation and sidecar) is lifted out byte for byte and re-indented where it lands. Refuses a name the target, its ancestors or its descendants already use; reports the callers that stop resolving when the target is not something the source inherits; leave_delegate keeps a service on the source that calls the moved one (a Thing target). A dry run unless dry_run is false; an apply takes the workspace lock.",
            false,
            refactor::move_member_tool,
        )
        .with_output::<outputs::MoveMemberResult>(),
        solution_tool::<refactor_requests::NewBuildingBlockRequest>(
            "new_building_block",
            "Create a new building block in the repository, as the building-block framework's Create New Building Block does on a server: its project, EntryPoint template and Thing, Management shape, Manager template and Thing (not for an abstract block), default and admin groups and organization, as files, plus the project in twaco.toml. The files match what the framework produced on a server. The permission helper and the ui and test types are not created. A dry run unless dry_run is false; an apply takes the workspace lock.",
            false,
            refactor::new_building_block_tool,
        )
        .with_output::<outputs::NewBuildingBlockResult>(),
        solution_tool::<refactor_requests::EntityNewRequest>(
            "entity_new",
            "Create a ThingWorx entity as a repository XML file: Projects, Groups, common Things, data Things, media, styles, states, menus, themes, and Networks. A new Project also appends its project declaration to twaco.toml. DataShape and DataTable creation also writes their managed sidecar. A dry run unless dry_run is false; apply takes the workspace lock and writes every file or none.",
            false,
            refactor::entity_new_tool,
        )
        .with_output::<outputs::EntityNewResult>(),
        solution_tool::<network_requests::NetworkRequest>(
            "network",
            "List repository Networks, show an asset hierarchy (from the server when server is true), or diff repository and server connections. Show returns the same data as network show --json; its tree is limited to 200 rows unless detail is true. Read-only.",
            true,
            network::network_tool,
        ).with_output::<outputs::NetworkResult>(),
        solution_tool::<block_requests::BlockRequest>(
            "block",
            "List the solution's Solution Framework building blocks (each a project with a <project>.EntryPoint Thing), or show one: type, version, display name, entry point, manager, organization, groups, permission helper, dependencies. With server true, also the framework's view from the server: its version, state (Configured, Not Configured, Error) and deploy order, notes on what differs, and for show the parameters DeployComponent takes. Read-only.",
            true,
            block::block_tool,
        )
        .with_output::<outputs::BlockResult>(),
        solution_tool::<block_requests::BlockVersionRequest>(
            "block_version",
            "Raise (bump patch, minor or major) or set a building block's version: the entry point's componentVersion and the Project's packageVersion, together and nothing else. A bump is refused while the two disagree; set writes both. A dry run unless dry_run is false; an apply takes the workspace lock and writes both files or neither.",
            false,
            block::block_version_tool,
        )
        .with_output::<outputs::BlockVersionResult>(),
        solution_tool::<network_requests::NetworkWriteRequest>(
            "network_write",
            "Add, move or remove a Network connection, import an RFC 4180 asset CSV, or pull server connections into the repository. An import can create Things from repository or platform templates. A dry run unless dry_run is false; apply writes every created Thing and the Network XML in one transaction.",
            false,
            network::network_write_tool,
        ).with_output::<outputs::NetworkWriteResult>(),
        solution_tool::<refactor_requests::EntityEditRequest>(
            "entity_edit",
            "Change an existing entity in the repository, as Composer would: add, change or remove a property of a Thing, ThingTemplate or ThingShape (a persistent one also writes the stored value of every Thing that has it), add a script service (a definition and its sidecars, as sync would) or remove one with its sidecars and run-time permissions, add or remove a script subscription, implement or stop implementing a ThingShape, change a timer's or scheduler's settings, given keys of a style, a MediaEntity's image, a menu's items, or a StateDefinition's states. Removing or retyping a property, or removing a service, something still uses is refused unless force. A dry run unless dry_run is false; apply takes the workspace lock and writes every file or none.",
            false,
            refactor::entity_edit_tool,
        )
        .with_output::<outputs::EntityEditResult>(),
        solution_tool::<refactor_requests::MashupNewRequest>(
            "mashup_new",
            "Create a ThingWorx 10.2 responsive mashup from an embedded layout template, with its entity file and managed content.json and custom.css sidecars. template is a name or alias (blank by default); mashup_templates lists the slots where mashup_add can place widgets. Menu templates require primary_menu and may use secondary_menu. master puts its page container in the template's master slot. A dry run unless dry_run is false; an apply takes the workspace lock and writes every file or none.",
            false,
            refactor::mashup_new_tool,
        )
        .with_output::<outputs::MashupNewResult>(),
        bare_tool::<refactor_requests::MashupTemplatesRequest>(
            "mashup_templates",
            "List embedded responsive mashup templates, or with name one template's slot tree. Read-only; no server.",
            true,
            refactor::mashup_templates_tool,
        )
        .with_output::<outputs::MashupTemplatesResult>(),
        solution_tool::<refactor_requests::MashupAddRequest>(
            "mashup_add",
            "Add a catalogue widget, or one an [[extension]] project of the solution defines, to an existing responsive mashup. With no placement it goes in center. A widget beside another goes in the same row or column when that container runs that way; otherwise both get containers of their own in a new row or column. A widget that can fill fills only a container it is alone in. A dry run unless dry_run is false; an apply takes the workspace lock and writes the entity and managed content sidecar together.",
            false,
            refactor::mashup_add_tool,
        )
        .with_output::<outputs::MashupAddResult>(),
        solution_tool::<refactor_requests::MashupAddContainedRequest>(
            "mashup_add_contained",
            "Add a repository mashup as a contained mashup. Its visible child parameters become container properties; set supplies static values for visible input parameters. With no placement it goes in center. A dry run unless dry_run is false; an apply takes the workspace lock and writes the entity and managed content sidecar together.",
            false,
            refactor::mashup_add_contained_tool,
        )
        .with_output::<outputs::MashupAddResult>(),
        solution_tool::<refactor_requests::MashupBindRequest>(
            "mashup_bind",
            "Add one data binding to an existing mashup. Addresses are exactly those mashup describe prints: ui:<widget>.<Property>, mashup:<Parameter>, and data:<Entity>.<Service> with an optional [as alias], [SelectedRows], field or parameter. A missing Thing call is created; templates and shapes need an existing call. A dry run unless dry_run is false; an apply takes the workspace lock and writes the entity and managed content sidecar together.",
            false,
            refactor::mashup_bind_tool,
        )
        .with_output::<outputs::MashupBindResult>(),
        solution_tool::<refactor_requests::MashupOnRequest>(
            "mashup_on",
            "Run a data call or widget service when one mashup event fires. Triggers are ui:<widget>.<Event>, mashup:Loaded, or an existing data:<Entity>.<Service>[as alias].ServiceInvokeCompleted, AllDataChanged or SelectedRowsChanged; handlers are data calls or widget services. A missing Thing handler call is created; templates and shapes need an existing call. A dry run unless dry_run is false; an apply takes the workspace lock and writes the entity and managed content sidecar together.",
            false,
            refactor::mashup_on_tool,
        )
        .with_output::<outputs::MashupOnResult>(),
        solution_tool::<refactor_requests::MashupRemoveRequest>(
            "mashup_remove",
            "Remove a widget and its subtree from an existing mashup. By default it refuses bindings, events, data-call configuration, or extension JSON that names the subtree. cascade removes only proven bindings and events, reports orphaned data calls, and never guesses which calls to delete. A dry run unless dry_run is false; apply writes the entity and managed content sidecar together.",
            false,
            refactor::mashup_remove_tool,
        )
        .with_output::<outputs::MashupRemoveResult>(),
        solution_tool::<refactor_requests::MashupUnbindRequest>(
            "mashup_unbind",
            "Remove exactly one existing data binding selected by its source and target addresses. A dry run unless dry_run is false; apply writes the entity and managed content sidecar together.",
            false,
            refactor::mashup_unbind_tool,
        )
        .with_output::<outputs::MashupUnbindResult>(),
        solution_tool::<refactor_requests::MashupOffRequest>(
            "mashup_off",
            "Remove exactly one existing event handler selected by its event and invoked address. A dry run unless dry_run is false; apply writes the entity and managed content sidecar together.",
            false,
            refactor::mashup_off_tool,
        )
        .with_output::<outputs::MashupOffResult>(),
        bare_tool::<refactor_requests::WidgetsRequest>(
            "widgets",
            "List or search the embedded ThingWorx 10.2 widget catalogue (type, Composer label, aliases, container, whether bindable), or with widget, one widget's bindable properties with base type and direction (in, out, in/out), its events and services. Read-only; no server.",
            true,
            refactor::widgets_tool,
        )
        .with_output::<outputs::WidgetsResult>(),
        solution_tool::<refactor_requests::MashupParameterRequest>(
            "mashup_param",
            "Add, change or remove a mashup parameter. It keeps the entity definition, content parameter definitions, parameter property maps and every containing mashupcontainer cache in step. A dry run unless dry_run is false; apply writes all affected mashups in one transaction.",
            false,
            refactor::mashup_parameter_tool,
        )
        .with_output::<outputs::MashupParameterResult>(),
        solution_tool::<refactor_requests::MashupRecipeRequest>(
            "mashup_recipe",
            "Write one recipe: the wiring real mashups repeat, in one edit. enable-when/show-when (widget, condition, inputs name=address: an expression into Disabled/Visible), text (widget, expression, inputs: a label's computed text), message (after a data call: say or from, kind info|warning|error), confirm (widget, then, title, say), popup (widget or on, target mashup, window modal-popup|popup|replace|new, size WxH, title, pass name=address, reload handler on close), close-after (after a data call the popup closes itself), auto-refresh (then data call, every seconds or interval address), validate (widget or on, condition, inputs, then, else_say), chart (widget, data, x, value for a pie, kind), columns (widget, columns field[:Title[:width]], data), picker (widget, data, x display field, value, selected). A dry run unless dry_run is false; an apply takes the workspace lock and writes the entity and managed content sidecar together.",
            false,
            refactor::mashup_recipe_tool,
        )
        .with_output::<outputs::MashupRecipeResult>(),
        solution_tool::<refactor_requests::MashupEventRequest>(
            "mashup_event",
            "Give a mashup an event its parents can react to. ThingWorx has no contained-mashup events, so, as real mashups do, it adds a NUMBER parameter (BOTH; every parent's copy refreshed) set to the time by an expression the given trigger evaluates. A parent reacts with mashup_on and event ui:<container>.<name>, which wires an events router. A dry run unless dry_run is false; apply writes all affected mashups in one transaction.",
            false,
            refactor::mashup_event_tool,
        )
        .with_output::<outputs::MashupParameterResult>(),
        solution_tool::<refactor_requests::MashupValidateRequest>(
            "mashup_validate",
            "Validate mashups in the repository without the server: structure, containers that hold both containers and widgets, an id_index below the widgets' numbers, bindings and events naming a widget, data source or call the mashup lacks, bindings against a catalogue widget's contract (direction, unknown property), events naming an event or service the widget lacks, bindings with several property maps, and contained-mashup parameter drift. Give mashup, or all. Read-only.",
            true,
            refactor::mashup_validate_tool,
        )
        .with_output::<outputs::MashupValidateResult>(),
        solution_tool::<refactor_requests::MashupDescribeRequest>(
            "mashup_describe",
            "Describe a mashup in the repository without its XML: the widget tree (ids, types, row/column containers, which widget fills its container, text and [[tokens]]); each service it calls, with what sets each declared parameter (a binding's source, static, or unbound); each binding, one row per property map, warning when a binding has more than one (the runtime applies only the first); and each event. Addresses are those mashup bind and on take: ui:<widget>.<Property>, data:<Entity>.<Service>[.<Parameter>], mashup:<Parameter>. Read-only.",
            true,
            refactor::mashup_describe_tool,
        )
        .with_output::<outputs::MashupDescribeResult>(),
        solution_tool::<refactor_requests::ReprojectRequest>(
            "reproject",
            "Move entities to another project of the solution: their projectName, and their files under that project's root (sidecars stay). The plan names each reference the move puts outside the deploy order, with the depends_on that allows it. A dry run unless dry_run is false; an apply takes the workspace lock and writes every file or none.",
            false,
            refactor::reproject_tool,
        )
        .with_output::<outputs::ReprojectResult>(),
        solution_tool::<refactor_requests::RetemplateRequest>(
            "retemplate",
            "Change a Thing's template (or a template's base template) and/or the shapes it implements, in the repository. The plan lists what the entity and everything inheriting it gains and loses, the stored property values and configuration-table rows left with no definition, and the references to a lost member; such a loss is refused unless accept_loss is true. A dry run unless dry_run is false; an apply takes the workspace lock.",
            false,
            refactor::retemplate_tool,
        )
        .with_output::<outputs::RetemplateResult>(),
        solution_tool::<data_requests::ConfigTableRequest>(
            "config_table",
            "List a Thing's configuration tables, read one on the server, diff it against the entity XML in the repository, compare it with a backup, back it up to a new file in the solution (a backup is never overwritten), or restore it from such a backup. restore is a dry run unless dry_run is false; it refuses a backup of another Thing or table, and reads the table back.",
            false,
            data::config_table_tool,
        )
        .with_output::<outputs::ConfigTableResult>(),
        solution_tool::<data_requests::LogsRequest>(
            "logs",
            "Read a server log: ApplicationLog, ScriptLog, CommunicationLog, ConfigurationLog or SecurityLog. Newest first. A summary first (counts by level, top origins, repeated messages, the newest entries); detail: true lists every entry. truncated: true means the limit was reached.",
            true,
            data::logs_tool,
        )
        .with_output::<outputs::LogsResult>(),
        solution_tool::<data_requests::LogLevelRequest>(
            "log_level",
            "Read a server log's level and its subloggers' levels, or change one. ScriptLog at WARN records no logger.info or logger.debug from scripts; lower it while debugging, then put it back. A change is the whole server's and a dry run unless dry_run is false; the result says how to undo it.",
            false,
            data::log_level_tool,
        )
        .with_output::<outputs::LogLevelResult>(),
        solution_progress_tool::<content_requests::RepoRequest>(
            "repo",
            "Read the server's file repositories: list them, list a folder (recursive: true for everything below), get a text file's content, or compare the tree kept in source control (filerepository/<repo>/) with the server's (same, differs, local-only, remote-only; equal sizes are compared by SHA-256). Read-only.",
            true,
            content::repo_tool,
        )
        .with_output::<outputs::RepoResult>(),
        solution_progress_tool::<content_requests::RepoWriteRequest>(
            "repo_write",
            "Change a file repository: put (upload text, or a file of the solution), mkdir, rm (a file, or a folder; recursive: true to delete one that holds anything), mv (a file), or push/pull the tree kept in source control. A dry run unless dry_run is false. Nothing existing is replaced without overwrite: true; applied changes are read back.",
            false,
            content::repo_write_tool,
        )
        .with_output::<outputs::RepoWriteResult>(),
        solution_progress_tool::<content_requests::LocalizationRequest>(
            "localization",
            "Compare the solution's localization table files with the server: same, differs, local-only and server-only tokens, plus problems that would block a push. Read-only; detail: true lists tokens and problems.",
            true,
            content::localization_tool,
        )
        .with_output::<outputs::LocalizationResult>(),
        solution_progress_tool::<content_requests::LocalizationWriteRequest>(
            "localization_write",
            "Pull localization tables into the solution, push local tables (and read them back), or create, set or remove local tokens. A dry run unless dry_run is false. push imports tokens without deleting any; prune: true deletes server tokens no local file has.",
            false,
            content::localization_write_tool,
        )
        .with_output::<outputs::LocalizationWriteResult>(),
        solution_tool::<content_requests::UsersRequest>(
            "users",
            "The test users twaco.toml declares ([[test.user]]) on the server: whether each exists, signs in with the profile's test_password, and is in its groups. Read-only.",
            true,
            content::users_tool,
        )
        .with_output::<outputs::UsersResult>(),
        solution_tool::<content_requests::UsersWriteRequest>(
            "users_write",
            "Create the missing test users, the [test] project and their group memberships (seed), or delete the test users (delete). The profile must say allow_test_users = true; seed also needs its test_password, which is never returned. A dry run unless dry_run is false.",
            false,
            content::users_write_tool,
        )
        .with_output::<outputs::UsersWriteResult>(),
        solution_tool::<content_requests::AppKeyRequest>(
            "appkey",
            "Create an application key that acts as a user (create; a dry run unless dry_run is false), or read an existing one (show). It always gets an expiration (default 30d; the server's own default is a day). The key itself is returned only with show: true, and written as a profile's app_key only with save. Delete one with entity_delete ApplicationKeys/<name>.",
            false,
            content::appkey_tool,
        )
        .with_output::<outputs::AppKeyResult>(),
        solution_tool::<content_requests::TestRequest>(
            "test",
            "The service cases of tests/*.toml, each called as each of its test users and expecting a status (and optionally rows). A dry run lists and checks the cases; dry_run false calls them, which may change the server, and needs allow_test_users = true in the profile.",
            false,
            content::test_tool,
        )
        .with_output::<outputs::TestResult>(),
        solution_tool::<content_requests::ExtensionsRequest>(
            "extensions",
            "The server's extension packages: list them, or show one (its extensions, and which are in use). Read-only.",
            true,
            content::extensions_tool,
        )
        .with_output::<outputs::ExtensionsResult>(),
        root_tool::<content_requests::ExtCheckRequest>(
            "ext_check",
            "Check a Java ThingWorx extension project offline, before it is built or uploaded: metadata.xml against the Java source (classes it names, ThingTemplates' thingPackage), the jars JarResources lists, each Widget's ide and runtime files, packageVersion against extension.properties, duplicate services, properties or events (inherited ones included), and base types. Notes say what may be fine but cannot be proven (a class only in a jar, a DataShape from elsewhere, a class nothing declares). Needs no solution or server; root is the project folder.",
            true,
            content::ext_check_tool,
        )
        .with_output::<outputs::ExtCheckResult>(),
        root_tool::<content_requests::ExtBuildRequest>(
            "ext_build",
            "Build a ThingWorx extension project's package zip and read it back: a package.json widget project runs its build script (installing its dependencies the first time), a Java project its Gradle wrapper, Gradle or Ant (Gradle with --no-daemon), and a project with neither a build nor Java sources is packed by twaco (metadata.xml, ui/, Entities/, Localization/, lib/*.jar under lib/common/). Returns the zip, its package and version, the widgets and entities its metadata.xml declares, and files it names that the zip lacks. ext_check runs first. The project must be inside the solution.",
            false,
            content::ext_build_tool,
        )
        .with_output::<outputs::ExtBuildResult>(),
        root_tool::<content_requests::ExtNewRequest>(
            "ext_new",
            "Lay out a new ThingWorx extension project. Java (default), as the Eclipse plugin's project wizard did: settings.gradle, a build.gradle that makes the importable zip (build/distributions/<name>.zip), extension.properties, configfiles/metadata.xml, .gitignore, the Java package folder, given an Extension SDK (sdk, or TWACO_SDK) its jars in twx-lib/, and when applied the Gradle wrapper (with Gradle installed). widget: a package.json widget project as the ThingWorx widget template lays it out: TypeScript IDE and runtime widgets on typescriptwebpacksupport and an esbuild script that writes zip/<name>-prod-v<version>.zip. The folder must be inside the solution, and missing or empty. A dry run unless dry_run is false; applied, ext_check's report is returned.",
            false,
            content::ext_new_tool,
        )
        .with_output::<outputs::ExtScaffoldResult>(),
        root_tool::<content_requests::ExtAddRequest>(
            "ext_add",
            "Add to a Java ThingWorx extension project: a thing-template (Java class extending Thing with its base template, plus its ThingPackage), a thing-shape or resource (Java class), a widget (ui/<name>/ ide and runtime JS and CSS) or a jar (copied into lib/), each with its metadata.xml entry; the first Java class also lists the jar the build makes. metadata.xml keeps its formatting. The project and a jar must be inside the solution. A dry run unless dry_run is false; applied, ext_check's report is returned.",
            false,
            content::ext_add_tool,
        )
        .with_output::<outputs::ExtScaffoldResult>(),
        solution_progress_tool::<content_requests::ExtDeployRequest>(
            "ext_deploy",
            "Deploy one of the solution's Java extensions (an [[extension]] in twaco.toml): ext_check it, run its Gradle wrapper, Gradle or Ant build, find the package zip under build/ or zip/, and have the server validate it; with dry_run false install it and confirm the server lists every entity its metadata.xml declares. The server skips a version it has, so a change needs bump (patch, minor or major), which writes the raised packageVersion into metadata.xml and extension.properties and puts it back if the build or install fails. An upgrade of a package with Java code is queued by the server for its next restart, and is reported as queued.",
            false,
            content::ext_deploy_tool,
        )
        .with_output::<outputs::ExtDeployResult>(),
        solution_progress_tool::<content_requests::ExtensionWriteRequest>(
            "extension_write",
            "Import an extension package zip of the solution, or remove an installed package. A dry run unless dry_run is false: an import is then only validated by the server, which installs nothing. A removal is refused while the package is in use.",
            false,
            content::extension_write_tool,
        )
        .with_output::<outputs::ExtensionWriteResult>(),
        solution_tool::<content_requests::BundleRequest>(
            "bundle",
            "The solution's configured bundle, the one importable XML that [bundle] in twaco.toml names: whether it is current, out of date or missing (dry run, as `twaco bundle --check`), or with dry_run false rebuilt from the repository. notes name references to entities the bundle leaves out. package builds a release bundle into a file you name instead.",
            false,
            content::bundle_tool,
        )
        .with_output::<outputs::BundleResult>(),
        solution_tool::<content_requests::SearchRequest>(
            "search",
            "Search the server's entities, as Composer's Spotlight box does: by text found in a name or description (any case; a * makes it a pattern), entity types and project, and with modified_since only what changed since then, newest first: what someone changed on the server since the last deploy. Read-only. Each result is Collection/Name with type, project, description and when it last changed, ready for entity_get or export. more says the limit cut the list; note says when a project does not exist. An unknown type is refused, because the server would otherwise search everything.",
            true,
            content::search_tool,
        )
        .with_output::<outputs::SearchResult>(),
        solution_tool::<content_requests::ModelResolveRequest>(
            "model_resolve",
            "How many Things implement a ThingTemplate or ThingShape on the server, and the first of them (limit, default 50; truncated says more exist). Read-only. Answers what the repository cannot: instances made at run time or by other solutions.",
            true,
            content::model_resolve_tool,
        )
        .with_output::<outputs::ModelResolveResult>(),
        solution_tool::<content_requests::EntityInspectRequest>(
            "entity_inspect",
            "An entity's properties, services and events as the server has them, each with what declares it: the entity itself, a template, a shape or a platform package (sources counts them). With values, a Thing's current property values; a property the profile may not read is listed under unreadable. member narrows to one name. Read-only.",
            true,
            content::entity_inspect_tool,
        )
        .with_output::<outputs::EntityInspectResult>(),
        solution_tool::<content_requests::AccessExplainRequest>(
            "access_explain",
            "Whether a user may invoke a service of an entity: verdict allowed, denied or unknown, from the server's own run-time check (CheckPermissionForUser) and visibility through the user's organizations and groups (Administrators and the owner see it; a deny for one of the user's organizations wins), with reasons. A grant to an organizational unit, or visibility taken from the template, is not resolved, so it is unknown rather than allowed. Read-only: the service is never invoked.",
            true,
            content::access_explain_tool,
        )
        .with_output::<outputs::AccessExplainResult>(),
        solution_tool::<content_requests::EntityGetRequest>(
            "entity_get",
            "One entity's XML as the server has it: Collection/Name reaches any entity on the server, a bare name one in the repository. Read-only; nothing is written. A long entity is cut at max_chars (truncated says so); export with action entity writes it whole to a file.",
            true,
            content::entity_get_tool,
        )
        .with_output::<outputs::EntityGetResult>(),
        solution_progress_tool::<content_requests::ExportRequest>(
            "export",
            "Export from the server as Composer's Import/Export does: an entity (Collection/Name), a collection (optionally one project's part), or a whole project, as one XML file written inside the solution; or the source-control layout of a project, collection or tags into a file repository folder or zip (a dry run unless dry_run is false).",
            false,
            content::export_tool,
        )
        .with_output::<outputs::ExportResult>(),
        solution_tool::<content_requests::PackageRequest>(
            "package",
            "Package the repository for release, offline, into a file inside the solution: a bundle (one importable XML; part all, backend or frontend), a source-control zip (<Project>/<Collection>/<Name>.xml), or an extension zip (one project's, or the solution's as a zip of its projects' zips; editable or not; metadata from [package] in twaco.toml).",
            false,
            content::package_tool,
        )
        .with_output::<outputs::PackageResult>(),
        solution_progress_tool::<content_requests::ImportRequest>(
            "import",
            "Import into the server as Composer's Import/Export does: an export file of the solution (XML, or a zip of them), or a source-control tree in a file repository. A dry run unless dry_run is false: a file's plan lists what it adds and what it replaces; an XML file also preserves configured server-owned parts. Source-control imports and archives cannot preserve them. The server's property values and configuration table rows are kept unless the overwrite flags say otherwise.",
            false,
            content::import_tool,
        )
        .with_output::<outputs::ImportResult>(),
        solution_tool::<info_requests::SettingsRequest>(
            "settings",
            "Read the server's subsystem settings (Composer: Browse > Subsystems): list the subsystems, show one with every setting, its value and description, or search every setting by name or description. Read-only; PASSWORD values are never shown.",
            true,
            info::settings_tool,
        )
        .with_output::<outputs::SettingsResult>(),
        solution_tool::<info_requests::CatalogRequest>(
            "catalog",
            "The repository's offline service catalog: callable services by entity, their parameters, result, description, origin and whether code exists. entity may be Entity.Service (or a service name one entity declares) for one service; body: true adds its script. Summary returns at most 50 services; detail returns all.",
            true,
            info::catalog_tool,
        )
        .with_output::<outputs::CatalogResult>(),
        solution_tool::<info_requests::ContractDiffRequest>(
            "contract_diff",
            "Compare the working tree's public contract with a git revision (against, required). Offline and read-only; the revision must contain its own twaco.toml. Reports breaking and compatible changes, unreadable inputs, and current callers at resolved or structural confidence. Callers outside this repository are invisible. project limits both sides; detail expands the text report. The JSON arrays always contain every change and caller.",
            true,
            info::contract_diff_tool,
        )
        .with_output::<outputs::ContractDiffResult>(),
        solution_tool::<info_requests::ImpactRequest>(
            "impact",
            "What changing an entity, or one service, property or field of it, would reach: the Things, templates, shapes, mashups and projects that depend on it, directly or through others, each at the strength of its weakest reference (structural: declared in the XML or twaco.toml; resolved: a static name in a script or a mashup binding; review: a string that looks like the name, for a person to judge). Offline and read-only. It cannot see references built at run time or outside the repository; `complete` says whether every input was read and `unreadable` lists what was not. With server: true it also asks the server (read-only) what refers to the entity one step away, in its dependency graph and, for a template or shape, the Things that have it, each marked when the repository does not have it; the server sees structural references only. Summary leaves the chain to each dependent out; detail includes it.",
            true,
            info::impact_tool,
        )
        .with_output::<outputs::ImpactResult>(),
        solution_tool::<info_requests::TraceRequest>(
            "trace",
            "How each caller uses one service, where impact says only who: for each mashup, what runs the call, what sets each parameter (or that it is unbound, with the default it then gets) and where each result field goes; for each script, the call as written, the variable the result goes in and what the script reads of it (Things[\"X\"].Service() from anywhere, me.Service() from the entity and what inherits it); and what the service itself calls. Offline and read-only; names built at run time are not seen.",
            true,
            info::trace_tool,
        )
        .with_output::<outputs::TraceResult>(),
        solution_tool::<info_requests::UnusedRequest>(
            "unused",
            "Entities nothing reaches: Things, templates, shapes and DataShapes with no chain of references from an entry point (what twaco.toml deploys, every mashup, anything that runs on events, and what `[unused] keep` names). Advisory and read-only: it deletes nothing, and an entity used only from outside the repository (a REST client, a connected system) looks unused, so list those under `[unused] keep`. `complete` says whether every input was read; `keep_unmatched` lists keep patterns that match nothing. With `fields`, instead the fields of service results that mashups bind field by field which no mashup binds or names; a result bound whole is not judged, and a field read by a script, another service or a REST client looks unused.",
            true,
            info::unused_tool,
        )
        .with_output::<outputs::UnusedResult>(),
        solution_tool::<info_requests::DocsRequest>(
            "docs",
            "The solution written down from the repository: the projects and their deploy order, how Things, templates and shapes inherit, every service with its signature, the DataShapes with their fields and where they are used, and the references that only look like a name and need a person's judgement. Offline and read-only; it has no dates, so regenerating it and diffing shows what changed. Permissions are not read yet, and references built at run time are not seen; `complete` says whether every input was read. Returns the document as JSON in `document` and as Markdown in `markdown`. Summary gives service and field counts; detail gives every signature and field.",
            true,
            info::docs_tool,
        )
        .with_output::<outputs::DocsResult>(),
        root_tool::<info_requests::DoctorRequest>(
            "doctor",
            "What resolved, what is reachable and what is missing, before anything else is blamed: twaco's version, the solution and its projects, the entity files, sidecars, .gitignore and secrets git tracks, the baseline, the workspace lock, the profile, and whether the server answers and accepts the credentials. Read-only; works without a solution. ok is false when an item fails.",
            true,
            info::doctor_tool,
        )
        .with_output::<outputs::DoctorResult>(),
        root_tool::<info_requests::GuideRequest>(
            "guide",
            "Knowledge for working on this solution: twaco's workflow, the ThingWorx platform's verified-live quirks, the service-code reference, and the solution's own AGENTS.md, CLAUDE.md and docs/. Search before a live import, a hand-written mashup binding, a configuration-table change, or a service that introspects metadata or touches JSON. list: the topics; search: the best-matching sections; read: a topic (a long one gives its outline) or one section by heading.",
            true,
            info::guide_tool,
        )
        .with_output::<outputs::GuideResult>(),
        root_tool::<info_requests::HelpSearchRequest>(
            "help_search",
            "Search the ThingWorx Platform help center for the server's version (or another): pages holding every word, best first, with title, path, summary and address. It explains concepts and how-tos; for one service's parameters, the editor types (`types`) are better.",
            true,
            info::help_search_tool,
        )
        .with_output::<outputs::HelpSearchResult>(),
        root_tool::<info_requests::HelpPageRequest>(
            "help_page",
            "Read one ThingWorx Platform help page as Markdown, from a path or address that help_search returned. Long pages: ask for one section by heading; the result lists every heading.",
            true,
            info::help_page_tool,
        )
        .with_output::<outputs::HelpPageResult>(),
        root_tool::<info_requests::JavadocRequest>(
            "javadoc",
            "Search or read ThingWorx Platform API Javadoc as Markdown. sdk can name an Extension SDK zip, extracted folder, or Javadoc jar; otherwise TWACO_SDK, then the named profile's sdk, then the public 10.1.0 site is used. refresh is ignored for an SDK. search ranks class and member simple names exact, prefix, then contains. class gives the class description and method summaries; member gives every overload with parameters, returns and throws. service takes a service as a script or path names it (Resources[\"InfoTableFunctions\"].Sort, Things/Name.Service, me.Service, Class.member), finds the class that declares it, looking in the classes it extends, and returns every overload.",
            true,
            info::javadoc_tool,
        )
        .with_output::<outputs::JavadocResult>(),
        solution_tool::<data_requests::CallRequest>(
            "call",
            "Call a ThingWorx service. Without dry_run, a service known to only read runs (a platform read such as InfoTableFunctions or GetPropertyValues that the repository does not define, or one twaco.toml [call] read_only lists; the result says read_only) and any other is a dry run; dry_run true always plans, false always calls. A bare target is a Thing; Collection/Name reaches templates, shapes, resources and subsystems.",
            false,
            data::call_service_tool,
        )
        .with_output::<outputs::CallResult>(),
    ]
});

/// Every tool's `tools/list` entry, in the order the tools are published.
pub(crate) fn definitions(protocol: Protocol) -> Vec<Value> {
    selected_definitions(protocol, &Selection::ALL)
}

/// The definitions of the selected tools, in registry order.
pub(crate) fn selected_definitions(protocol: Protocol, selection: &Selection) -> Vec<Value> {
    TOOLS
        .iter()
        .filter(|tool| selection.includes(tool.name))
        .map(|tool| tool.definition(protocol))
        .collect()
}

/// The groups `twaco mcp --tools` chooses from, each with its tools. Every tool is in exactly one
/// group; a test holds the table to the registry. `core` is the everyday loop: read the solution,
/// gate it, compare with the server, sync and deploy.
pub(crate) const GROUPS: &[(&str, &[&str])] = &[
    (
        "core",
        &[
            "projects",
            "types",
            "check",
            "status",
            "sync",
            "extract",
            "fmt",
            "push",
            "deploy",
            "bundle",
            "catalog",
            "impact",
            "contract_diff",
            "trace",
            "unused",
            "search",
            "entity_get",
            "docs",
            "doctor",
            "guide",
        ],
    ),
    (
        "entity",
        &[
            "entity_pull",
            "entity_delete",
            "entity_restore",
            "entity_carry",
            "entity_new",
            "network",
            "network_write",
            "entity_edit",
            "new_building_block",
            "block",
            "block_version",
            "export",
            "import",
            "package",
        ],
    ),
    (
        "refactor",
        &[
            "rename",
            "move_member",
            "reproject",
            "retemplate",
            "adopt_report",
            "adopt_apply",
            "handoff",
        ],
    ),
    (
        "mashup",
        &[
            "mashup_new",
            "mashup_templates",
            "mashup_add",
            "mashup_add_contained",
            "mashup_bind",
            "mashup_on",
            "mashup_remove",
            "mashup_unbind",
            "mashup_off",
            "widgets",
            "mashup_param",
            "mashup_recipe",
            "mashup_event",
            "mashup_validate",
            "mashup_describe",
        ],
    ),
    (
        "permissions",
        &[
            "permissions",
            "requirements_init",
            "requirements_check",
            "permissions_init",
            "permissions_audit",
            "permissions_apply",
            "permissions_push",
        ],
    ),
    (
        "data",
        &[
            "db_run",
            "db_query",
            "datatable_copy",
            "db_clean",
            "config_table",
            "call",
            "test",
        ],
    ),
    (
        "server",
        &[
            "init_from_server",
            "logs",
            "log_level",
            "repo",
            "repo_write",
            "localization",
            "localization_write",
            "users",
            "users_write",
            "appkey",
            "extensions",
            "ext_check",
            "ext_build",
            "ext_new",
            "ext_add",
            "ext_deploy",
            "extension_write",
            "settings",
            "model_resolve",
            "entity_inspect",
            "access_explain",
        ],
    ),
    ("reference", &["help_search", "help_page", "javadoc"]),
];

/// Which tools a server lists and runs. A client's context holds every listed definition on
/// every turn, so a client that needs only some groups should not pay for the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection(Option<Vec<&'static str>>);

impl Selection {
    /// Every tool.
    pub const ALL: Selection = Selection(None);

    /// The tools of the named groups, comma-separated; `all` is every tool. An unknown group is
    /// refused with the list of known ones, so a typo never serves nothing.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut tools = Vec::new();
        let mut any = false;
        for word in text
            .split(',')
            .map(str::trim)
            .filter(|word| !word.is_empty())
        {
            any = true;
            if word == "all" {
                return Ok(Self::ALL);
            }
            let Some((_, members)) = GROUPS.iter().find(|(group, _)| *group == word) else {
                let known: Vec<&str> = GROUPS.iter().map(|(group, _)| *group).collect();
                return Err(format!(
                    "unknown tool group `{word}`; use all or any of {}",
                    known.join(", ")
                ));
            };
            tools.extend_from_slice(members);
        }
        if !any {
            return Err("--tools names no group; use all or a comma-separated list".to_string());
        }
        Ok(Self(Some(tools)))
    }

    fn includes(&self, name: &str) -> bool {
        self.0.as_ref().is_none_or(|tools| tools.contains(&name))
    }
}

/// The registered name equal to `name`, or `None`. The result is safe to log, because it comes
/// from the registry and not from the caller.
pub(crate) fn registered_name(name: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.name)
}

/// Run a registered tool, arguments read and checked first.
pub(crate) fn call(
    root: &Path,
    selection: &Selection,
    name: &str,
    arguments: &Value,
    progress: &dyn Progress,
) -> Option<Result<Value, ToolError>> {
    // A tool the selection leaves out is unknown to this server, as `tools/list` says.
    let tool = TOOLS
        .iter()
        .find(|tool| tool.name == name && selection.includes(name))?;
    let outcome = (tool.call)(root, arguments, progress);
    // A tool that publishes an output schema owes every successful result to it.
    #[cfg(test)]
    if let (Ok(value), Some(schema)) = (&outcome, &tool.output_schema) {
        assert!(
            jsonschema::draft202012::is_valid(schema, value),
            "{}: {value} does not fit its output schema",
            tool.name
        );
    }
    Some(outcome)
}

/// Whether the typed tool accepts these arguments, without running it.
#[cfg(test)]
pub(crate) fn accepts(name: &str, arguments: &Value) -> Option<Result<(), String>> {
    let tool = TOOLS.iter().find(|tool| tool.name == name)?;
    Some((tool.check)(&tool.input_schema, arguments))
}

#[cfg(test)]
pub(crate) fn output_definition_for_test(protocol: Protocol) -> Value {
    #[derive(schemars::JsonSchema)]
    struct Projection {
        _ok: bool,
    }
    let tool =
        tool::<super::requests::common::NoArguments>("test", "test", true, |_, _| Ok(json!({})))
            .with_output::<Projection>();
    tool.definition(protocol)
}
