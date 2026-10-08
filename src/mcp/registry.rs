use super::outputs;
use super::requests::common::{parse, schema, NoArguments};
use super::requests::{
    content as content_requests, data as data_requests, entity as entity_requests,
    info as info_requests, refactor as refactor_requests, source as source_requests,
};
use super::{content, data, entity, info, refactor, source, ToolError};
use crate::core::config::Solution;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::LazyLock;

type Call = Box<dyn Fn(&Path, &Value) -> Result<Value, ToolError> + Send + Sync>;

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
    fn definition(&self, protocol: &str) -> Value {
        let mut definition = json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": !self.read_only,
                "openWorldHint": false,
            },
        });
        if protocol >= "2025-06-18" {
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
        call: Box::new(move |root, arguments| call(root, read(&parser, arguments)?)),
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
        solution_tool::<NoArguments>(
            "projects",
            "The solution's ThingWorx projects, their roots, their deploy order and how many entity documents each holds.",
            true,
            |solution, _| super::projects(solution),
        )
        .with_output::<outputs::ProjectsResult>(),
        solution_tool::<source_requests::TypesRequest>(
            "types",
            "Generate editor declarations, type-check every service, or fetch and cache platform declarations. All actions take the workspace lock.",
            false,
            source::types_tool,
        )
        .with_output::<outputs::TypesResult>(),
        solution_tool::<source_requests::CheckRequest>(
            "check",
            "Run every gate of the solution (line endings, sidecars in sync, formatting, script traps, code order, project validation, declared hooks). With live: true, every service script is also parsed by the ThingWorx server, and an unreachable server fails the check. live defaults to the solution's [gates] live.",
            true,
            source::check_tool,
        )
        .with_output::<outputs::CheckResult>(),
        solution_tool::<entity_requests::StatusRequest>(
            "status",
            "Compare entities with the server and the recorded baseline: in-sync, local-changed, server-changed, both-changed, not-on-server, or no baseline yet. Lists every entity that needs attention.",
            false,
            entity::status_tool,
        )
        .with_output::<outputs::StatusResult>(),
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
        solution_tool::<entity_requests::PushRequest>(
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
        ),
        solution_tool::<entity_requests::EntityRestoreRequest>(
            "entity_restore",
            "List the backup sets taken before deletes and forced overwrites (no set given), or import one back: all its entities, or the named ones. A dry run unless dry_run is false; each import is confirmed on the server.",
            false,
            entity::entity_restore_tool,
        ),
        solution_tool::<entity_requests::EntityCarryRequest>(
            "entity_carry",
            "Copy run-time, design-time and visibility permissions from renamed (old) entities to the ones that replaced them, mapping principals through the rename ledger. pairs is a flat list [old, new, old, new, ...] of Collection/Name; renamed adds every pending ledger entity. A dry run unless dry_run is false; every write is read back and the ledger marked carried.",
            false,
            entity::entity_carry_tool,
        ),
        solution_tool::<entity_requests::PermissionsRequest>(
            "permissions",
            "Compare entities' run-time, design-time and visibility permissions (and the instance permissions of a ThingShape or ThingTemplate) in the repository with the server's. An import only adds: it never removes a grant the server has, and never changes the server's allow or deny for a principal it already lists. Each difference is server-only, repository-only or flipped (allow/deny differs). A permission set the entity XML does not declare is not compared. Read-only.",
            true,
            entity::permissions_tool,
        ),
        solution_tool::<entity_requests::PermissionsInitRequest>(
            "permissions_init",
            "Draft a permissions.toml for each project without one, from what the project grants today, so that permissions_apply then changes nothing but what each draft's notes name: each rule names its roles outright; roles come from the permission helper's RoleGroupsAndOrganizations when the project has one, else from the groups its run-time blocks grant. from_helper takes the grants from the helper's tables instead of the entity XML (a matrix edited in the helper's mashup). An entity with a deny or a non-group principal in its run-time block, or a visibility deny of a role's unit, is left unmanaged, with a note. A dry run (the drafts' text) unless dry_run is false; existing files are never overwritten.",
            false,
            entity::permissions_init_tool,
        ),
        solution_tool::<entity_requests::PermissionsAuditRequest>(
            "permissions_audit",
            "Check each project's permissions.toml (its root folder) against the entity XML. Errors: run-time or visibility blocks the policy would change (permissions apply writes them), services of a strict entity no rule classifies, a group or user in visibility (the server answers 500), an Organization in run-time permissions. Warnings: principals under a project of the solution that no entity defines, rules or patterns that match nothing. Notes: explicit denies. With server: true (and a profile), also the server against the repository: each entity's permission sets, the permission helper's tables, the policy's platform grants and memberships, and each role's organizational unit. detail lists the grants behind each finding. Read-only.",
            true,
            entity::permissions_audit_tool,
        ),
        solution_tool::<entity_requests::PermissionsApplyRequest>(
            "permissions_apply",
            "Write each project's permissions.toml into its entity XML: the run-time block of each Thing, the instance run-time block of each ThingShape and ThingTemplate, and the role principals of each visibility block. Only blocks that differ change; the rest of each file is untouched. Refused while a strict entity has a service no rule classifies. remaining lists audit findings the write does not settle. A dry run unless dry_run is false; the files are written in one transaction. Deploy them, then permissions_push, since an import never removes a grant.",
            false,
            entity::permissions_apply_tool,
        ),
        solution_tool::<entity_requests::PermissionsPushRequest>(
            "permissions_push",
            "Make the server's permission sets exactly the repository's: every differing run-time, design-time or visibility set is written whole and read back. Removes server-only grants and corrects flipped allow/deny, which an import cannot do. A set the entity XML does not declare is never written. A dry run unless dry_run is false; an applied push records the baseline of each pushed entity that then matches the server. With platform: true (and no entities), instead add the permissions.toml [[platform]] grants and memberships the server lacks (what DeployComponent does on entities the project does not own); nothing is ever removed.",
            false,
            entity::permissions_push_tool,
        ),
        solution_tool::<data_requests::DbRunRequest>(
            "db_run",
            "Run one SQL script as an atomic SQLCommand through a throwaway Database Thing. Plans by default and shows the SQL and sanitized connection target; pass dry_run: false to execute.",
            false,
            data::db_run_tool,
        ),
        solution_tool::<data_requests::DbQueryRequest>(
            "db_query",
            "Run read-only SQLQuery through a throwaway Database Thing. Returns columns and the first 20 rows unless detail is true.",
            true,
            data::db_query_tool,
        ),
        solution_tool::<data_requests::DatatableCopyRequest>(
            "datatable_copy",
            "Copy the rows of one DataTable into the DataTable that replaced it, mapping fields by name, by the rename ledger, or by map ({old: new}). Refuses unmapped fields (unless drop_unmapped), type changes, and a non-empty target (unless append). A dry run unless dry_run is false; the target is read back and compared. Each row's source, tags and timestamp are not carried.",
            false,
            data::datatable_copy_tool,
        ),
        solution_tool::<data_requests::DbCleanRequest>(
            "db_clean",
            "Find, and with dry_run false delete, the temporary ZZ.Twaco.Sql.* Database Things an interrupted db_run or db_query left on the server. Only names twaco generates, on Database Things, are touched.",
            false,
            data::db_clean_tool,
        ),
        solution_tool::<source_requests::DeployRequest>(
            "deploy",
            "Deploy the solution: offline gates, one bundle per project in dependency order, every script parsed by the server (fails closed), a conflict check per entity, then import, read-back, and the project's deploy and post-import services. A dry run (a plan) unless dry_run is false.",
            false,
            source::deploy_tool,
        )
        .with_output::<outputs::DeployResult>(),
        solution_tool::<refactor_requests::AdoptReportRequest>(
            "adopt_report",
            "Compare a designer's Composer <Entities> export with the repository: which services it would revert, which entities it adds or changes (node by node with detail), and which it lacks. Writes nothing. reverts > 0 is what `twaco adopt --fail-on-revert` fails on: the export would undo repository work.",
            true,
            refactor::adopt_tool,
        ),
        solution_tool::<refactor_requests::AdoptApplyRequest>(
            "adopt_apply",
            "Adopt the mechanical half of a designer's export: mashup content into sidecars, a new mashup's entity file, changed media. Never writes a service or configuration table; run sync afterwards. Takes the workspace lock.",
            false,
            refactor::adopt_apply_tool,
        ),
        solution_tool::<refactor_requests::RenameRequest>(
            "rename",
            "Rename one entity, a dotted project/building-block prefix, a DataShape field, a declared service, one service parameter, or a configuration table. A dry run unless dry_run is false; an apply takes the workspace lock. The result carries a plan_digest: pass it back with dry_run false to apply exactly the plan that was reviewed.",
            false,
            refactor::rename_tool,
        ),
        solution_tool::<refactor_requests::MoveMemberRequest>(
            "move_member",
            "Move or copy a service or a property from one Thing, template or shape to another in the repository: the definition (and a service's implementation and sidecar) is lifted out byte for byte and re-indented where it lands. Refuses a name the target, its ancestors or its descendants already use; reports the callers that stop resolving when the target is not something the source inherits; leave_delegate keeps a service on the source that calls the moved one (a Thing target). A dry run unless dry_run is false; an apply takes the workspace lock.",
            false,
            refactor::move_member_tool,
        ),
        solution_tool::<refactor_requests::NewBuildingBlockRequest>(
            "new_building_block",
            "Create a new building block in the repository, as the building-block framework's Create New Building Block does on a server: its project, EntryPoint template and Thing, Management shape, Manager template and Thing (not for an abstract block), default and admin groups and organization, as files, plus the project in twaco.toml. The files match what the framework produced on a server. The permission helper and the ui and test types are not created. A dry run unless dry_run is false; an apply takes the workspace lock.",
            false,
            refactor::new_building_block_tool,
        ),
        solution_tool::<refactor_requests::RetemplateRequest>(
            "retemplate",
            "Change a Thing's template (or a template's base template) and/or the shapes it implements, in the repository. The plan lists what the entity and everything inheriting it gains and loses, the stored property values and configuration-table rows left with no definition, and the references to a lost member; such a loss is refused unless accept_loss is true. A dry run unless dry_run is false; an apply takes the workspace lock.",
            false,
            refactor::retemplate_tool,
        ),
        solution_tool::<data_requests::ConfigTableRequest>(
            "config_table",
            "Read one Thing's configuration table on the server, diff it against the entity XML in the repository, back it up to a file in the solution (refused if the file exists, unless overwrite), or restore it from such a backup. restore is a dry run unless dry_run is false; it refuses a backup of another Thing or table, and reads the table back.",
            false,
            data::config_table_tool,
        ),
        solution_tool::<data_requests::LogsRequest>(
            "logs",
            "Read a server log: ApplicationLog, ScriptLog, CommunicationLog, ConfigurationLog or SecurityLog. Newest first. A summary first (counts by level, top origins, repeated messages, the newest entries); detail: true lists every entry. truncated: true means the limit was reached.",
            true,
            data::logs_tool,
        ),
        solution_tool::<data_requests::LogLevelRequest>(
            "log_level",
            "Read a server log's level and its subloggers' levels, or change one. ScriptLog at WARN records no logger.info or logger.debug from scripts; lower it while debugging, then put it back. A change is the whole server's and a dry run unless dry_run is false; the result says how to undo it.",
            false,
            data::log_level_tool,
        ),
        solution_tool::<content_requests::RepoRequest>(
            "repo",
            "Read the server's file repositories: list them, list a folder (recursive: true for everything below), get a text file's content, or compare the tree kept in source control (filerepository/<repo>/) with the server's (same, differs, local-only, remote-only; equal sizes are compared by SHA-256). Read-only.",
            true,
            content::repo_tool,
        ),
        solution_tool::<content_requests::RepoWriteRequest>(
            "repo_write",
            "Change a file repository: put (upload text, or a file of the solution), mkdir, rm (a file, or a folder; recursive: true to delete one that holds anything), mv (a file), or push/pull the tree kept in source control. A dry run unless dry_run is false. Nothing existing is replaced without overwrite: true; applied changes are read back.",
            false,
            content::repo_write_tool,
        ),
        solution_tool::<content_requests::ExtensionsRequest>(
            "extensions",
            "The server's extension packages: list them, or show one (its extensions, and which are in use). Read-only.",
            true,
            content::extensions_tool,
        ),
        solution_tool::<content_requests::ExtensionWriteRequest>(
            "extension_write",
            "Import an extension package zip of the solution, or remove an installed package. A dry run unless dry_run is false: an import is then only validated by the server, which installs nothing. A removal is refused while the package is in use.",
            false,
            content::extension_write_tool,
        ),
        solution_tool::<content_requests::BundleRequest>(
            "bundle",
            "The solution's configured bundle, the one importable XML that [bundle] in twaco.toml names: whether it is current, out of date or missing (dry run, as `twaco bundle --check`), or with dry_run false rebuilt from the repository. notes name references to entities the bundle leaves out. package builds a release bundle into a file you name instead.",
            false,
            content::bundle_tool,
        ),
        solution_tool::<content_requests::SearchRequest>(
            "search",
            "Search the server's entities, as Composer's Spotlight box does: by text found in a name or description (any case; a * makes it a pattern), entity types and project. Read-only. Each result is Collection/Name with type, project and description, ready for entity_get or export. more says the limit cut the list; note says when a project does not exist. An unknown type is refused, because the server would otherwise search everything.",
            true,
            content::search_tool,
        ),
        solution_tool::<content_requests::EntityGetRequest>(
            "entity_get",
            "One entity's XML as the server has it: Collection/Name reaches any entity on the server, a bare name one in the repository. Read-only; nothing is written. A long entity is cut at max_chars (truncated says so); export with action entity writes it whole to a file.",
            true,
            content::entity_get_tool,
        ),
        solution_tool::<content_requests::ExportRequest>(
            "export",
            "Export from the server as Composer's Import/Export does: an entity (Collection/Name), a collection (optionally one project's part), or a whole project, as one XML file written inside the solution; or the source-control layout of a project, collection or tags into a file repository folder or zip (a dry run unless dry_run is false).",
            false,
            content::export_tool,
        ),
        solution_tool::<content_requests::PackageRequest>(
            "package",
            "Package the repository for release, offline, into a file inside the solution: a bundle (one importable XML; part all, backend or frontend), a source-control zip (<Project>/<Collection>/<Name>.xml), or an extension zip (one project's, or the solution's as a zip of its projects' zips; editable or not; metadata from [package] in twaco.toml).",
            false,
            content::package_tool,
        ),
        solution_tool::<content_requests::ImportRequest>(
            "import",
            "Import into the server as Composer's Import/Export does: an export file of the solution (XML, or a zip of them), or a source-control tree in a file repository. A dry run unless dry_run is false: a file's plan lists what it adds and what it replaces; a source-control plan is the server's own diff. The server's property values and configuration table rows are kept unless the overwrite flags say otherwise.",
            false,
            content::import_tool,
        ),
        solution_tool::<info_requests::SettingsRequest>(
            "settings",
            "Read the server's subsystem settings (Composer: Browse > Subsystems): list the subsystems, show one with every setting, its value and description, or search every setting by name or description. Read-only; PASSWORD values are never shown.",
            true,
            info::settings_tool,
        ),
        solution_tool::<info_requests::CatalogRequest>(
            "catalog",
            "The repository's offline service catalog: callable services by entity, their parameters, result, description, origin and whether code exists. Summary returns at most 50 services; detail returns all.",
            true,
            info::catalog_tool,
        ),
        solution_tool::<info_requests::ImpactRequest>(
            "impact",
            "What changing an entity, or one service, property or field of it, would reach: the Things, templates, shapes, mashups and projects that depend on it, directly or through others, each at the strength of its weakest reference (structural: declared in the XML or twaco.toml; resolved: a static name in a script or a mashup binding; review: a string that looks like the name, for a person to judge). Offline and read-only. It cannot see references built at run time or outside the repository; `complete` says whether every input was read and `unreadable` lists what was not. Summary leaves the chain to each dependent out; detail includes it.",
            true,
            info::impact_tool,
        ),
        solution_tool::<info_requests::UnusedRequest>(
            "unused",
            "Entities nothing reaches: Things, templates, shapes and DataShapes with no chain of references from an entry point (what twaco.toml deploys, every mashup, anything that runs on events, and what `[unused] keep` names). Advisory and read-only: it deletes nothing, and an entity used only from outside the repository (a REST client, a connected system) looks unused, so list those under `[unused] keep`. `complete` says whether every input was read; `keep_unmatched` lists keep patterns that match nothing.",
            true,
            info::unused_tool,
        ),
        solution_tool::<info_requests::DocsRequest>(
            "docs",
            "The solution written down from the repository: the projects and their deploy order, how Things, templates and shapes inherit, every service with its signature, the DataShapes with their fields and where they are used, and the references that only look like a name and need a person's judgement. Offline and read-only; it has no dates, so regenerating it and diffing shows what changed. Permissions are not read yet, and references built at run time are not seen; `complete` says whether every input was read. Returns the document as JSON in `document` and as Markdown in `markdown`. Summary gives service and field counts; detail gives every signature and field.",
            true,
            info::docs_tool,
        ),
        root_tool::<info_requests::DoctorRequest>(
            "doctor",
            "What resolved, what is reachable and what is missing, before anything else is blamed: twaco's version, the solution and its projects, the entity files, sidecars, .gitignore and secrets git tracks, the baseline, the workspace lock, the profile, and whether the server answers and accepts the credentials. Read-only; works without a solution. ok is false when an item fails.",
            true,
            info::doctor_tool,
        ),
        root_tool::<info_requests::GuideRequest>(
            "guide",
            "Knowledge for working on this solution: twaco's workflow, the ThingWorx platform's verified-live quirks, the service-code reference, and the solution's own AGENTS.md, CLAUDE.md and docs/. Search before a live import, a hand-written mashup binding, a configuration-table change, or a service that introspects metadata or touches JSON. list: the topics; search: the best-matching sections; read: a topic (a long one gives its outline) or one section by heading.",
            true,
            info::guide_tool,
        ),
        root_tool::<info_requests::HelpSearchRequest>(
            "help_search",
            "Search the ThingWorx Platform help center for the server's version (or another): pages holding every word, best first, with title, path, summary and address. It explains concepts and how-tos; for one service's parameters, the editor types (`types`) are better.",
            true,
            info::help_search_tool,
        ),
        root_tool::<info_requests::HelpPageRequest>(
            "help_page",
            "Read one ThingWorx Platform help page as Markdown, from a path or address that help_search returned. Long pages: ask for one section by heading; the result lists every heading.",
            true,
            info::help_page_tool,
        ),
        bare_tool::<info_requests::JavadocRequest>(
            "javadoc",
            "Search or read the ThingWorx Platform API 10.1.0 Javadoc as Markdown. search ranks class and member simple names exact, prefix, then contains. class gives the class description and method summaries; member gives every overload with parameters, returns and throws. Useful for the Java methods of objects scripts call and Resource service parameter descriptions.",
            true,
            info::javadoc_tool,
        ),
        solution_tool::<data_requests::CallRequest>(
            "call",
            "Call a ThingWorx service. A service can write, and twaco cannot tell which do, so this is a dry run unless dry_run is false. A bare target is a Thing; Collection/Name reaches templates, shapes, resources and subsystems.",
            false,
            data::call_service_tool,
        ),
    ]
});

/// Every tool's `tools/list` entry, in the order the tools are published.
pub(crate) fn definitions(protocol: &str) -> Vec<Value> {
    TOOLS.iter().map(|tool| tool.definition(protocol)).collect()
}

/// Run a registered tool, arguments read and checked first.
pub(crate) fn call(root: &Path, name: &str, arguments: &Value) -> Option<Result<Value, ToolError>> {
    let tool = TOOLS.iter().find(|tool| tool.name == name)?;
    let outcome = (tool.call)(root, arguments);
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
pub(crate) fn output_definition_for_test(protocol: &str) -> Value {
    #[derive(schemars::JsonSchema)]
    struct Projection {
        _ok: bool,
    }
    let tool =
        tool::<super::requests::common::NoArguments>("test", "test", true, |_, _| Ok(json!({})))
            .with_output::<Projection>();
    tool.definition(protocol)
}
