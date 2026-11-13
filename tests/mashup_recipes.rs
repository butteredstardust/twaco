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

fn leaf_ids(widget: &Value) -> Vec<String> {
    if !matches!(
        widget["Properties"]["Type"].as_str(),
        Some("flexcontainer" | "mashup")
    ) {
        return vec![widget["Properties"]["Id"].as_str().unwrap().to_string()];
    }
    widget["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(leaf_ids)
        .collect()
}

fn find_widget<'a>(widget: &'a Value, id: &str) -> Option<&'a Value> {
    if widget["Properties"]["Id"] == id {
        return Some(widget);
    }
    widget["Widgets"]
        .as_array()?
        .iter()
        .find_map(|child| find_widget(child, id))
}

#[test]
fn two_column_form_stacks_in_each_column_through_cli_and_mcp() {
    for mcp in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
        invoke(
            root,
            mcp,
            &["mashup", "new", "Acme.Form_MU", "--apply"],
            "mashup_new",
            json!({"name":"Acme.Form_MU", "dry_run":false}),
        );
        let add = |kind: &str, label: &str, placement: Option<(&str, &str)>, in_column: bool| {
            let mut cli = vec![
                "mashup",
                "add",
                "Acme.Form_MU",
                kind,
                "--label",
                label,
                "--apply",
            ];
            let mut arguments =
                json!({"mashup":"Acme.Form_MU", "widget":kind, "label":label, "dry_run":false});
            if let Some((flag, anchor)) = placement {
                cli.extend([flag, anchor]);
                arguments[flag.trim_start_matches('-').replace('-', "_")] = json!(anchor);
            }
            if in_column {
                cli.push("--in-column");
                arguments["in_column"] = json!(true);
            }
            invoke(root, mcp, &cli, "mashup_add", arguments)["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let heading = add("label", "Form", None, false);
        let left = add("textfield", "Field 01", Some(("--below", &heading)), false);
        let right = add("textfield", "Field 02", Some(("--right-of", &left)), false);
        let entity = root.join("Mashups/Acme.Form_MU.xml");
        let sidecar = root.join("src/Acme.Form_MU/mashup/content.json");
        let before = (
            std::fs::read(&entity).unwrap(),
            std::fs::read(&sidecar).unwrap(),
        );
        let planned = invoke(
            root,
            mcp,
            &[
                "mashup",
                "add",
                "Acme.Form_MU",
                "textarea",
                "--below",
                &left,
                "--in-column",
            ],
            "mashup_add",
            json!({"mashup":"Acme.Form_MU", "widget":"textarea", "below":left, "in_column":true}),
        );
        assert_eq!(planned["applied"], false);
        assert_eq!(std::fs::read(&entity).unwrap(), before.0);
        assert_eq!(std::fs::read(&sidecar).unwrap(), before.1);
        let description = add("textarea", "Description", Some(("--below", &left)), true);
        let first_date = add("datepicker", "Date", Some(("--below", &right)), true);
        let second_date = add("datepicker", "Date", Some(("--below", &first_date)), true);
        let checkbox = add(
            "checkbox",
            "Checkbox",
            Some(("--below", &second_date)),
            true,
        );
        let path = root.join("src/Acme.Form_MU/mashup/content.json");
        let content: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let center = find_widget(&content["UI"], "flexcontainer-2").unwrap();
        // The form keeps one split row. Each column contains its own rows in order,
        // including after repeated insertions beneath the dates.
        assert_eq!(center["Widgets"].as_array().unwrap().len(), 2, "{center}");
        let form = &center["Widgets"][1];
        assert_eq!(form["Properties"]["flex-shrink"], 0);
        assert_eq!(form["Properties"]["flex-direction"], "row", "{form}");
        let columns = form["Widgets"].as_array().unwrap();
        assert_eq!(columns.len(), 2, "{center}");
        assert_eq!(columns[0]["Widgets"].as_array().unwrap().len(), 2);
        assert_eq!(columns[1]["Widgets"].as_array().unwrap().len(), 4);
        assert_eq!(columns[0]["Properties"]["flex-min-height"], "178px");
        assert_eq!(leaf_ids(&columns[0]), [left.clone(), description.clone()]);
        assert_eq!(
            leaf_ids(&columns[1]),
            [right.clone(), first_date, second_date, checkbox]
        );
        for column in columns {
            assert_eq!(column["Properties"]["flex-direction"], "column");
            for row in column["Widgets"].as_array().unwrap() {
                assert_eq!(row["Properties"]["flex-direction"], "row");
                assert_eq!(leaf_ids(row).len(), 1);
                if row["Properties"]["flex-size"] == "fixed" {
                    assert_eq!(
                        row["Properties"]["flex-min-height"],
                        row["Properties"]["flex-max-height"]
                    );
                } else {
                    assert_eq!(row["Properties"]["flex-shrink"], 0, "{row}");
                }
            }
        }
        let textarea_row = columns[0]["Widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| leaf_ids(row).contains(&description))
            .unwrap();
        assert_eq!(textarea_row["Properties"]["flex-min-height"], "100px");
        assert_eq!(textarea_row["Properties"]["flex-size"], "range");
        assert!(
            find_widget(textarea_row, &description).unwrap()["Properties"]["ResponsiveLayout"]
                == true
        );

        // Default --below still adds a row beneath an entire multi-control row.
        invoke(
            root,
            mcp,
            &["mashup", "new", "Acme.Default_MU", "--apply"],
            "mashup_new",
            json!({"name":"Acme.Default_MU", "dry_run":false}),
        );
        let mut anchor = "center".to_string();
        let mut ids = Vec::new();
        for (kind, flag) in [
            ("label", "--in"),
            ("textfield", "--below"),
            ("textfield", "--right-of"),
            ("datepicker", "--below"),
        ] {
            let mut arguments = json!({"mashup":"Acme.Default_MU", "widget":kind, "dry_run":false});
            arguments[flag.trim_start_matches('-').replace('-', "_")] = json!(anchor);
            let result = invoke(
                root,
                mcp,
                &[
                    "mashup",
                    "add",
                    "Acme.Default_MU",
                    kind,
                    flag,
                    &anchor,
                    "--apply",
                ],
                "mashup_add",
                arguments,
            );
            anchor = result["id"].as_str().unwrap().to_string();
            ids.push(anchor.clone());
        }
        let content: Value = serde_json::from_slice(
            &std::fs::read(root.join("src/Acme.Default_MU/mashup/content.json")).unwrap(),
        )
        .unwrap();
        let center = find_widget(&content["UI"], "flexcontainer-2").unwrap();
        assert_eq!(center["Properties"]["flex-direction"], "column");
        assert_eq!(center["Widgets"].as_array().unwrap().len(), 3);
        assert_eq!(
            leaf_ids(&center["Widgets"][1]),
            [ids[1].clone(), ids[2].clone()]
        );
        assert_eq!(leaf_ids(&center["Widgets"][2]), [ids[3].clone()]);
    }
}

#[test]
fn recipe_retries_match_wiring_and_preserve_different_arguments() {
    for mcp in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(root.join("Things/Acme.Manager.xml"), r#"<Entities><Things><Thing name="Acme.Manager" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="Save"><ResultType baseType="NOTHING"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#).unwrap();
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "new",
                "Acme.Retry_MU",
                "--param",
                "Step:NUMBER",
                "--param",
                "Other:NUMBER",
                "--param",
                "Note:STRING",
                "--param",
                "OtherNote:STRING",
                "--apply",
            ],
            "mashup_new",
            json!({"name":"Acme.Retry_MU", "params":["Step:NUMBER", "Other:NUMBER", "Note:STRING", "OtherNote:STRING"], "dry_run":false}),
        );
        let mut buttons = Vec::new();
        for _ in 0..2 {
            let result = invoke(
                root,
                mcp,
                &["mashup", "add", "Acme.Retry_MU", "button", "--apply"],
                "mashup_add",
                json!({"mashup":"Acme.Retry_MU", "widget":"button", "dry_run":false}),
            );
            buttons.push(result["id"].as_str().unwrap().to_string());
        }
        for alias in ["save", "other"] {
            invoke(
                root,
                mcp,
                &[
                    "mashup",
                    "source",
                    "add",
                    "Acme.Retry_MU",
                    "Acme.Manager",
                    "Save",
                    "--as",
                    alias,
                    "--apply",
                ],
                "mashup_source_add",
                json!({"mashup":"Acme.Retry_MU", "entity":"Acme.Manager", "service":"Save", "alias":alias, "dry_run":false}),
            );
        }
        let entity = root.join("Mashups/Acme.Retry_MU.xml");
        let sidecar = root.join("src/Acme.Retry_MU/mashup/content.json");
        let bytes = || {
            (
                std::fs::read(&entity).unwrap(),
                std::fs::read(&sidecar).unwrap(),
            )
        };
        // Each request differs in exactly one part of the wiring contract. Each repeat must
        // also find its own expression after other, similar expressions have been added.
        for (button, parameter, expression, input) in [
            (&buttons[0], "Step", "value + 1", "value=mashup:Step"),
            (&buttons[1], "Step", "value + 1", "value=mashup:Step"),
            (&buttons[0], "Other", "value + 1", "value=mashup:Step"),
            (&buttons[0], "Step", "value - 1", "value=mashup:Step"),
            (&buttons[0], "Step", "value + 1", "value=mashup:Other"),
        ] {
            let cli = [
                "mashup",
                "recipe",
                "set-on-click",
                "Acme.Retry_MU",
                button,
                "--param",
                parameter,
                "--expr",
                expression,
                "--input",
                input,
                "--apply",
            ];
            let arguments = json!({"mashup":"Acme.Retry_MU", "recipe":"set-on-click", "widget":button,
                "param":parameter, "expression":expression, "inputs":[input], "dry_run":false});
            let applied = invoke(root, mcp, &cli, "mashup_recipe", arguments.clone());
            assert_eq!(applied["added"].as_array().unwrap().len(), 1);
            let before = bytes();
            let repeated = invoke(root, mcp, &cli, "mashup_recipe", arguments);
            assert!(repeated["added"].as_array().unwrap().is_empty());
            assert_eq!(bytes(), before);
        }
        // Success and failure messages are matched separately, including named calls of the
        // same service and failure-only recipes.
        for (after, success, failure, expected_added) in [
            (
                "data:Acme.Manager.Save[as save]",
                Some("Saved"),
                "Save failed",
                2,
            ),
            (
                "data:Acme.Manager.Save[as save]",
                Some("Saved"),
                "Try again",
                1,
            ),
            ("data:Acme.Manager.Save[as other]", None, "Save failed", 1),
            ("data:Acme.Manager.Save[as save]", None, "Save failed", 0),
            (
                "data:Acme.Manager.Save[as save]",
                Some("Done"),
                "Save failed",
                1,
            ),
        ] {
            let mut cli = vec![
                "mashup",
                "recipe",
                "message",
                "Acme.Retry_MU",
                "--after",
                after,
                "--on-failure",
                failure,
                "--apply",
            ];
            let mut arguments = json!({"mashup":"Acme.Retry_MU", "recipe":"message", "after":after,
                "on_failure":failure, "dry_run":false});
            if let Some(success) = success {
                cli.extend(["--say", success]);
                arguments["say"] = json!(success);
            }
            let applied = invoke(root, mcp, &cli, "mashup_recipe", arguments.clone());
            assert_eq!(applied["added"].as_array().unwrap().len(), expected_added);
            let before = bytes();
            let repeated = invoke(root, mcp, &cli, "mashup_recipe", arguments);
            assert!(repeated["added"].as_array().unwrap().is_empty());
            assert_eq!(bytes(), before);
        }
        for (source, kind) in [
            ("mashup:Note", "info"),
            ("mashup:OtherNote", "info"),
            ("mashup:Note", "warning"),
        ] {
            let cli = [
                "mashup",
                "recipe",
                "message",
                "Acme.Retry_MU",
                "--after",
                "data:Acme.Manager.Save[as save]",
                "--from",
                source,
                "--kind",
                kind,
                "--on-failure",
                "Save failed",
                "--apply",
            ];
            let arguments = json!({"mashup":"Acme.Retry_MU", "recipe":"message", "after":"data:Acme.Manager.Save[as save]",
                "from":source, "kind":kind, "on_failure":"Save failed", "dry_run":false});
            let applied = invoke(root, mcp, &cli, "mashup_recipe", arguments.clone());
            assert_eq!(applied["added"].as_array().unwrap().len(), 1);
            let before = bytes();
            let repeated = invoke(root, mcp, &cli, "mashup_recipe", arguments);
            assert!(repeated["added"].as_array().unwrap().is_empty());
            assert_eq!(bytes(), before);
        }
        // A dynamic message's saved default text is not a static message with that text.
        let cli = [
            "mashup",
            "recipe",
            "message",
            "Acme.Retry_MU",
            "--after",
            "data:Acme.Manager.Save[as save]",
            "--say",
            "",
            "--apply",
        ];
        let arguments = json!({"mashup":"Acme.Retry_MU", "recipe":"message", "after":"data:Acme.Manager.Save[as save]",
            "say":"", "dry_run":false});
        assert_eq!(
            invoke(root, mcp, &cli, "mashup_recipe", arguments.clone())["added"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let before = bytes();
        assert!(invoke(root, mcp, &cli, "mashup_recipe", arguments)["added"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(bytes(), before);
    }
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
            let applied = invoke(root, mcp, &cli, "mashup_recipe", arguments.clone());
            assert_eq!(applied["applied"], true);
            if name == "set-on-click" {
                let before = (
                    std::fs::read(&entity).unwrap(),
                    std::fs::read(&sidecar).unwrap(),
                );
                let repeated = invoke(root, mcp, &cli, "mashup_recipe", arguments);
                assert!(repeated["added"].as_array().unwrap().is_empty());
                assert_eq!(std::fs::read(&entity).unwrap(), before.0);
                assert_eq!(std::fs::read(&sidecar).unwrap(), before.1);
            }
            let content: Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
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
