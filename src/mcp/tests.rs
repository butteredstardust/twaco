use super::entity::push_outcome_json;
use super::schema::validate_arguments;
use super::source::types_tool_with_compiler;
use super::*;
use crate::core::datashape;
use crate::core::lock;

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
        serde_json::from_str(reply[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
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
    let missing = call(json!({"kind":"service","old":"Run","new":"Execute","skip_checks":true}));
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
                stdout: b".twaco/types/check/s0000.js(3,4): error TS2554: Expected 2 arguments.\n"
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
    let body: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
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
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["dry_run"], true);
    assert_eq!(text["would_call"]["service"], "Reset");
    let _ = std::fs::remove_dir_all(root);
}
