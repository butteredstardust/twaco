use super::requests::common::{parse, schema, NoArguments};
use super::requests::{
    content as content_requests, data as data_requests, entity as entity_requests,
    refactor as refactor_requests, source as source_requests,
};
use super::{content, data, entity, refactor, source, ToolError};
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

    /// Publish an output schema for a stable, owned projection of this tool's result.
    #[cfg(test)]
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

static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(|| {
    vec![
        solution_tool::<NoArguments>(
            "projects",
            "The solution's ThingWorx projects, their roots, their deploy order and how many entity documents each holds.",
            true,
            |solution, _| super::projects(solution),
        ),
        solution_tool::<entity_requests::PushRequest>(
            "push",
            "Import one entity's file to the server. Refuses when the server changed since the last sync, was deleted there, or has no baseline, unless force is true. Reads the entity back and records a baseline only for what the server kept. A dry run unless dry_run is false.",
            false,
            entity::push_tool,
        ),
        solution_tool::<content_requests::RepoRequest>(
            "repo",
            "Read the server's file repositories: list them, list a folder (recursive: true for everything below), get a text file's content, or compare the tree kept in source control (filerepository/<repo>/) with the server's (same, differs, local-only, remote-only; equal sizes are compared by SHA-256). Read-only.",
            true,
            content::repo_tool,
        ),
        solution_tool::<data_requests::DbRunRequest>(
            "db_run",
            "Run one SQL script as an atomic SQLCommand through a throwaway Database Thing. Plans by default and shows the SQL and sanitized connection target; pass dry_run: false to execute.",
            false,
            data::db_run_tool,
        ),
        solution_tool::<entity_requests::StatusRequest>(
            "status",
            "Compare entities with the server and the recorded baseline: in-sync, local-changed, server-changed, both-changed, not-on-server, or no baseline yet. Lists every entity that needs attention.",
            false,
            entity::status_tool,
        ),
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
        solution_tool::<source_requests::TypesRequest>(
            "types",
            "Generate editor declarations, type-check every service, or fetch and cache platform declarations. All actions take the workspace lock.",
            false,
            source::types_tool,
        ),
        solution_tool::<source_requests::CheckRequest>(
            "check",
            "Run every gate of the solution (line endings, sidecars in sync, formatting, script traps, code order, project validation, declared hooks). With live: true, every service script is also parsed by the ThingWorx server, and an unreachable server fails the check. live defaults to the solution's [gates] live.",
            true,
            source::check_tool,
        ),
        solution_tool::<source_requests::SyncRequest>(
            "sync",
            "Write sidecars back into their entity XML: service scripts, DataShape fields, mashup content, DataTable configuration. This is how an edit to a script.js takes effect. check: true reports what would change and writes nothing. Takes the workspace lock while it writes.",
            false,
            source::sync_tool,
        ),
        solution_tool::<source_requests::ExtractRequest>(
            "extract",
            "Entity XML to sidecars: service scripts, DataShape fields, mashup content, DataTable configuration. Overwrites the sidecars of the entities chosen; takes the workspace lock.",
            false,
            source::extract_tool,
        ),
        solution_tool::<source_requests::FmtRequest>(
            "fmt",
            "Format every service script sidecar with the built-in formatter. check: true reports which would change and writes nothing.",
            false,
            source::fmt_tool,
        ),
        solution_tool::<source_requests::DeployRequest>(
            "deploy",
            "Deploy the solution: offline gates, one bundle per project in dependency order, every script parsed by the server (fails closed), a conflict check per entity, then import, read-back, and the project's deploy and post-import services. A dry run (a plan) unless dry_run is false.",
            false,
            source::deploy_tool,
        ),
        solution_tool::<refactor_requests::AdoptReportRequest>(
            "adopt_report",
            "Compare a designer's Composer <Entities> export with the repository: which services it would revert, which entities it adds or changes (node by node with detail), and which it lacks. Writes nothing.",
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
            "Create a new building block in the repository, as the PTC Solution Framework's Create New Building Block does on a server: its project, EntryPoint template and Thing, Management shape, Manager template and Thing (not for an abstract block), default and admin groups and organization, as files, plus the project in twaco.toml. The files match what the framework produced on a server. The permission helper and the ui and test types are not created. A dry run unless dry_run is false; an apply takes the workspace lock.",
            false,
            refactor::new_building_block_tool,
        ),
        solution_tool::<refactor_requests::RetemplateRequest>(
            "retemplate",
            "Change a Thing's template (or a template's base template) and/or the shapes it implements, in the repository. The plan lists what the entity and everything inheriting it gains and loses, the stored property values and configuration-table rows left with no definition, and the references to a lost member; such a loss is refused unless accept_loss is true. A dry run unless dry_run is false; an apply takes the workspace lock.",
            false,
            refactor::retemplate_tool,
        ),
    ]
});

/// The typed tool's `tools/list` entry, if it is registered.
pub(crate) fn definition(name: &str, protocol: &str) -> Option<Value> {
    TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.definition(protocol))
}

/// Run a registered tool, arguments read and checked first.
pub(crate) fn call(root: &Path, name: &str, arguments: &Value) -> Option<Result<Value, ToolError>> {
    let tool = TOOLS.iter().find(|tool| tool.name == name)?;
    Some((tool.call)(root, arguments))
}

/// Swap the hand-written entry of every registered tool for the generated one.
pub(crate) fn replace_definitions(definitions: &mut [Value], protocol: &str) {
    for definition in definitions {
        if let Some(name) = definition["name"].as_str() {
            if let Some(replacement) = self::definition(name, protocol) {
                *definition = replacement;
            }
        }
    }
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
