use super::requests;
use super::{content, data, entity, ToolError};
use crate::core::config::Solution;
use schemars::JsonSchema;
use serde_json::{json, Value};
use std::path::Path;

/// The definition and route for a typed MCP tool. Keeping these together prevents a request
/// schema from drifting away from the parser that accepts it.
pub(crate) struct Tool {
    pub(crate) name: &'static str,
    definition: fn() -> Value,
    parse: fn(&Value) -> Result<Request, ToolError>,
    route: fn(&Solution, Request) -> Result<Value, ToolError>,
    output_schema: Option<fn() -> Value>,
}

impl Tool {
    fn definition(&self, protocol: &str) -> Value {
        let mut definition = (self.definition)();
        if protocol >= "2025-06-18" {
            if let Some(output_schema) = self.output_schema {
                definition["outputSchema"] = output_schema();
            }
        }
        definition
    }

    fn call(&self, solution: &Solution, request: Request) -> Result<Value, ToolError> {
        (self.route)(solution, request)
    }
}

enum Request {
    Push(requests::entity::PushRequest),
    Repo(requests::content::RepoRequest),
    DbRun(requests::data::DbRunRequest),
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "push",
        definition: push_definition,
        parse: push_parse,
        route: push_route,
        output_schema: None,
    },
    Tool {
        name: "repo",
        definition: repo_definition,
        parse: repo_parse,
        route: repo_route,
        output_schema: None,
    },
    Tool {
        name: "db_run",
        definition: db_run_definition,
        parse: db_run_parse,
        route: db_run_route,
        output_schema: None,
    },
];

pub(crate) fn definition(name: &str, protocol: &str) -> Option<Value> {
    TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.definition(protocol))
}

pub(crate) fn call_with_solution(
    root: &Path,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, ToolError>> {
    let tool = TOOLS.iter().find(|tool| tool.name == name)?;
    Some((tool.parse)(arguments).and_then(|request| {
        Solution::discover(root)
            .map_err(ToolError::coded)
            .and_then(|solution| tool.call(&solution, request))
    }))
}

pub(crate) fn replace_definitions(definitions: &mut [Value], protocol: &str) {
    for definition in definitions {
        if let Some(name) = definition["name"].as_str() {
            if let Some(replacement) = self::definition(name, protocol) {
                *definition = replacement;
            }
        }
    }
}

fn tool_definition<T: JsonSchema>(name: &str, description: &str, read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": requests::common::schema::<T>(),
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": !read_only,
            "openWorldHint": false,
        },
    })
}

fn push_definition() -> Value {
    tool_definition::<requests::entity::PushRequest>(
        "push",
        "Import one entity's file to the server. Refuses when the server changed since the last sync, was deleted there, or has no baseline, unless force is true. Reads the entity back and records a baseline only for what the server kept. A dry run unless dry_run is false.",
        false,
    )
}

fn push_route(solution: &Solution, request: Request) -> Result<Value, ToolError> {
    let Request::Push(request) = request else {
        unreachable!("push route receives a push request")
    };
    entity::push_tool(solution, request)
}

fn push_parse(arguments: &Value) -> Result<Request, ToolError> {
    requests::common::parse::<requests::entity::PushRequest>(arguments)
        .map(Request::Push)
        .map_err(|error| ToolError::invalid(format!("{error}; nothing was done")))
}

fn repo_definition() -> Value {
    tool_definition::<requests::content::RepoRequest>(
        "repo",
        "Read the server's file repositories: list them, list a folder (recursive: true for everything below), get a text file's content, or compare the tree kept in source control (filerepository/<repo>/) with the server's (same, differs, local-only, remote-only; equal sizes are compared by SHA-256). Read-only.",
        true,
    )
}

fn repo_route(solution: &Solution, request: Request) -> Result<Value, ToolError> {
    let Request::Repo(request) = request else {
        unreachable!("repo route receives a repo request")
    };
    content::repo_tool(solution, request)
}

fn repo_parse(arguments: &Value) -> Result<Request, ToolError> {
    requests::common::parse::<requests::content::RepoRequest>(arguments)
        .map(Request::Repo)
        .map_err(|error| ToolError::invalid(format!("{error}; nothing was done")))
}

fn db_run_definition() -> Value {
    tool_definition::<requests::data::DbRunRequest>(
        "db_run",
        "Run one SQL script as an atomic SQLCommand through a throwaway Database Thing. Plans by default and shows the SQL and sanitized connection target; pass dry_run: false to execute.",
        false,
    )
}

fn db_run_route(solution: &Solution, request: Request) -> Result<Value, ToolError> {
    let Request::DbRun(request) = request else {
        unreachable!("db_run route receives a db_run request")
    };
    data::db_run_tool(solution, request)
}

fn db_run_parse(arguments: &Value) -> Result<Request, ToolError> {
    requests::common::parse::<requests::data::DbRunRequest>(arguments)
        .map(Request::DbRun)
        .map_err(|error| ToolError::invalid(format!("{error}; nothing was done")))
}

#[cfg(test)]
pub(crate) fn output_definition_for_test(protocol: &str) -> Value {
    #[derive(schemars::JsonSchema)]
    struct Projection {
        _ok: bool,
    }
    fn output_schema() -> Value {
        requests::common::schema::<Projection>()
    }
    Tool {
        name: "test",
        definition: || tool_definition::<Projection>("test", "test", true),
        parse: |_arguments| Err(ToolError::invalid("test-only tool")),
        route: |_solution, _request| Ok(json!({})),
        output_schema: Some(output_schema),
    }
    .definition(protocol)
}
