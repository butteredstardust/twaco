//! The recipe options reach the same planned/applied content through CLI and MCP, offline.

use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

fn invoke(root: &Path, mcp: bool, cli: &[&str], tool: &str, arguments: Value) -> Value {
    if mcp {
        let input = format!(
            "{}\n{}\n",
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":arguments}})
        );
        let mut output = Vec::new();
        twaco::mcp::serve_tools(
            root,
            &twaco::mcp::Selection::ALL,
            input.as_bytes(),
            &mut output,
        )
        .unwrap();
        let response: Value = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|response| response["id"] == 1)
            .unwrap();
        assert_eq!(response["result"]["isError"], false, "{response}");
        response["result"]["structuredContent"].clone()
    } else {
        let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .env_clear()
            .args(cli)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{cli:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

fn properties<'a>(widget: &'a Value, type_id: &str) -> Option<&'a Value> {
    if widget["Properties"]["Type"] == type_id {
        return Some(&widget["Properties"]);
    }
    widget["Widgets"]
        .as_array()?
        .iter()
        .find_map(|child| properties(child, type_id))
}

#[test]
fn recipe_options_plan_and_apply_through_cli_and_mcp() {
    for mcp in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(root.join("DataShapes")).unwrap();
        std::fs::write(root.join("Things/Acme.Manager.xml"), r#"<Entities><Things><Thing name="Acme.Manager" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetSteps"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Steps_DS"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#).unwrap();
        std::fs::write(root.join("DataShapes/Acme.Steps_DS.xml"), r#"<Entities><DataShapes><DataShape name="Acme.Steps_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="number" baseType="INTEGER"/><FieldDefinition name="caption" baseType="STRING"/><FieldDefinition name="state" baseType="STRING"/><FieldDefinition name="interactive" baseType="BOOLEAN"/><FieldDefinition name="mashup" baseType="MASHUPNAME"/><FieldDefinition name="start" baseType="DATETIME"/><FieldDefinition name="description" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#).unwrap();
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "new",
                "Acme.Steps_MU",
                "--param",
                "CurrentStep:NUMBER",
                "--apply",
            ],
            "mashup_new",
            json!({"name":"Acme.Steps_MU", "params":["CurrentStep:NUMBER"], "dry_run":false}),
        );
        let button = invoke(
            root,
            mcp,
            &["mashup", "add", "Acme.Steps_MU", "button", "--apply"],
            "mashup_add",
            json!({"mashup":"Acme.Steps_MU", "widget":"button", "dry_run":false}),
        );
        let button = button["id"].as_str().unwrap();
        let grid = invoke(
            root,
            mcp,
            &["mashup", "add", "Acme.Steps_MU", "grid", "--apply"],
            "mashup_add",
            json!({"mashup":"Acme.Steps_MU", "widget":"grid", "dry_run":false}),
        );
        let grid = grid["id"].as_str().unwrap();
        for (name, widget, flags, options) in [
            (
                "confirm",
                button,
                vec![
                    "--then",
                    "mashup:Refresh",
                    "--action-label",
                    "Delete",
                    "--cancel-label",
                    "Keep",
                ],
                json!({"then":"mashup:Refresh", "action_label":"Delete", "cancel_label":"Keep"}),
            ),
            (
                "columns",
                grid,
                vec![
                    "--data",
                    "data:Acme.Manager.GetSteps",
                    "--column",
                    "start:Started:180",
                    "--column",
                    "caption:Step",
                ],
                json!({"data":"data:Acme.Manager.GetSteps", "columns":["start:Started:180", "caption:Step"]}),
            ),
            (
                "tracker",
                "new",
                vec![
                    "--data",
                    "data:Acme.Manager.GetSteps",
                    "--field-map",
                    "StepLabelField=caption",
                ],
                json!({"data":"data:Acme.Manager.GetSteps", "field_maps":["StepLabelField=caption"]}),
            ),
            (
                "set-on-click",
                button,
                vec![
                    "--param",
                    "CurrentStep",
                    "--expr",
                    "CurrentStep + 1",
                    "--input",
                    "CurrentStep=mashup:CurrentStep",
                ],
                json!({"param":"CurrentStep", "expression":"CurrentStep + 1", "inputs":["CurrentStep=mashup:CurrentStep"]}),
            ),
        ] {
            let mut cli = vec!["mashup", "recipe", name, "Acme.Steps_MU", widget];
            cli.extend(flags);
            let mut arguments = json!({"mashup":"Acme.Steps_MU", "recipe":name, "widget":widget});
            arguments
                .as_object_mut()
                .unwrap()
                .extend(options.as_object().unwrap().clone());
            let entity = root.join("Mashups/Acme.Steps_MU.xml");
            let sidecar = root.join("src/Acme.Steps_MU/mashup/content.json");
            let before = (
                std::fs::read(&entity).unwrap(),
                std::fs::read(&sidecar).unwrap(),
            );
            let plan = invoke(root, mcp, &cli, "mashup_recipe", arguments.clone());
            assert_eq!(plan["applied"], false);
            assert_eq!(std::fs::read(&entity).unwrap(), before.0);
            assert_eq!(std::fs::read(&sidecar).unwrap(), before.1);
            cli.push("--apply");
            arguments["dry_run"] = json!(false);
            let applied = invoke(root, mcp, &cli, "mashup_recipe", arguments);
            assert_eq!(applied["applied"], true);
            let content: Value = serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
            match name {
                "confirm" => {
                    let props = properties(&content["UI"], "ptcsconfirmation").unwrap();
                    assert_eq!(props["ActionButtonLabel"], "Delete");
                    assert_eq!(props["CancelButtonLabel"], "Keep");
                }
                "columns" => {
                    let columns = properties(&content["UI"], "ptcsgrid").unwrap()["ColumnFormat"]
                        ["formatInfo"]
                        .as_array()
                        .unwrap();
                    assert_eq!(columns.len(), 7);
                    assert_eq!(columns[0]["FieldName"], "start");
                    assert_eq!(columns[0]["FormatOptions"]["renderer"], "DATETIME");
                    assert_eq!(
                        columns[0]["FormatOptions"]["FormatString"],
                        "[[dateTimeFormat_Default]]"
                    );
                    assert!(columns[2..]
                        .iter()
                        .all(|column| column["inLayout"] == false && column["hidden"] == true));
                }
                "tracker" => {
                    let props = properties(&content["UI"], "ptcsprogresstracker").unwrap();
                    assert_eq!(props["IsInteractive"], true);
                    assert_eq!(props["StepSize"], "large");
                    assert_eq!(props["StepLabelField"], "caption");
                    assert_eq!(props["StepMashupNameField"], "mashup");
                }
                "set-on-click" => {
                    let props = properties(&content["UI"], "expression2").unwrap();
                    assert_eq!(props["AutoEvaluate"], false);
                    assert_eq!(props["DisableInitialOutput"], true);
                    assert_eq!(props["OutputBaseType"], "NUMBER");
                    assert_eq!(props["Expression"], "Output = (CurrentStep + 1);");
                    assert_eq!(
                        props["_currentParameterDefs"][0]["ParameterName"],
                        "CurrentStep"
                    );
                }
                _ => unreachable!(),
            }
        }
        if !mcp {
            let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
                .current_dir(root)
                .env_clear()
                .args([
                    "mashup",
                    "recipe",
                    "set-on-click",
                    "Acme.Steps_MU",
                    button,
                    "--param",
                    "CurrentStep",
                    "--param",
                    "OtherStep",
                    "--expr",
                    "1",
                    "--apply",
                ])
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("give --param once"));
        }
    }
}
