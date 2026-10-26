use super::entity::push_outcome_json;
use super::source::types_tool_with_compiler;
use super::*;
use crate::core::datashape;
use crate::core::lock;
use serde::de::DeserializeOwned;

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

fn solution_dir() -> (tempfile::TempDir, PathBuf) {
    let root_guard = tempfile::Builder::new()
        .prefix("twaco-mcp-")
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::create_dir_all(root.join("Projects")).unwrap();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    std::fs::write(
        root.join("Things/P.T.xml"),
        "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"></Thing></Things></Entities>",
    )
    .unwrap();
    std::fs::write(
        root.join("Projects/P.xml"),
        "<Entities><Projects><Project name=\"P\" projectName=\"P\"></Project></Projects></Entities>",
    )
    .unwrap();
    (root_guard, root)
}

#[test]
fn contract_diff_is_read_only_offline_and_requires_against() {
    let (_dir, root) = solution_dir();
    let responses = converse(
        &root,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"contract_diff","arguments":{}}}),
        ],
    );
    let tool = responses[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "contract_diff")
        .unwrap();
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    assert_eq!(tool["annotations"]["openWorldHint"], false);
    assert_eq!(tool["inputSchema"]["required"], json!(["against"]));
    assert!(tool["inputSchema"]["properties"].get("profile").is_none());
    assert!(responses[1].to_string().contains("against"));
    assert!(responses[1].to_string().contains("required"));
}

#[test]
fn contract_diff_returns_changes_and_text_matching_its_output_schema() {
    use std::process::Command;
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipping contract MCP integration test: git is not on PATH");
        return;
    }
    let (_dir, root) = solution_dir();
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Acme Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["add", "."],
        vec![
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "Acme solution",
        ],
    ] {
        let output = Command::new("git")
            .current_dir(&root)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::create_dir(root.join("DataShapes")).unwrap();
    std::fs::write(root.join("DataShapes/Acme.Rows.xml"), r#"<Entities><DataShapes><DataShape name="Acme.Rows" projectName="P"><FieldDefinitions/></DataShape></DataShapes></Entities>"#).unwrap();
    for detail in [false, true] {
        let responses = converse(
            &root,
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"contract_diff","arguments":{"against":"HEAD","detail":detail}}}),
            ],
        );
        let content = &responses[0]["result"]["structuredContent"];
        assert_eq!(content["compatible"][0]["entity"], "DataShapes/Acme.Rows");
        assert!(content["breaking"].as_array().unwrap().is_empty());
        assert!(
            jsonschema::draft202012::is_valid(&output_schema_of("contract_diff"), content),
            "{content}"
        );
        let text = content["text"].as_str().unwrap();
        if detail {
            assert!(text.contains("compatible: DataShapes/Acme.Rows"));
        } else {
            assert!(text.contains("compatible: 1 entity"));
        }
    }
    assert!(!root.join(".twaco/profiles").exists());
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
                    | "minItems"
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
fn init_from_server_is_a_root_tool_with_required_projects() {
    let root_guard = tempfile::tempdir().unwrap();
    let response = converse(
        root_guard.path(),
        &[json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"tools/call",
            "params":{"name":"init_from_server","arguments":{}}
        })],
    );
    let tool = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "init_from_server")
        .unwrap();
    assert_eq!(tool["annotations"]["readOnlyHint"], false);
    assert_eq!(tool["inputSchema"]["required"], json!(["projects"]));
    assert_eq!(tool["inputSchema"]["properties"]["projects"]["minItems"], 1);
    assert!(response[0].to_string().contains("projects"));
}

#[test]
fn projects_includes_misfiled_entity_documents() {
    let (_dir, root) = solution_dir();
    std::fs::write(
        root.join("Things/Wrong.xml"),
        "<Entities><Things><Thing name=\"Wrong\" projectName=\"Elsewhere\"/></Things></Entities>",
    )
    .unwrap();
    let (failed, body) = call_tool(&root, "projects", json!({}));
    assert!(!failed, "{body}");
    assert_eq!(
        body["misfiled"],
        json!([{"name": "Wrong", "project": "Elsewhere", "found_under": "P"}])
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

fn contains_forbidden_keyword(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    if object.keys().any(|key| {
        matches!(
            key.as_str(),
            "$schema" | "$defs" | "$ref" | "title" | "format"
        )
    }) {
        return true;
    }
    object
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|properties| properties.values().any(contains_forbidden_keyword))
        || object.get("items").is_some_and(contains_forbidden_keyword)
        || object
            .get("additionalProperties")
            .is_some_and(contains_forbidden_keyword)
}

#[test]
fn generated_schemas_are_valid_conservative_json_schema() {
    for tool in tool_definitions() {
        for key in ["inputSchema", "outputSchema"] {
            let Some(schema) = tool.get(key) else {
                continue;
            };
            assert_eq!(schema["type"], "object", "{} {key}", tool["name"]);
            assert!(
                jsonschema::draft202012::meta::is_valid(schema),
                "{} {key}: {:?}",
                tool["name"],
                jsonschema::draft202012::meta::validate(schema).err()
            );
            assert!(
                !contains_forbidden_keyword(schema),
                "{} {key} has a generated-only keyword",
                tool["name"]
            );
        }
    }
}

#[test]
fn the_stable_tools_publish_an_output_schema_only_to_clients_that_know_it() {
    let with = |protocol: &str| -> Vec<String> {
        registry::definitions(Protocol::parse(protocol).unwrap())
            .iter()
            .filter(|tool| tool.get("outputSchema").is_some())
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(with("2024-11-05").is_empty());
    assert!(with("2025-03-26").is_empty());
    // Every tool describes its result.
    let every: Vec<String> = tool_definitions()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    for protocol in ["2025-06-18", LATEST.as_str()] {
        assert_eq!(with(protocol), every, "{protocol}");
    }
}

/// The tool definitions as they were written by hand, before any was generated.
fn hand_written_tools() -> Vec<Value> {
    const HAND_WRITTEN: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mcp_tools_hand_written.json"
    );
    serde_json::from_str(&std::fs::read_to_string(HAND_WRITTEN).unwrap()).unwrap()
}

/// A value that fits a property, for every way the hand-written schema allows one.
fn fitting_values(property: &Value) -> Vec<Value> {
    let mut values = Vec::new();
    if let Some(default) = property.get("default") {
        values.push(default.clone());
    }
    if let Some(allowed) = property["enum"].as_array() {
        values.extend(allowed.iter().cloned());
        return values;
    }
    match property["type"].as_str() {
        Some("string") => values.extend([json!("x"), json!("")]),
        Some("boolean") => values.extend([json!(true), json!(false)]),
        Some("integer") => {
            let minimum = property["minimum"].as_u64().unwrap_or(0);
            values.extend([
                json!(minimum),
                json!(minimum + 1),
                json!(minimum.saturating_sub(1)),
            ]);
        }
        Some("array") => values.extend([json!([]), json!(["x"]), json!([1])]),
        Some("object") => values.extend([json!({}), json!({"x": "y"}), json!({"x": 1})]),
        _ => {}
    }
    values
}

fn misfitting_values() -> Vec<Value> {
    vec![
        json!(null),
        json!(1),
        json!(-1),
        json!("zzz"),
        json!(true),
        json!([]),
        json!({}),
    ]
}

/// Every argument object worth asking a tool about, made from its hand-written schema: the
/// minimum, each property fitting and misfitting, a required property missing, an unknown one.
fn contract_inputs(schema: &Value) -> Vec<Value> {
    let properties = schema["properties"].as_object().unwrap();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut base = Map::new();
    for name in &required {
        base.insert(
            (*name).to_string(),
            fitting_values(&properties[*name]).remove(0),
        );
    }
    let mut inputs = vec![
        json!({}),
        Value::Object(base.clone()),
        json!(null),
        json!([]),
        json!("x"),
    ];
    for (name, property) in properties {
        for value in fitting_values(property)
            .into_iter()
            .chain(misfitting_values())
        {
            let mut input = base.clone();
            input.insert(name.clone(), value);
            inputs.push(Value::Object(input));
        }
    }
    for name in &required {
        let mut input = base.clone();
        input.remove(*name);
        inputs.push(Value::Object(input));
    }
    let mut unknown = base;
    unknown.insert("unknown_argument".to_string(), json!(true));
    inputs.push(Value::Object(unknown));
    inputs
}

#[test]
fn typed_requests_accept_and_reject_what_the_hand_written_schemas_did() {
    let mut checked = 0;
    for tool in hand_written_tools() {
        let name = tool["name"].as_str().unwrap();
        for arguments in contract_inputs(&tool["inputSchema"]) {
            let Some(typed) = registry::accepts(name, &arguments) else {
                break;
            };
            let old = legacy_schema(&tool["inputSchema"], &arguments);
            assert_eq!(
                old.is_ok(),
                typed.is_ok(),
                "{name}: {arguments}: hand-written {old:?}, typed {typed:?}"
            );
            if let (Err(old), Err(typed)) = (&old, &typed) {
                assert_eq!(old, typed, "{name}: {arguments}");
            }
            checked += 1;
        }
    }
    assert!(checked > 100, "only {checked} inputs were compared");
}

/// What a client can read from a property of a schema, without the order of its keys.
fn meaning(schema: &Value) -> Value {
    let mut normalised = schema.clone();
    if let Some(object) = normalised.as_object_mut() {
        if object
            .get("required")
            .is_some_and(|required| required == &json!([]))
        {
            object.remove("required");
        }
        if object["type"] == "object" {
            object.entry("properties").or_insert_with(|| json!({}));
        }
        if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
            for property in properties.values_mut() {
                *property = meaning(property);
            }
        }
    }
    normalised
}

#[test]
fn generated_schemas_mean_what_the_hand_written_ones_did() {
    let hand_written = hand_written_tools();
    // The historical fixture proves migration compatibility. New tools are covered by the
    // current golden file and tools/list contract, without rewriting that historical baseline.
    let generated: Vec<_> = registry::definitions(LATEST)
        .into_iter()
        .filter(|tool| hand_written.iter().any(|old| old["name"] == tool["name"]))
        .collect();
    let names = |tools: &[Value]| {
        tools
            .iter()
            .map(|tool| tool["name"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&generated),
        names(&hand_written),
        "the same tools, in the same order"
    );
    for (generated, tool) in generated.iter().zip(&hand_written) {
        let name = tool["name"].as_str().unwrap();
        assert_eq!(
            meaning(&generated["inputSchema"]),
            meaning(&tool["inputSchema"]),
            "{name}"
        );
        for key in ["description", "annotations"] {
            assert_eq!(generated[key], tool[key], "{name}: {key}");
        }
    }
}

fn parse_typed<T: DeserializeOwned + schemars::JsonSchema>(arguments: &Value) -> Result<T, String> {
    requests::common::parse(&requests::common::schema::<T>(), arguments)
}

#[test]
fn typed_requests_keep_defaults_and_argument_errors() {
    let push = parse_typed::<requests::entity::PushRequest>(&json!({"entity":"P.T"})).unwrap();
    assert!(push.dry_run);
    assert!(!push.force);
    assert!(push.backup);
    assert_eq!(push.profile, "default");

    let repo = parse_typed::<requests::content::RepoRequest>(&json!({})).unwrap();
    assert_eq!(repo.max_chars, 100_000);
    assert!(!repo.detail);

    let db = parse_typed::<requests::data::DbRunRequest>(&json!({})).unwrap();
    assert_eq!(db.timeout, 120);
    assert!(db.dry_run);

    assert_eq!(
        parse_typed::<requests::entity::PushRequest>(&json!({"entity":null})).unwrap_err(),
        "`entity` must be a string, not null"
    );
    assert_eq!(
        parse_typed::<requests::data::DbRunRequest>(&json!({"unknown":true}))
            .unwrap_err(),
        "this tool takes no argument `unknown` (it takes: dry_run, file, no_transaction, profile, sql, thing, timeout)"
    );
}

#[test]
fn localization_tools_plan_apply_and_refuse_missing_values() {
    let (_directory, root) = solution_dir();
    let (failed, plan) = call_tool(
        &root,
        "localization_write",
        json!({"action":"set","token":"P.Title","value":"Title"}),
    );
    assert!(!failed && plan["dry_run"] == true, "{plan}");
    assert!(!root.join("localization/P/LocalizationTable.xml").exists());
    let (failed, applied) = call_tool(
        &root,
        "localization_write",
        json!({"action":"set","token":"P.Title","value":"Title","dry_run":false}),
    );
    assert!(!failed && applied["dry_run"] == false, "{applied}");
    assert!(root.join("localization/P/LocalizationTable.xml").exists());
    let (failed, refused) = call_tool(
        &root,
        "localization_write",
        json!({"action":"set","token":"P.Title"}),
    );
    assert!(
        failed
            && refused["error"]
                .as_str()
                .unwrap()
                .contains("`value` is required")
    );
    let (failed, profile) = call_tool(&root, "localization", json!({"profile":"missing"}));
    assert!(failed && profile["code"].is_string(), "{profile}");
}

#[test]
fn localization_detail_names_differing_fields_and_both_token_sides() {
    let (_directory, root) = solution_dir();
    let solution = crate::core::config::Solution::load(&root.join("twaco.toml")).unwrap();
    let token = |value: &str, context: &str| crate::core::localization::Token {
        name: "P.Title".to_string(),
        value: value.to_string(),
        usage: "label".to_string(),
        context: context.to_string(),
    };
    let status = crate::core::localization::Status {
        compared: vec![crate::core::localization::Compared {
            table: "Default".to_string(),
            name: "P.Title".to_string(),
            state: crate::core::localization::State::Differs,
            different: vec!["value", "context"],
            local: Some(token("Local", "here")),
            server: Some(token("Server", "there")),
            file: Some(root.join("localization/P/LocalizationTable.xml")),
        }],
        problems: Vec::new(),
        missing_tables: Vec::new(),
        unreadable: Vec::new(),
    };
    let item = localization_status_json(&solution, &status, true)["compared"]
        .as_array()
        .unwrap()[0]
        .clone();
    assert_eq!(item["different"], json!(["value", "context"]));
    assert_eq!(item["local"]["value"], "Local");
    assert_eq!(item["server"]["context"], "there");
}

#[test]
fn bundle_handoff_is_recorded_only_when_mcp_applies() {
    let (_directory, root) = solution_dir();
    let (failed, planned) = call_tool(
        &root,
        "bundle",
        json!({"handoff":"planned", "dry_run":true}),
    );
    assert!(!failed, "{planned}");
    let solution = crate::core::config::Solution::load(&root.join("twaco.toml")).unwrap();
    assert!(crate::core::adopt::handoffs(&solution).unwrap().is_empty());
    let (failed, written) = call_tool(
        &root,
        "bundle",
        json!({"handoff":"delivered", "dry_run":false}),
    );
    assert!(!failed, "{written}");
    assert_eq!(
        crate::core::adopt::handoffs(&solution).unwrap()[0].name,
        "delivered"
    );
}

#[test]
fn adopt_suggest_base_returns_guidance_without_a_report_when_no_handoffs_exist() {
    let (_directory, root) = solution_dir();
    std::fs::write(
        root.join("export.xml"),
        "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"></Thing></Things></Entities>",
    )
    .unwrap();
    let (failed, result) = call_tool(
        &root,
        "adopt_report",
        json!({"export":"export.xml", "suggest_base":true}),
    );
    assert!(!failed, "{result}");
    assert!(result.get("reverts").is_none());
    assert!(result["notes"][0]
        .as_str()
        .unwrap()
        .contains("no recorded handoff matches this export"));
}

#[test]
fn output_schema_waits_for_the_new_protocol() {
    let old = registry::output_definition_for_test(Protocol::V2025_03_26);
    let new = registry::output_definition_for_test(Protocol::V2025_06_18);
    assert!(old.get("outputSchema").is_none());
    assert_eq!(new["outputSchema"]["type"], "object");
    assert!(jsonschema::draft202012::meta::is_valid(
        &new["outputSchema"]
    ));
}

#[test]
fn tools_list_publishes_owned_parts_in_status_push_and_deploy_schemas() {
    let status = output_schema_of("status");
    assert!(status["properties"]["statuses"]["items"]["properties"]["server_owned"].is_object());
    let push = output_schema_of("push");
    assert_eq!(push["properties"]["server_owned"]["type"], "array");
    let deploy = output_schema_of("deploy");
    assert!(deploy["properties"]["server_owned"].is_object());
    assert_eq!(
        deploy["properties"]["server_owned"]["items"]["properties"]["parts"]["type"],
        "array"
    );
}

#[test]
fn validator_keeps_top_level_messages_stable() {
    let required = json!({ "type": "object", "properties": { "x": { "type": "string" } }, "required": ["x"], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&required, &json!({})),
        Err("`x` is required".to_string())
    );

    let names = json!({ "type": "object", "properties": { "a": { "type": "string" }, "b": { "type": "string" } }, "required": [], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&names, &json!({"x": 1})),
        Err("this tool takes no argument `x` (it takes: a, b)".to_string())
    );

    let boolean = json!({ "type": "object", "properties": { "x": { "type": "boolean" } }, "required": [], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&boolean, &json!({"x": 1})),
        Err("`x` must be a boolean, not 1".to_string())
    );

    let array = json!({ "type": "object", "properties": { "x": { "type": "array", "items": { "type": "string" } } }, "required": [], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&array, &json!({"x": "not an array"})),
        Err("`x` must be an array of strings, not \"not an array\"".to_string())
    );

    let integer = json!({ "type": "object", "properties": { "x": { "type": "integer", "minimum": 3 } }, "required": [], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&integer, &json!({"x": 1})),
        Err("`x` must be an integer of at least 3, not 1".to_string())
    );

    let choices = json!({ "type": "object", "properties": { "x": { "type": "string", "enum": ["a", "b"] } }, "required": [], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&choices, &json!({"x": "c"})),
        Err("`x` must be one of [\"a\",\"b\"], not \"c\"".to_string())
    );
    assert_eq!(
        legacy_schema(&choices, &json!([])),
        Err("`arguments` must be a JSON object".to_string())
    );
}

#[test]
fn validator_follows_nested_schemas() {
    let map = json!({ "type": "object", "properties": { "map": { "type": "object", "additionalProperties": { "type": "string" } } }, "required": [], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&map, &json!({"map": {"title": 1}})),
        Err("`map.title` must be a string, not 1".to_string())
    );

    let closed = json!({ "type": "object", "properties": { "map": { "type": "object", "properties": { "title": { "type": "string" } }, "additionalProperties": false } }, "required": [], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&closed, &json!({"map": {"other": "x"}})),
        Err("`map.other` is not allowed".to_string())
    );

    let array = json!({ "type": "object", "properties": { "only": { "type": "array", "items": { "type": "string" } } }, "required": [], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&array, &json!({"only": ["P.T", 2]})),
        Err("`only[1]` must be a string, not 2".to_string())
    );

    let objects = json!({ "type": "object", "properties": { "rows": { "type": "array", "items": { "type": "object" } } }, "required": [], "additionalProperties": false });
    assert_eq!(
        legacy_schema(&objects, &json!({"rows": 1})),
        Err("`rows` must be an array, not 1".to_string())
    );
}

#[test]
fn tool_errors_include_stable_codes() {
    let (_dir, root) = solution_dir();
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

    let unclassified = tool_result(
        Err(ToolError::with(ErrorCode::Unclassified, "free text")),
        LATEST,
    );
    let value: Value =
        serde_json::from_str(unclassified["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        value,
        json!({ "error": "free text", "code": "unclassified" })
    );
}

#[test]
fn typed_and_argument_failures_keep_their_codes() {
    let (_dir, root) = solution_dir();
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
    let (_dir, root) = solution_dir();
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
            server_owned: Vec::new(),
            entity: crate::core::entity::key::EntityKey::new("Things", "P.T").unwrap(),
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
    let (_dir, root) = solution_dir();
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
}

#[test]
fn the_handshake_negotiates_a_version_and_notifications_get_no_reply() {
    let (_dir, root) = solution_dir();
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
        responses[2]["result"]["protocolVersion"],
        LATEST.as_str(),
        "an unknown version gets ours"
    );
}

#[test]
fn errors_are_protocol_errors_or_tool_errors_as_the_spec_says() {
    let (_dir, root) = solution_dir();
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
}

#[test]
fn logs_arguments_are_refused_before_the_server_is_asked() {
    let (_dir, root) = solution_dir();
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
}

#[test]
fn protocol_revisions_round_trip_and_sort_by_date() {
    let mut previous: Option<Protocol> = None;
    for text in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
        let protocol = Protocol::parse(text).unwrap();
        assert_eq!(protocol.as_str(), text);
        assert!(
            previous.is_none_or(|previous| previous < protocol),
            "{text}"
        );
        assert_eq!(
            protocol.has_structured_output(),
            text >= "2025-06-18",
            "{text}"
        );
        previous = Some(protocol);
    }
    assert_eq!(previous, Some(LATEST));
    assert_eq!(Protocol::parse("2025-06-19"), None);
    let old = tool_result(Ok(json!({"ok": true})), Protocol::V2025_03_26);
    assert!(old.get("structuredContent").is_none());
    let new = tool_result(Ok(json!({"ok": true})), Protocol::V2025_06_18);
    assert_eq!(new["structuredContent"], json!({"ok": true}));
    let failed = tool_result(
        Err(ToolError::with(ErrorCode::Unclassified, "free text")),
        Protocol::V2025_06_18,
    );
    assert_eq!(failed["isError"], true);
    assert!(failed.get("structuredContent").is_none());
}

#[test]
fn annotations_say_what_each_tool_may_reach_and_discard() {
    let tools = tool_definitions();
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not registered"))
    };
    for (list, names) in registry::HINT_LISTS {
        for name in names {
            let hints = &tool(name)["annotations"];
            if list != "OPEN_WORLD" {
                assert_eq!(
                    hints["readOnlyHint"], false,
                    "{list} names {name}, which writes nothing"
                );
            }
        }
    }
    for tool in &tools {
        let hints = &tool["annotations"];
        let name = tool["name"].as_str().unwrap();
        // The spec defines these for writing tools only; a reader that carries them says nothing.
        if hints["readOnlyHint"] == true {
            assert!(hints.get("destructiveHint").is_none(), "{name}");
            assert!(hints.get("idempotentHint").is_none(), "{name}");
        } else {
            assert!(hints["destructiveHint"].is_boolean(), "{name}");
            assert!(hints["idempotentHint"].is_boolean(), "{name}");
        }
    }
    // A tool that takes a server profile can reach the server; bundle's only picks the profile's
    // patches from twaco.toml.
    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        if name != "bundle" && tool["inputSchema"]["properties"].get("profile").is_some() {
            assert_eq!(
                tool["annotations"]["openWorldHint"], true,
                "{name} takes a profile"
            );
        }
    }
    assert_eq!(tool("fmt")["annotations"]["destructiveHint"], false);
    assert_eq!(
        tool("sync")["annotations"]["destructiveHint"],
        true,
        "sync rewrites entity XML"
    );
    assert_eq!(tool("deploy")["annotations"]["idempotentHint"], false);
    assert_eq!(tool("guide")["annotations"]["openWorldHint"], false);
}

#[test]
fn every_tool_is_listed_with_a_schema_and_honest_annotations() {
    let (_dir, root) = solution_dir();
    let responses = converse(
        &root,
        &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let tools = responses[0]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "init_from_server",
            "projects",
            "types",
            "check",
            "status",
            "entity_pull",
            "sync",
            "extract",
            "fmt",
            "push",
            "entity_delete",
            "entity_restore",
            "entity_carry",
            "permissions",
            "requirements_init",
            "requirements_check",
            "permissions_init",
            "permissions_audit",
            "permissions_apply",
            "permissions_push",
            "db_run",
            "db_query",
            "datatable_copy",
            "db_clean",
            "deploy",
            "adopt_report",
            "adopt_apply",
            "handoff",
            "rename",
            "move_member",
            "new_building_block",
            "entity_new",
            "network",
            "network_write",
            "entity_edit",
            "mashup_new",
            "mashup_templates",
            "mashup_add",
            "mashup_add_contained",
            "mashup_bind",
            "mashup_on",
            "widgets",
            "mashup_param",
            "mashup_recipe",
            "mashup_event",
            "mashup_validate",
            "mashup_describe",
            "reproject",
            "retemplate",
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
            "ext_check",
            "extension_write",
            "bundle",
            "search",
            "entity_get",
            "export",
            "package",
            "import",
            "settings",
            "catalog",
            "contract_diff",
            "impact",
            "trace",
            "unused",
            "docs",
            "doctor",
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
        "permissions_init",
        "requirements_init",
        "permissions_apply",
        "permissions_push",
        "db_run",
        "db_clean",
        "datatable_copy",
        "deploy",
        "rename",
        "move_member",
        "new_building_block",
        "entity_new",
        "network_write",
        "entity_edit",
        "mashup_new",
        "mashup_add",
        "mashup_add_contained",
        "mashup_bind",
        "mashup_on",
        "mashup_param",
        "mashup_recipe",
        "mashup_event",
        "reproject",
        "retemplate",
        "config_table",
        "log_level",
        "repo_write",
        "localization_write",
        "users_write",
        "test",
        "extension_write",
        "export",
        "import",
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
    // Left out, only a call known to only read runs: the schema has no default to promise.
    let dry_run = &call["inputSchema"]["properties"]["dry_run"];
    assert!(dry_run.get("default").is_none(), "{dry_run}");
    assert!(
        dry_run["description"]
            .as_str()
            .unwrap()
            .contains("otherwise planned"),
        "{dry_run}"
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
}

#[test]
fn rename_plans_then_applies_and_refuses_bad_requests() {
    let (_dir, root) = solution_dir();
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
}

#[test]
fn field_rename_requires_scope_and_is_a_dry_run_by_default() {
    let (_dir, root) = solution_dir();
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
}

#[test]
fn service_rename_requires_scope_and_is_a_dry_run_by_default() {
    let (_dir, root) = solution_dir();
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
}

#[test]
fn table_rename_requires_scope_and_is_a_dry_run_by_default() {
    let (_dir, root) = solution_dir();
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
}

#[test]
fn projects_and_check_answer_offline_with_structured_content() {
    let (_dir, root) = solution_dir();
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
    assert_eq!(projects["projects"][0]["entities"], 2);
    let check = &responses[2]["result"]["structuredContent"];
    assert!(check["gates"].as_array().unwrap().len() >= 6);
    assert!(
        check.get("failures").is_none(),
        "findings are detail, not summary"
    );
}

#[test]
fn handoff_plans_records_and_supplies_an_adopt_base() {
    let (_dir, root) = solution_dir();
    let export = root.join("base.xml");
    std::fs::write(
        &export,
        "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"></Thing></Things></Entities>",
    )
    .unwrap();
    let call = |id, name, arguments| {
        converse(
            &root,
            &[json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})],
        )
        .remove(0)
    };
    let body = |reply: &Value| -> Value {
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    let plan = call(
        1,
        "handoff",
        json!({"action":"record","name":"h1","files":["base.xml"]}),
    );
    assert!(body(&plan)["plan"].as_bool().unwrap());
    assert!(!root.join(".twaco/handoffs/h1").exists());
    let recorded = call(
        2,
        "handoff",
        json!({"action":"record","name":"h1","files":["base.xml"],"dry_run":false}),
    );
    assert_eq!(body(&recorded)["recorded"]["name"], "h1");
    let listed = call(3, "handoff", json!({"action":"list"}));
    assert_eq!(body(&listed)["handoffs"][0]["name"], "h1");
    let report = call(4, "adopt_report", json!({"export":"base.xml","base":"h1"}));
    assert_eq!(body(&report)["base"], "handoff h1");
}

#[test]
fn catalog_answers_offline_with_counts_and_services() {
    let (_dir, root) = solution_dir();
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
    let (_dir, root) = solution_dir();
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
    let checked = types_tool_with_compiler(
        &solution,
        parse_typed(&json!({"action":"check"})).unwrap(),
        Some(&FakeCompiler),
        &crate::core::progress::NONE,
    )
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
        parse_typed(&json!({"action":"check", "detail":true})).unwrap(),
        Some(&FakeCompiler),
        &crate::core::progress::NONE,
    )
    .unwrap();
    assert_eq!(detailed["findings_list"].as_array().unwrap().len(), 1);
    assert!(detailed.get("first").is_none());
}

#[test]
fn sync_writes_a_sidecar_edit_and_refuses_while_the_workspace_is_locked() {
    let (_dir, root) = solution_dir();
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
}

#[test]
fn every_bad_message_gets_its_error_and_the_server_reads_on() {
    let (_dir, root) = solution_dir();
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
}

#[test]
fn arguments_of_the_wrong_shape_are_refused_before_the_tool_runs() {
    let (_dir, root) = solution_dir();
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
        (
            "sync",
            json!({}),
            "name an entity (entity or entities), or pass all: true",
        ),
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
    assert!(!root.join(".twaco/baselines/default.json").exists());
}

#[test]
fn a_status_with_an_unreadable_entity_file_records_nothing() {
    let (_dir, root) = solution_dir();
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
    assert!(!root.join(".twaco/baselines/default.json").exists());
}

#[test]
fn a_status_whose_reads_failed_records_nothing() {
    let (_dir, root) = solution_dir();
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
    assert!(!root.join(".twaco/baselines/default.json").exists());
}

#[test]
fn writing_tools_take_the_lock_before_they_read_anything() {
    // Each call would fail on what it reads (an unknown entity, no profile), so being
    // refused for the lock instead proves the lock came first.
    let (_dir, root) = solution_dir();
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
}

#[test]
fn a_held_workspace_lock_has_its_stable_code() {
    let (_dir, root) = solution_dir();
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
}

#[test]
fn a_service_call_is_a_dry_run_unless_asked_and_needs_no_server_to_be_one() {
    let (_dir, root) = solution_dir();
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
    // A dry run with a language says that a user's preference would change.
    let responses = converse(
        &root,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"call","arguments":{"target":"T","service":"Reset","as":"vera.viewer","lang":"de"}}}),
        ],
    );
    let text: Value = serde_json::from_str(
        responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(
        text["note"]
            .as_str()
            .unwrap()
            .contains("vera.viewer, whose language preference is set to de"),
        "{text}"
    );
}

#[test]
fn a_call_known_to_only_read_runs_without_dry_run_and_any_other_stays_a_plan() {
    // The server cannot be reached, so a call that runs fails reaching it; a plan answers.
    let (_dir, root) = solution_with_unreachable_server();
    let reached = |arguments: Value| {
        let result = call(&root, "call", arguments);
        let text = result["content"][0]["text"].as_str().unwrap().to_string();
        (result["isError"] == true, text)
    };
    let (failed, text) = reached(
        json!({"target": "Resources/InfoTableFunctions", "service": "Query", "parameters": {}}),
    );
    assert!(failed, "a platform read runs: {text}");
    let (failed, text) =
        reached(json!({"target": "Things/P.Outside", "service": "GetPropertyValues"}));
    assert!(failed, "a platform read runs: {text}");
    for arguments in [
        json!({"target": "Things/P.Outside", "service": "AddDataTableEntry"}),
        json!({"target": "Resources/InfoTableFunctions", "service": "Query", "dry_run": true}),
        json!({"target": "Things/P.Outside", "service": "GetPropertyValues", "as": "vera.viewer", "lang": "de"}),
    ] {
        let (failed, text) = reached(arguments.clone());
        assert!(!failed, "{arguments}: {text}");
        let body: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["dry_run"], true, "{arguments}");
    }
}

/// The `outputSchema` a tool publishes to the newest protocol revision.
fn output_schema_of(name: &str) -> Value {
    registry::definitions(LATEST)
        .into_iter()
        .find(|tool| tool["name"] == name)
        .and_then(|tool| tool.get("outputSchema").cloned())
        .unwrap_or_else(|| panic!("{name} publishes no output schema"))
}

fn call(root: &Path, name: &str, arguments: Value) -> Value {
    let responses = converse(
        root,
        &[
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        ],
    );
    responses[1]["result"].clone()
}

/// A solution whose server profile points at a port nothing listens on: every read fails at once,
/// and nothing is ever sent.
fn solution_with_unreachable_server() -> (tempfile::TempDir, PathBuf) {
    let (_dir, root) = solution_dir();
    std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        "url = \"http://127.0.0.1:9/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
    )
    .unwrap();
    (_dir, root)
}

#[test]
fn what_the_stable_tools_return_fits_the_output_schema_they_publish() {
    // The registry holds every successful result of a tool that publishes an output schema to it,
    // so these calls fail the test if a shape drifts. Each also has to be a success.
    let (_dir, root) = solution_with_unreachable_server();
    let calls = [
        ("projects", json!({})),
        ("check", json!({})),
        ("check", json!({"detail": true})),
        ("sync", json!({"all": true, "check": true})),
        ("extract", json!({"entity": "P.T"})),
        ("fmt", json!({"check": true})),
        ("types", json!({})),
        // The server cannot be reached, so every read fails: a result, not a tool error.
        ("status", json!({"all": true})),
        ("status", json!({"all": true, "detail": true})),
    ];
    for (name, arguments) in calls {
        let result = call(&root, name, arguments.clone());
        assert_ne!(result["isError"], true, "{name} {arguments}: {result}");
        let content = &result["structuredContent"];
        assert!(
            jsonschema::draft202012::is_valid(&output_schema_of(name), content),
            "{name} {arguments}: {content}"
        );
    }
}

#[test]
fn what_the_offline_tools_return_fits_the_output_schema_they_publish() {
    // As above: the registry checks each result against the tool's schema. These need no server,
    // or answer before reaching one.
    let (_dir, root) = solution_with_unreachable_server();
    std::fs::write(
        root.join("Things/P.T.xml"),
        "<Entities><Things><Thing name=\"P.T\" projectName=\"P\" thingTemplate=\"GenericThing\"><ThingShape><ServiceDefinitions><ServiceDefinition name=\"Run\" description=\"\"><ResultType baseType=\"NUMBER\" name=\"result\"/><ParameterDefinitions><FieldDefinition baseType=\"STRING\" name=\"from\"/></ParameterDefinitions></ServiceDefinition></ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[var result = 1;]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>",
    )
    .unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\n\n[[test.user]]\nname = \"tess\"\ngroups = [\"P.Users\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("tests/run.toml"),
        "[[case]]\nname = \"runs\"\nservice = \"P.T.Run\"\nas = [\"tess\"]\nexpect = 200\n",
    )
    .unwrap();
    // In order: some calls write what later ones read (the mashups, P.New, permissions.toml).
    let calls = [
        (
            "mashup_new",
            json!({"name": "P.Board_MU", "dry_run": false}),
        ),
        (
            "mashup_new",
            json!({"name": "P.Card_MU", "dry_run": false, "params": ["id:LONG:IN"]}),
        ),
        ("catalog", json!({"detail": true})),
        ("docs", json!({})),
        ("docs", json!({"detail": true})),
        ("doctor", json!({})),
        ("guide", json!({})),
        ("guide", json!({"text": "AddMember group"})),
        ("guide", json!({"topic": "workflow"})),
        ("guide", json!({"topic": "quirks"})),
        ("impact", json!({"entity": "P.T"})),
        ("impact", json!({"entity": "P.T", "detail": true})),
        ("impact", json!({"entity": "P.T", "format": "dot"})),
        ("trace", json!({"service": "P.T.Run"})),
        ("unused", json!({})),
        ("unused", json!({"detail": true})),
        ("unused", json!({"fields": true})),
        ("mashup_templates", json!({})),
        ("mashup_templates", json!({"name": "blank"})),
        ("widgets", json!({})),
        ("widgets", json!({"search": "grid"})),
        ("widgets", json!({"widget": "button"})),
        ("mashup_validate", json!({"all": true})),
        ("mashup_describe", json!({"mashup": "P.Board_MU"})),
        (
            "mashup_add",
            json!({"mashup": "P.Board_MU", "widget": "button"}),
        ),
        (
            "mashup_add_contained",
            json!({"mashup": "P.Board_MU", "child": "P.Card_MU"}),
        ),
        (
            "mashup_param",
            json!({"mashup": "P.Board_MU", "action": "add", "name": "x", "type": "STRING"}),
        ),
        (
            "mashup_event",
            json!({"mashup": "P.Card_MU", "name": "Saved", "when": "mashup:Loaded"}),
        ),
        (
            "mashup_recipe",
            json!({"mashup": "P.Board_MU", "recipe": "auto-refresh", "then": "data:P.T.Run", "every": 30}),
        ),
        ("bundle", json!({})),
        (
            "package",
            json!({"action": "source_control", "out": "out/sc.zip"}),
        ),
        (
            "package",
            json!({"action": "extension", "out": "out/p.zip", "project": "P"}),
        ),
        ("permissions_init", json!({})),
        ("requirements_init", json!({})),
        ("requirements_init", json!({"dry_run": false})),
        ("requirements_check", json!({})),
        ("requirements_check", json!({"detail": true})),
        ("permissions_init", json!({"dry_run": false})),
        ("permissions_audit", json!({})),
        ("permissions_audit", json!({"detail": true})),
        ("permissions_apply", json!({})),
        ("test", json!({})),
        ("entity_restore", json!({})),
        ("handoff", json!({"action": "list"})),
        (
            "entity_new",
            json!({"kind": "thing", "name": "P.New", "dry_run": false}),
        ),
        (
            "entity_new",
            json!({"kind": "timer", "name": "P.Tick", "every": "1m", "run_as": "Administrator"}),
        ),
        (
            "entity_edit",
            json!({"entity": "P.New", "action": "property_add", "name": "Speed", "type": "NUMBER"}),
        ),
        (
            "new_building_block",
            json!({"name": "P.Block", "type": "standard", "base_extension": "PTC.Base:1.0.0"}),
        ),
        (
            "rename",
            json!({"kind": "entity", "old": "P.T", "new": "P.U"}),
        ),
        (
            "retemplate",
            json!({"entity": "P.T", "template": "RemoteThing"}),
        ),
        (
            "move_member",
            json!({"action": "copy", "kind": "service", "from": "P.T", "to": "P.New", "name": "Run"}),
        ),
        ("call", json!({"target": "P.T", "service": "Run"})),
        (
            "localization_write",
            json!({"action": "new", "table": "P.Words"}),
        ),
        (
            "localization_write",
            json!({"action": "set", "token": "P.hello", "value": "Hello"}),
        ),
    ];
    let mut failures = Vec::new();
    for (name, arguments) in calls {
        let result = call(&root, name, arguments.clone());
        if result["isError"] == true {
            failures.push(format!(
                "{name} {arguments}: {}",
                result["content"][0]["text"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_deploy_the_gates_stop_answers_in_the_published_shape_and_says_why() {
    let (_dir, root) = solution_with_unreachable_server();
    std::fs::write(root.join("Things/Broken.xml"), "<Entities><Things><Thing").unwrap();
    let result = call(&root, "deploy", json!({}));
    let content = &result["structuredContent"];
    assert_eq!(content["stage"], "gates", "{result}");
    assert!(
        content["blocking_gates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gate| gate["first"].as_array().is_some_and(|first| first
                .iter()
                .any(|f| f.as_str().unwrap().contains("Broken.xml")))),
        "{result}"
    );
    assert!(jsonschema::draft202012::is_valid(
        &output_schema_of("deploy"),
        content
    ));
}

#[test]
fn every_push_result_fits_the_output_schema() {
    use crate::core::commands::push::PushOutcome;
    let schema = output_schema_of("push");
    let entity = || crate::core::entity::key::EntityKey::new("Things", "P.T").unwrap();
    let effects = commands::Effects::new(commands::Access::Read, commands::Access::Read);
    let refusal = push::Refusal::Conflict {
        server: "server".to_string(),
        baseline: "baseline".to_string(),
    };
    let plans = [
        push::Decision::AlreadyThere,
        push::Decision::Create,
        push::Decision::Update,
        push::Decision::Refuse(refusal.clone()),
    ];
    let applied = [
        push::Outcome::AlreadyThere,
        push::Outcome::Pushed { created: true },
        push::Outcome::Pushed { created: false },
        push::Outcome::Refused(refusal),
    ];
    let mut results = Vec::new();
    for decision in plans {
        let outcome = PushOutcome::Plan {
            server_owned: Vec::new(),
            entity: entity(),
            decision,
            effects,
        };
        results.push(push_outcome_json("Things/P.T", true, false, outcome));
    }
    for result in applied {
        let outcome = PushOutcome::Applied {
            server_owned: Vec::new(),
            entity: entity(),
            result,
            backup: None,
            effects,
        };
        let mut json = push_outcome_json("Things/P.T", false, true, outcome);
        json["backup"] = json!(".twaco/backups/x");
        results.push(json);
    }
    for result in results {
        assert!(
            jsonschema::draft202012::is_valid(&schema, &result),
            "{result}"
        );
    }
}

#[test]
fn package_reports_what_it_wrote_from_a_single_build() {
    let (_dir, root) = solution_dir();
    let bundle = call(
        &root,
        "package",
        json!({"action":"bundle","out":"out/release.xml"}),
    );
    assert_ne!(bundle["isError"], true, "{bundle}");
    let detail = &bundle["structuredContent"]["detail"];
    assert_eq!(
        (detail["entities"].as_u64(), detail["files"].as_u64()),
        (Some(2), Some(2))
    );
    let written = std::fs::read(root.join("out/release.xml")).unwrap();
    assert_eq!(
        bundle["structuredContent"]["bytes"].as_u64(),
        Some(written.len() as u64)
    );
    let extension = call(
        &root,
        "package",
        json!({"action":"extension","out":"out/extension.zip","project":"P"}),
    );
    let detail = &extension["structuredContent"]["detail"];
    assert_eq!(detail["entities"], 2, "{extension}");
    assert!(detail["version"].is_string());
    let solution = call(
        &root,
        "package",
        json!({"action":"extension","out":"out/all.zip"}),
    );
    assert_eq!(
        solution["structuredContent"]["detail"]["projects"][0]["project"], "P",
        "{solution}"
    );
}

fn call_tool(root: &Path, tool: &str, arguments: Value) -> (bool, Value) {
    let reply = converse(
        root,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":arguments}}),
        ],
    );
    let result = &reply[0]["result"];
    let body = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    (result["isError"] == true, body)
}

#[test]
fn the_tools_do_what_the_command_line_does() {
    let (_dir, root) = solution_dir();

    // `twaco guide` with nothing lists the topics; with words it searches.
    let (failed, listed) = call_tool(&root, "guide", json!({}));
    assert!(!failed && listed["topics"].is_array(), "{listed}");
    let (failed, found) = call_tool(&root, "guide", json!({"text": "AddMember group"}));
    assert!(!failed && found["results"].is_array(), "{found}");

    // Several entities, as the command line takes them: every name is resolved.
    for (tool, extra) in [("extract", json!({})), ("sync", json!({"check": true}))] {
        let with = |names: Value| {
            let mut arguments = extra.clone();
            for (key, value) in names.as_object().unwrap() {
                arguments[key] = value.clone();
            }
            arguments
        };
        let (failed, body) = call_tool(&root, tool, with(json!({"entities": ["P.T"]})));
        assert!(!failed, "{tool}: {body}");
        let (failed, body) = call_tool(
            &root,
            tool,
            with(json!({"entity": "P.T", "entities": ["P.Nope"]})),
        );
        assert!(failed, "{tool}: {body}");
        assert_eq!(body["code"], "unknown_entity", "{tool}: {body}");
    }

    // A backup goes into the solution, checked before anything is asked of the server.
    for (backup, code) in [
        ("../outside.json", "invalid_arguments"),
        ("twaco.toml", "already_exists"),
    ] {
        let (failed, body) = call_tool(
            &root,
            "config_table",
            json!({"thing": "P.T", "table": "Settings", "action": "backup", "backup": backup}),
        );
        assert!(failed, "{backup}: {body}");
        assert_eq!(body["code"], code, "{backup}: {body}");
    }

    // A restore reads its backup only from inside the solution, before the server is asked.
    let outside_dir = tempfile::Builder::new()
        .prefix("twaco-outside-")
        .tempdir()
        .unwrap();
    let outside = outside_dir.path().join("outside.json");
    std::fs::write(&outside, "{}").unwrap();
    let (failed, body) = call_tool(
        &root,
        "config_table",
        json!({"thing": "P.T", "table": "Settings", "action": "restore", "backup": outside.to_string_lossy()}),
    );
    assert!(failed, "{body}");
    assert_eq!(body["code"], "invalid_arguments", "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("outside the solution"),
        "{body}"
    );

    // doctor works without a profile, and says the server commands need one.
    let (failed, body) = call_tool(&root, "doctor", json!({"profile": "no-such-profile"}));
    assert!(!failed && body["ok"] == true, "{body}");
    let subjects: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["subject"].as_str().unwrap())
        .collect();
    assert!(
        subjects.contains(&"solution") && subjects.contains(&"profile"),
        "{body}"
    );

    // bundle plans by default, as `twaco bundle --check`, and writes nothing.
    let (failed, body) = call_tool(&root, "bundle", json!({}));
    assert!(!failed, "{body}");
    assert_eq!(body["state"], "missing", "{body}");
    assert_eq!(body["ok"], false, "{body}");
    let target = root.join(body["bundle"].as_str().unwrap());
    assert!(!target.exists(), "a dry run wrote {}", target.display());
    let (failed, body) = call_tool(&root, "bundle", json!({"dry_run": false}));
    assert!(!failed && body["state"] == "written", "{body}");
    assert!(target.is_file());
    let (_, body) = call_tool(&root, "bundle", json!({}));
    assert_eq!(body["state"], "current", "{body}");
}

/// A server that answers every entity read with the same XML, so a status read has work to do.
fn entity_server() -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buffer = [0u8; 8192];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let path = request.split_whitespace().nth(1).unwrap_or("");
            let name = path.rsplit('/').next().unwrap_or("");
            let body = format!(
                "<Entities><Things><Thing name=\"{name}\" projectName=\"Test\"></Thing></Things></Entities>"
            );
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    address
}

#[test]
fn status_record_returns_matching_entities_in_sync_and_keeps_differing_verdicts() {
    let body = |result: &Value| {
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 8192];
            let read = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..read]);
            let name = request
                .split_whitespace()
                .nth(1)
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap();
            let xml = format!("<Entities><Things><Thing name=\"{name}\" projectName=\"Acme.App\"></Thing></Things></Entities>");
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",xml.len()).unwrap();
        }
    });
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname='Acme.App'\n").unwrap();
    std::fs::create_dir(root.join("Things")).unwrap();
    for (name, value) in [
        ("Acme.Same", ""),
        ("Acme.Different", "<value>changed</value>"),
    ] {
        std::fs::write(root.join(format!("Things/{name}.xml")),format!("<Entities><Things><Thing name=\"{name}\" projectName=\"Acme.App\">{value}</Thing></Things></Entities>")).unwrap();
    }
    std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        format!(
            "url='http://{}/Thingworx/'\nusername='user'\npassword='pass'\n",
            address
        ),
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".twaco/baselines")).unwrap();
    std::fs::write(root.join(".twaco/baselines/default.json"),r#"{"entities":{"Things":{"Acme.Same":{"local":"v7:old","server":"v7:old"},"Acme.Different":{"local":"v7:old","server":"v7:old"}}}}"#).unwrap();
    let before = call(root, "status", json!({"all":true,"detail":true}));
    assert_eq!(body(&before)["counts"]["converged"], 1);
    assert_eq!(body(&before)["counts"]["both-changed"], 1);
    let after = call(
        root,
        "status",
        json!({"all":true,"detail":true,"record":true}),
    );
    assert_ne!(after["isError"], true, "{after}");
    let result = body(&after);
    assert_eq!(result["recorded"], 1);
    assert_eq!(result["counts"]["in-sync"], 1);
    assert_eq!(result["counts"]["both-changed"], 1);
    let entries = result["statuses"].as_array().unwrap();
    let same = entries
        .iter()
        .find(|s| s["entity"] == "Things/Acme.Same")
        .unwrap();
    assert_eq!(same["verdict"], "in-sync");
    assert_eq!(same["working"], same["baseline_local"]);
    assert_eq!(same["server"], same["baseline_server"]);
    let single = call(
        root,
        "status",
        json!({"entity":"Things/Same","record":true}),
    );
    assert_eq!(body(&single)["ok"], true);
}

fn status_workspace() -> (tempfile::TempDir, PathBuf) {
    let (guard, root) = solution_dir();
    for n in 0..6 {
        std::fs::write(
            root.join(format!("Things/P.T{n}.xml")),
            format!("<Entities><Things><Thing name=\"P.T{n}\" projectName=\"P\"></Thing></Things></Entities>"),
        )
        .unwrap();
    }
    std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        format!(
            "url = \"http://{}/Thingworx/\"\nusername = \"user\"\npassword = \"pass\"\n",
            entity_server()
        ),
    )
    .unwrap();
    (guard, root)
}

/// A server that answers every request for JSON with an empty InfoTable and every other request with
/// the XML of a Thing named by the path's last segment. It reads each request whole first, so a
/// request body is never cut off.
fn service_server() -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut request = Vec::new();
            let mut buffer = [0u8; 8192];
            let complete = loop {
                let Ok(read) = stream.read(&mut buffer) else {
                    break false;
                };
                if read == 0 {
                    break false;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break true;
                }
            };
            if !complete {
                continue;
            }
            let request = String::from_utf8_lossy(&request).into_owned();
            let path = request.split_whitespace().nth(1).unwrap_or("");
            let name = path.rsplit('/').next().unwrap_or("");
            let (body, kind) = if request
                .to_ascii_lowercase()
                .contains("accept: application/json")
            {
                (
                    "{\"dataShape\":{\"fieldDefinitions\":{}},\"rows\":[]}".to_string(),
                    "application/json",
                )
            } else {
                (
                    format!("<Entities><Things><Thing name=\"{name}\" projectName=\"P\"></Thing></Things></Entities>"),
                    "text/xml",
                )
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    address
}

#[test]
fn what_the_server_tools_return_fits_the_output_schema_they_publish() {
    // A server whose every answer is empty, or one entity: enough for these tools to succeed. The
    // registry checks each result against the tool's schema.
    let (_dir, root) = status_workspace();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\n\n[[test.user]]\nname = \"tess\"\ngroups = [\"P.Users\"]\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        format!(
            "url = \"http://{}/Thingworx/\"\nusername = \"user\"\npassword = \"pass\"\n",
            service_server()
        ),
    )
    .unwrap();
    let calls = [
        ("entity_get", json!({"entity": "Things/P.T0"})),
        (
            "entity_get",
            json!({"entity": "Things/P.T0", "max_chars": 10}),
        ),
        ("permissions", json!({"all": true})),
        ("permissions_push", json!({"all": true})),
        (
            "export",
            json!({"action": "entity", "entity": "Things/P.T0", "out": "out/t0.xml"}),
        ),
        ("entity_delete", json!({"entities": ["Things/P.T0"]})),
        (
            "entity_carry",
            json!({"pairs": ["Things/P.T0", "Things/P.T1"]}),
        ),
        ("search", json!({"text": "P"})),
        ("settings", json!({})),
        ("users", json!({})),
        ("extensions", json!({})),
        ("repo", json!({})),
        (
            "repo",
            json!({"action": "ls", "repository": "SystemRepository"}),
        ),
        (
            "repo_write",
            json!({"action": "mkdir", "repository": "SystemRepository", "path": "/x"}),
        ),
        (
            "call",
            json!({"target": "P.T0", "service": "Run", "dry_run": false}),
        ),
        ("settings", json!({"action": "search", "text": "x"})),
        ("db_clean", json!({})),
        ("localization", json!({})),
        ("localization_write", json!({"action": "pull"})),
        (
            "export",
            json!({"action": "source_control", "repository": "SystemRepository", "path": "/x", "project": "P"}),
        ),
        (
            "repo",
            json!({"action": "status", "repository": "SystemRepository"}),
        ),
        (
            "repo_write",
            json!({"action": "pull", "repository": "SystemRepository"}),
        ),
        ("logs", json!({"log": "ScriptLog"})),
        (
            "config_table",
            json!({"thing": "P.T0", "table": "Settings"}),
        ),
    ];
    let mut failures = Vec::new();
    for (name, arguments) in calls {
        let result = call(&root, name, arguments.clone());
        if result["isError"] == true {
            failures.push(format!(
                "{name} {arguments}: {}",
                result["content"][0]["text"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn status_call(id: u64, meta: Option<Value>) -> Value {
    let mut params = json!({ "name": "status", "arguments": { "all": true } });
    if let Some(meta) = meta {
        params["_meta"] = meta;
    }
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params })
}

fn progress_lines(lines: &[Value]) -> Vec<&Value> {
    lines
        .iter()
        .filter(|line| line["method"] == "notifications/progress")
        .collect()
}

#[test]
fn a_call_with_a_progress_token_gets_rising_notifications_before_its_response() {
    let (_guard, root) = status_workspace();
    let lines = converse(
        &root,
        &[status_call(1, Some(json!({ "progressToken": "tok-1" })))],
    );
    let notifications = progress_lines(&lines);
    assert!(notifications.len() >= 2, "{lines:?}");
    let values: Vec<u64> = notifications
        .iter()
        .map(|line| line["params"]["progress"].as_u64().unwrap())
        .collect();
    assert!(
        values.windows(2).all(|pair| pair[0] < pair[1]),
        "{values:?}"
    );
    assert_eq!(*values.last().unwrap(), 8, "all 8 entities counted");
    assert!(notifications
        .iter()
        .all(|line| line["params"]["progressToken"] == "tok-1" && line["jsonrpc"] == "2.0"));
    // The response is the last line, and it answers the request.
    let response = lines.last().unwrap();
    assert_eq!(response["id"], 1);
    assert!(response.get("result").is_some(), "{response}");
    assert_eq!(lines.len(), notifications.len() + 1);
}

#[test]
fn a_call_without_a_progress_token_gets_no_notifications() {
    let (_guard, root) = status_workspace();
    let lines = converse(
        &root,
        &[status_call(1, None), status_call(2, Some(json!({})))],
    );
    assert!(progress_lines(&lines).is_empty(), "{lines:?}");
    assert_eq!(lines.len(), 2);
}

#[test]
fn a_number_token_is_echoed_as_a_number() {
    let (_guard, root) = status_workspace();
    let lines = converse(
        &root,
        &[status_call(5, Some(json!({ "progressToken": 42 })))],
    );
    let notifications = progress_lines(&lines);
    assert!(!notifications.is_empty());
    assert!(notifications
        .iter()
        .all(|line| line["params"]["progressToken"] == 42));
}

// The default is a dry run, so the phase says that it plans.
#[test]
fn a_push_with_a_progress_token_reports_its_entity() {
    let (_guard, root) = status_workspace();
    let lines = converse(
        &root,
        &[json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {
                "name": "push",
                "arguments": { "entity": "P.T0" },
                "_meta": { "progressToken": "push-1" }
            }
        })],
    );
    let notifications = progress_lines(&lines);
    assert!(!notifications.is_empty(), "{lines:?}");
    assert!(notifications
        .iter()
        .all(|line| line["params"]["progressToken"] == "push-1"));
    assert!(
        notifications.iter().any(|line| line["params"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("planning push"))),
        "{notifications:?}"
    );
}

#[test]
fn a_deploy_the_localization_tables_stop_says_whether_the_entities_were_imported() {
    use crate::core::commands::deploy::DeployCommandError;
    use crate::core::localization::LocalizationError;
    for deployed in [false, true] {
        let error = DeployCommandError::Localization {
            why: Box::new(LocalizationError::NotVerified(
                "de/Acme.App.Save".to_string(),
            )),
            deployed,
            backup: deployed.then(|| ".twaco/backups/x".to_string()),
            gates: None,
        };
        let value = super::source::localization_failure(&error);
        assert_eq!(value["ok"], false);
        assert_eq!(value["stage"], "localization");
        assert_eq!(value["entities_deployed"], deployed);
        assert_eq!(value["backup"].is_string(), deployed);
        let note = value["note"].as_str().unwrap();
        assert_eq!(
            note.contains("the entities were deployed"),
            deployed,
            "{note}"
        );
    }
}

#[test]
fn mashup_new_plans_by_default_and_creates_the_files_when_asked() {
    let (_dir, root) = solution_dir();
    let arguments = json!({"name": "P.Orders_MU", "visibility": ["Organization/P.Org"]});
    let planned = call(&root, "mashup_new", arguments.clone());
    assert_eq!(planned["isError"], false, "{planned}");
    let report = &planned["structuredContent"];
    assert_eq!(report["applied"], false);
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["visibility"], json!(["Organization/P.Org"]));
    assert!(!root.join("Mashups/P.Orders_MU.xml").exists());

    let mut apply = arguments;
    apply["dry_run"] = json!(false);
    let applied = call(&root, "mashup_new", apply.clone());
    assert_eq!(applied["structuredContent"]["applied"], true, "{applied}");
    assert!(root.join("Mashups/P.Orders_MU.xml").exists());
    assert!(root.join("src/P.Orders_MU/mashup/content.json").exists());
    // Again: the mashup exists now, and the refusal says so with its code.
    let again = call(&root, "mashup_new", apply);
    assert_eq!(again["isError"], true);
    let refusal: Value =
        serde_json::from_str(again["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(refusal["code"], "already_exists", "{again}");
}

#[test]
fn entity_new_timer_applies_and_writes_the_entity_file() {
    let (_dir, root) = solution_dir();
    let planned = call(
        &root,
        "entity_new",
        json!({"kind": "timer", "name": "P.Refresh_TM", "run_as": "Administrator"}),
    );
    assert_eq!(planned["isError"], false, "{planned}");
    assert_eq!(planned["structuredContent"]["applied"], false);
    assert!(!root.join("Things/P.Refresh_TM.xml").exists());
    let applied = call(
        &root,
        "entity_new",
        json!({"kind": "timer", "name": "P.Refresh_TM", "run_as": "Administrator", "dry_run": false}),
    );
    assert_eq!(applied["isError"], false, "{applied}");
    assert_eq!(applied["structuredContent"]["applied"], true);
    assert!(root.join("Things/P.Refresh_TM.xml").exists());
}

#[test]
fn entity_new_style_applies() {
    let (_dir, root) = solution_dir();
    let applied = call(
        &root,
        "entity_new",
        json!({"kind": "style", "name": "P.Good_SD", "background": "#66cc33", "bold": true, "dry_run": false}),
    );
    assert_eq!(applied["isError"], false, "{applied}");
    assert_eq!(applied["structuredContent"]["applied"], true);
    assert!(root.join("StyleDefinitions/P.Good_SD.xml").exists());
}

#[test]
fn entity_edit_property_add_plans_then_applies() {
    let (_dir, root) = solution_dir();
    let created = call(
        &root,
        "entity_new",
        json!({"kind": "thing", "name": "P.Press", "dry_run": false}),
    );
    assert_eq!(created["isError"], false, "{created}");
    let arguments = json!({
        "action": "property_add", "entity": "P.Press", "name": "Speed", "type": "number",
        "default": "5", "persistent": true
    });
    let planned = call(&root, "entity_edit", arguments.clone());
    assert_eq!(planned["isError"], false, "{planned}");
    assert_eq!(planned["structuredContent"]["applied"], false);
    let path = root.join("Things/P.Press.xml");
    assert!(!std::fs::read_to_string(&path).unwrap().contains("Speed"));
    let mut apply = arguments;
    apply["dry_run"] = json!(false);
    let applied = call(&root, "entity_edit", apply);
    assert_eq!(applied["isError"], false, "{applied}");
    assert_eq!(
        applied["structuredContent"]["summary"],
        "add property Speed:NUMBER"
    );
    let xml = std::fs::read_to_string(&path).unwrap();
    assert!(xml.contains("name=\"Speed\""));
    assert!(xml.contains("<Speed><Value>5.0</Value>"));
    let missing = call(
        &root,
        "entity_edit",
        json!({"action": "property_add", "entity": "P.Press", "name": "Rate"}),
    );
    assert_eq!(missing["isError"], true, "{missing}");
}

#[test]
fn entity_edit_service_add_plans_then_removes() {
    let (_dir, root) = solution_dir();
    let created = call(
        &root,
        "entity_new",
        json!({"kind": "thing", "name": "P.Press", "dry_run": false}),
    );
    assert_eq!(created["isError"], false, "{created}");
    let arguments = json!({
        "action": "service_add", "entity": "P.Press", "name": "ReadRows",
        "params": ["limit:INTEGER"], "returns": "NUMBER", "then": "return 1;"
    });
    let planned = call(&root, "entity_edit", arguments.clone());
    assert_eq!(planned["isError"], false, "{planned}");
    assert_eq!(planned["structuredContent"]["applied"], false);
    let mut apply = arguments;
    apply["dry_run"] = json!(false);
    let applied = call(&root, "entity_edit", apply);
    assert_eq!(applied["isError"], false, "{applied}");
    let folder = root.join("src/P.Press/services/ReadRows");
    assert!(folder.join("definition.xml").is_file());
    let removed = call(
        &root,
        "entity_edit",
        json!({"action": "service_remove", "entity": "P.Press", "name": "ReadRows", "dry_run": false}),
    );
    assert_eq!(removed["isError"], false, "{removed}");
    assert!(!folder.exists());
}

#[test]
fn trace_reports_a_service_and_refuses_an_unknown_one() {
    let (_dir, root) = solution_dir();
    let created = call(
        &root,
        "entity_new",
        json!({"kind": "thing", "name": "P.Press", "dry_run": false}),
    );
    assert_eq!(created["isError"], false, "{created}");
    std::fs::write(
        root.join("Things/P.Report.xml"),
        "<Entities><Things><Thing name=\"P.Report\" projectName=\"P\"><ThingShape>\
         <ServiceDefinitions><ServiceDefinition name=\"Build\"><ResultType baseType=\"NUMBER\" \
         name=\"result\"></ResultType><ParameterDefinitions><FieldDefinition baseType=\"STRING\" \
         name=\"from\" aspect.defaultValue=\"today\"></FieldDefinition></ParameterDefinitions>\
         </ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>",
    )
    .unwrap();
    let traced = call(&root, "trace", json!({"service": "P.Report.Build"}));
    assert_eq!(traced["isError"], false, "{traced}");
    let result = &traced["structuredContent"];
    assert_eq!(result["service"], "Things/P.Report.Build");
    assert_eq!(result["returns"], "NUMBER");
    assert_eq!(result["parameters"][0]["default"], "today");
    let unknown = call(&root, "trace", json!({"service": "P.Press.Nope"}));
    assert_eq!(unknown["isError"], true, "{unknown}");
    let bad = call(&root, "trace", json!({"service": "nodot"}));
    assert_eq!(bad["isError"], true, "{bad}");
}

#[test]
fn mashup_add_applies_a_widget_and_refuses_two_placements() {
    let (_dir, root) = solution_dir();
    let created = call(
        &root,
        "mashup_new",
        json!({"name": "Acme.Orders_MU", "dry_run": false}),
    );
    assert_eq!(created["isError"], false, "{created}");
    let grid = call(
        &root,
        "mashup_add",
        json!({"mashup": "Acme.Orders_MU", "widget": "grid", "dry_run": false}),
    );
    assert_eq!(grid["isError"], false, "{grid}");
    assert_eq!(grid["structuredContent"]["applied"], true);
    assert_eq!(grid["structuredContent"]["id"], "ptcsgrid-3");
    let label = call(
        &root,
        "mashup_add",
        json!({"mashup": "Acme.Orders_MU", "widget": "label", "below": "ptcsgrid-3", "label": "Total", "dry_run": false}),
    );
    assert_eq!(label["isError"], false, "{label}");
    assert_eq!(label["structuredContent"]["applied"], true);
    assert_eq!(label["structuredContent"]["id"], "ptcslabel-4");
    let invalid = call(
        &root,
        "mashup_add",
        json!({"mashup": "Acme.Orders_MU", "widget": "label", "in": "center", "below": "ptcsgrid-3"}),
    );
    assert_eq!(invalid["isError"], true, "{invalid}");
    assert!(
        invalid
            .to_string()
            .contains("give one of in, right_of, left_of, above, below"),
        "{invalid}"
    );
}

#[test]
fn mashup_add_contained_applies_a_repository_child() {
    let (_dir, root) = solution_dir();
    assert_eq!(
        call(
            &root,
            "mashup_new",
            json!({"name": "Acme.Card_MU", "params": ["id:LONG:IN"], "dry_run": false})
        )["isError"],
        false
    );
    assert_eq!(
        call(
            &root,
            "mashup_new",
            json!({"name": "Acme.Board_MU", "dry_run": false})
        )["isError"],
        false
    );
    let applied = call(
        &root,
        "mashup_add_contained",
        json!({"mashup": "Acme.Board_MU", "child": "Acme.Card_MU", "set": {"id": "7"}, "dry_run": false}),
    );
    assert_eq!(applied["isError"], false, "{applied}");
    assert_eq!(applied["structuredContent"]["applied"], true);
    assert_eq!(applied["structuredContent"]["id"], "mashupcontainer-3");
}

#[test]
fn mashup_bind_adds_a_thing_call_after_creating_a_grid() {
    let (_dir, root) = solution_dir();
    std::fs::write(
        root.join("Things/Acme.Orders.Manager.xml"),
        r#"<Entities><Things><Thing name="Acme.Orders.Manager" projectName="P"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    ).unwrap();
    assert_eq!(
        call(
            &root,
            "mashup_new",
            json!({"name": "Acme.Orders_MU", "dry_run": false})
        )["isError"],
        false
    );
    assert_eq!(
        call(
            &root,
            "mashup_add",
            json!({"mashup": "Acme.Orders_MU", "widget": "grid", "dry_run": false})
        )["isError"],
        false
    );
    let bound = call(
        &root,
        "mashup_bind",
        json!({
            "mashup": "Acme.Orders_MU",
            "from": "data:Acme.Orders.Manager.GetOrders",
            "to": "ui:ptcsgrid-3.Data",
            "dry_run": false
        }),
    );
    assert_eq!(bound["isError"], false, "{bound}");
    assert_eq!(bound["structuredContent"]["applied"], true);
    assert_eq!(
        bound["structuredContent"]["new_source"],
        "Acme.Orders.Manager.GetOrders"
    );
}

#[test]
fn mashup_on_runs_a_service_after_creating_a_button() {
    let (_dir, root) = solution_dir();
    std::fs::write(
        root.join("Things/Acme.Orders.Manager.xml"),
        r#"<Entities><Things><Thing name="Acme.Orders.Manager" projectName="P"><ThingShape><ServiceDefinitions><ServiceDefinition name="SaveOrder"><ResultType baseType="NOTHING"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    ).unwrap();
    assert_eq!(
        call(
            &root,
            "mashup_new",
            json!({"name": "Acme.Orders_MU", "dry_run": false})
        )["isError"],
        false
    );
    let button = call(
        &root,
        "mashup_add",
        json!({"mashup": "Acme.Orders_MU", "widget": "button", "dry_run": false}),
    );
    assert_eq!(button["isError"], false, "{button}");
    let on = call(
        &root,
        "mashup_on",
        json!({"mashup": "Acme.Orders_MU", "event": "ui:ptcsbutton-3.Clicked", "invoke": "data:Acme.Orders.Manager.SaveOrder", "dry_run": false}),
    );
    assert_eq!(on["isError"], false, "{on}");
    assert_eq!(on["structuredContent"]["applied"], true);
    assert_eq!(
        on["structuredContent"]["new_source"],
        "Acme.Orders.Manager.SaveOrder"
    );
}

#[test]
fn widgets_returns_catalogue_details_and_refuses_two_selectors() {
    let (_dir, root) = solution_dir();
    let button = call(&root, "widgets", json!({"widget": "button"}));
    assert_eq!(button["isError"], false, "{button}");
    assert!(button["structuredContent"]["events"]
        .as_array()
        .is_some_and(|events| events.iter().any(|event| event == "Clicked")));
    let listed = call(&root, "widgets", json!({"search": "list"}));
    assert!(listed["structuredContent"]["widgets"]
        .as_array()
        .is_some_and(|widgets| !widgets.is_empty()));
    let both = call(
        &root,
        "widgets",
        json!({"widget": "button", "search": "list"}),
    );
    assert_eq!(both["isError"], true, "{both}");
}

#[test]
fn mashup_templates_lists_every_template_and_new_uses_template() {
    let (_dir, root) = solution_dir();
    let listed = call(&root, "mashup_templates", json!({}));
    assert_eq!(listed["isError"], false, "{listed}");
    assert_eq!(
        listed["structuredContent"]["templates"]
            .as_array()
            .unwrap()
            .len(),
        14
    );
    let created = call(
        &root,
        "mashup_new",
        json!({"name": "Acme.Card_MU", "template": "card", "dry_run": false}),
    );
    assert_eq!(created["isError"], false, "{created}");
    assert_eq!(created["structuredContent"]["template"], "card");
}

/// Input fed line by line while the server runs, so a test decides when each message arrives.
struct LiveInput {
    lines: std::sync::mpsc::Receiver<String>,
    buffer: std::io::Cursor<Vec<u8>>,
}

impl std::io::Read for LiveInput {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        use std::io::BufRead;
        let available = self.fill_buf()?;
        let n = available.len().min(out.len());
        out[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl std::io::BufRead for LiveInput {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if usize::try_from(self.buffer.position()).unwrap() >= self.buffer.get_ref().len() {
            // A closed channel is the end of input.
            let line = self.lines.recv().unwrap_or_default();
            self.buffer = std::io::Cursor::new(line.into_bytes());
        }
        let position = usize::try_from(self.buffer.position()).unwrap();
        Ok(&self.buffer.get_ref()[position..])
    }

    fn consume(&mut self, n: usize) {
        let position = self.buffer.position();
        self.buffer.set_position(position + n as u64);
    }
}

/// Output the test can read while the server still writes it.
#[derive(Clone, Default)]
struct SharedOutput(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl SharedOutput {
    fn lines(&self) -> Vec<Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Wait until a response with this id has been written.
    fn wait_for(&self, id: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(line) = self.lines().into_iter().find(|line| line["id"] == id) {
                return line;
            }
            assert!(Instant::now() < deadline, "no response to {id}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A server that holds every request until the test releases it.
fn gated_entity_server() -> (
    std::net::SocketAddr,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (arrived, requests) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        // Released once, the server answers every later request at once.
        let mut open = false;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buffer = [0u8; 8192];
            let _ = stream.read(&mut buffer);
            let _ = arrived.send(());
            if !open {
                if released.recv().is_err() {
                    return;
                }
                open = true;
            }
            let body = "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"></Thing></Things></Entities>";
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (address, requests, release)
}

#[test]
fn ping_is_answered_and_cancellation_honoured_while_a_call_runs() {
    let (_dir, root) = solution_dir();
    let (address, requests, release) = gated_entity_server();
    std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        format!(
            "url = \"http://{address}/Thingworx/\"\nusername = \"user\"\npassword = \"pass\"\n"
        ),
    )
    .unwrap();
    let (send, lines) = std::sync::mpsc::channel::<String>();
    let output = SharedOutput::default();
    let server = {
        let root = root.clone();
        let output = output.clone();
        std::thread::spawn(move || {
            let input = LiveInput {
                lines,
                buffer: std::io::Cursor::new(Vec::new()),
            };
            serve(&root, input, output).unwrap();
        })
    };
    let say = |message: Value| send.send(format!("{message}\n")).unwrap();

    // Call 1 reaches the server and waits there.
    say(status_call(1, None));
    requests.recv_timeout(Duration::from_secs(20)).unwrap();
    // The server still answers.
    say(json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
    assert_eq!(output.wait_for(2)["result"], json!({}));
    assert!(
        output.lines().iter().all(|line| line["id"] != 1),
        "call 1 is still running"
    );
    // Call 3 queues behind it and is cancelled before it starts: it never runs.
    say(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"projects","arguments":{}}}),
    );
    say(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}));
    // Call 4 is not cancelled and runs once call 1 is done.
    say(
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"projects","arguments":{}}}),
    );
    // Call 1 is cancelled while it runs: it finishes, but sends no response.
    say(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}));
    say(json!({"jsonrpc":"2.0","id":5,"method":"ping"}));
    output.wait_for(5);
    release.send(()).unwrap();
    assert_eq!(output.wait_for(4)["result"]["isError"], false);
    drop(send);
    server.join().unwrap();
    let ids: Vec<Value> = output
        .lines()
        .iter()
        .map(|line| line["id"].clone())
        .collect();
    assert_eq!(
        ids,
        [json!(2), json!(5), json!(4)],
        "no response for 1 or 3"
    );
}

#[test]
fn a_tool_group_lists_and_runs_only_its_tools() {
    let mut grouped: Vec<&str> = registry::GROUPS
        .iter()
        .flat_map(|(_, tools)| tools.iter().copied())
        .collect();
    grouped.sort_unstable();
    let mut registered: Vec<String> = tool_definitions()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    registered.sort_unstable();
    assert_eq!(grouped, registered, "every tool is in exactly one group");

    let core = Selection::parse("core").unwrap();
    let listed = registry::selected_definitions(LATEST, &core);
    assert!(listed.iter().any(|tool| tool["name"] == "deploy"));
    assert!(listed.iter().all(|tool| tool["name"] != "mashup_add"));
    // The point of a group: a client that needs the everyday loop pays for it alone.
    let bytes = |tools: &[Value]| serde_json::to_string(tools).unwrap().len();
    assert!(
        bytes(&listed) * 3 < bytes(&tool_definitions()),
        "core is {} of {} bytes",
        bytes(&listed),
        bytes(&tool_definitions())
    );
    assert_eq!(Selection::parse("all").unwrap(), Selection::ALL);
    assert_eq!(
        Selection::parse("core, mashup").unwrap(),
        Selection::parse("core,mashup").unwrap()
    );
    let error = Selection::parse("core,mashups").unwrap_err();
    assert!(
        error.contains("`mashups`") && error.contains("reference"),
        "{error}"
    );
    assert!(Selection::parse(" , ").is_err());

    // A tool outside the group is unknown to this server.
    let (_dir, root) = solution_dir();
    let input = format!(
        "{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"widgets","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"projects","arguments":{}}}),
    );
    let mut output = Vec::new();
    serve_tools(&root, &core, input.as_bytes(), &mut output).unwrap();
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["error"]["code"], -32602, "{}", lines[0]);
    assert_eq!(lines[1]["result"]["isError"], false, "{}", lines[1]);
}

#[test]
fn a_line_longer_than_the_cap_is_dropped_and_the_next_one_still_reads() {
    // A small buffer makes the long line arrive over several reads.
    let mut input = std::io::BufReader::with_capacity(4, &b"0123456789\nshort\ntail"[..]);
    let mut bytes = Vec::new();
    assert_eq!(
        super::read_line_capped(&mut input, &mut bytes, 8).unwrap(),
        Some(false)
    );
    assert!(bytes.is_empty());
    bytes.clear();
    assert_eq!(
        super::read_line_capped(&mut input, &mut bytes, 8).unwrap(),
        Some(true)
    );
    assert_eq!(bytes, b"short\n");
    bytes.clear();
    assert_eq!(
        super::read_line_capped(&mut input, &mut bytes, 8).unwrap(),
        Some(true)
    );
    assert_eq!(bytes, b"tail");
    bytes.clear();
    assert_eq!(
        super::read_line_capped(&mut input, &mut bytes, 8).unwrap(),
        None
    );
}

#[test]
fn a_call_beyond_the_queue_is_answered_busy_and_forgotten() {
    let call = |id: u64| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"projects","arguments":{}}});
    let input = format!("{}\n{}\n", call(1), call(2));
    let output = std::sync::Mutex::new(Vec::new());
    let pending = super::Pending::default();
    // Nothing takes from this queue, so it is full after one call.
    let (queue, _calls) = std::sync::mpsc::sync_channel(1);
    super::read_messages(&Selection::ALL, input.as_bytes(), &output, &pending, &queue).unwrap();
    let written = String::from_utf8(output.into_inner().unwrap()).unwrap();
    let lines: Vec<Value> = written
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{written}");
    assert_eq!(lines[0]["id"], 2);
    assert_eq!(lines[0]["error"]["code"], -32000, "{}", lines[0]);
    assert!(
        !pending.cancel(&json!(2)),
        "the refused call is no longer pending"
    );
    assert!(pending.cancel(&json!(1)), "the queued call still is");
}

#[test]
fn a_refused_call_that_reuses_an_id_leaves_the_earlier_call_cancellable() {
    let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects","arguments":{}}});
    let input = format!("{call}\n{call}\n");
    let output = std::sync::Mutex::new(Vec::new());
    let pending = super::Pending::default();
    let (queue, calls) = std::sync::mpsc::sync_channel(1);
    super::read_messages(&Selection::ALL, input.as_bytes(), &output, &pending, &queue).unwrap();
    let queued: super::Call = calls.try_recv().unwrap();
    assert!(pending.cancel(&json!(1)));
    assert!(queued.cancelled.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn a_finished_call_leaves_a_later_call_with_its_id_cancellable() {
    let pending = super::Pending::default();
    let id = json!(7);
    let first = pending.start(&id);
    let second = pending.start(&id);
    pending.finish(&id, &first);
    assert!(pending.cancel(&id));
    assert!(second.load(std::sync::atomic::Ordering::SeqCst));
    pending.finish(&id, &second);
    assert!(!pending.cancel(&id));
}

#[test]
fn network_tools_show_a_hierarchy_and_edit_it_through_a_dry_run() {
    let (_dir, root) = solution_dir();
    for name in ["P.Site", "P.Line"] {
        let created = call(
            &root,
            "entity_new",
            json!({"kind": "thing", "name": name, "dry_run": false}),
        );
        assert_eq!(created["isError"], false, "{created}");
    }
    let network = call(
        &root,
        "entity_new",
        json!({"kind": "network", "name": "P.Plant", "dry_run": false}),
    );
    assert_eq!(network["isError"], false, "{network}");
    let root_added = call(
        &root,
        "network_write",
        json!({"action": "add", "network": "P.Plant", "thing": "P.Site", "as_root": true, "dry_run": false}),
    );
    assert_eq!(root_added["isError"], false, "{root_added}");
    // A dry run by default: nothing written.
    let planned = call(
        &root,
        "network_write",
        json!({"action": "add", "network": "P.Plant", "thing": "P.Line", "under": "P.Site"}),
    );
    assert_eq!(planned["isError"], false, "{planned}");
    assert_eq!(planned["structuredContent"]["applied"], false);
    let before = call(
        &root,
        "network",
        json!({"action": "show", "network": "P.Plant"}),
    );
    assert_eq!(before["structuredContent"]["nodes"], 1, "{before}");
    let applied = call(
        &root,
        "network_write",
        json!({"action": "add", "network": "P.Plant", "thing": "P.Line", "under": "P.Site", "dry_run": false}),
    );
    assert_eq!(applied["isError"], false, "{applied}");
    let shown = call(
        &root,
        "network",
        json!({"action": "show", "network": "Plant"}),
    );
    let tree = &shown["structuredContent"]["tree"];
    assert_eq!(tree[0]["name"], "P.Site", "{shown}");
    assert_eq!(tree[1]["name"], "P.Line");
    assert_eq!(tree[1]["level"], 1);
    let listed = call(&root, "network", json!({"action": "list"}));
    assert_eq!(listed["isError"], false, "{listed}");
    // A second parent is refused, as the server would.
    let refused = call(
        &root,
        "network_write",
        json!({"action": "add", "network": "P.Plant", "thing": "P.Line", "as_root": true, "dry_run": false}),
    );
    assert_eq!(refused["isError"], true, "{refused}");
}

#[test]
fn requirements_tools_default_to_offline_plans_and_return_blocking_findings() {
    let (_dir, root) = solution_dir();
    let gates = call(&root, "check", json!({}));
    let skipped = gates["structuredContent"]["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|gate| gate["gate"] == "requirements")
        .unwrap();
    assert!(skipped["skipped"]
        .as_str()
        .unwrap()
        .contains("requirements.toml is absent"));
    std::fs::write(root.join("Things/Acme.Ref.xml"), r#"<Entities><Things><Thing name="Acme.Ref" projectName="P" thingTemplate="Acme.External"/></Things></Entities>"#).unwrap();
    let planned = call(&root, "requirements_init", json!({}));
    assert_eq!(planned["isError"], false, "{planned}");
    assert_eq!(planned["structuredContent"]["dry_run"], true);
    assert_eq!(planned["structuredContent"]["written"], false);
    assert!(!root.join("requirements.toml").exists());
    let applied = call(&root, "requirements_init", json!({"dry_run": false}));
    assert_eq!(applied["isError"], false, "{applied}");
    assert_eq!(applied["structuredContent"]["written"], true);
    let checked = call(
        &root,
        "requirements_check",
        json!({"detail":true, "profile":"does-not-exist"}),
    );
    assert_eq!(checked["isError"], false, "{checked}");
    assert_eq!(checked["structuredContent"]["ok"], true);
    assert_eq!(checked["structuredContent"]["server_checked"], false);
    let refused = call(&root, "requirements_init", json!({"dry_run":false}));
    assert_eq!(refused["isError"], true);
    std::fs::write(root.join("requirements.toml"), "").unwrap();
    let checked = call(&root, "requirements_check", json!({"detail":true}));
    assert_eq!(checked["isError"], false, "{checked}");
    assert_eq!(checked["structuredContent"]["ok"], false);
    assert!(checked["structuredContent"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["requirement"] == "ThingTemplates/Acme.External"
            && f["referrers"][0] == "Things/Acme.Ref"));
    assert_eq!(
        call(&root, "requirements_check", json!({"unknown":true}))["isError"],
        true
    );
    let definitions = tool_definitions();
    let check = definitions
        .iter()
        .find(|tool| tool["name"] == "requirements_check")
        .unwrap();
    assert_eq!(check["annotations"]["readOnlyHint"], true);
    assert_eq!(check["annotations"]["openWorldHint"], true);
    let init = definitions
        .iter()
        .find(|tool| tool["name"] == "requirements_init")
        .unwrap();
    assert_eq!(init["annotations"]["destructiveHint"], false);
    assert_eq!(init["annotations"]["openWorldHint"], false);
}

#[test]
fn check_with_detail_gives_each_gate_its_notes() {
    let (_dir, root) = solution_dir();
    std::fs::write(root.join("requirements.toml"), "").unwrap();
    let notes = |detail: bool| {
        let result = call(&root, "check", json!({ "detail": detail }));
        result["structuredContent"]["gates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|gate| gate["gate"] == "requirements")
            .unwrap()["notes"]
            .clone()
    };
    assert!(notes(true)[0]
        .as_str()
        .is_some_and(|note| note.starts_with("requirements:")));
    assert!(notes(false).is_null(), "a summary leaves the notes out");
}
