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
fn runtime_reported_filter_and_blank_form_builds_fit_through_cli_and_mcp() {
    for mcp in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
        for (name, template) in [
            ("Acme.Filters_MU", "filter-panel"),
            ("Acme.Form_MU", "blank"),
        ] {
            invoke(
                root,
                mcp,
                &["mashup", "new", name, "--template", template, "--apply"],
                "mashup_new",
                json!({"name":name, "template":template, "dry_run":false}),
            );
        }
        let add = |name: &str,
                   kind: &str,
                   flag: &str,
                   anchor: &str,
                   label: Option<&str>,
                   extra: Option<&str>| {
            let mut cli = vec!["mashup", "add", name, kind, flag, anchor, "--apply"];
            let mut args = json!({"mashup":name, "widget":kind, "dry_run":false});
            args[flag.trim_start_matches('-').replace('-', "_")] = json!(anchor);
            if let Some(label) = label {
                cli.extend(["--label", label]);
                args["label"] = json!(label);
            }
            if let Some(extra) = extra {
                cli.push(extra);
                args[extra.trim_start_matches('-').replace('-', "_")] = json!(true);
            }
            invoke(root, mcp, &cli, "mashup_add", args)["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let filters = "Acme.Filters_MU";
        let search = add(
            filters,
            "text-field",
            "--in",
            "filters",
            Some("Search"),
            None,
        );
        let date = add(filters, "date-picker", "--in", "filters", None, None);
        let category = add(
            filters,
            "dropdown",
            "--in",
            "filters",
            Some("Category"),
            None,
        );
        let apply = add(
            filters,
            "button",
            "--in",
            "filters",
            Some("Apply"),
            Some("--bottom"),
        );
        let clear = add(filters, "button", "--right-of", &apply, Some("Clear"), None);
        add(filters, "grid", "--in", "content", None, None);
        let read = |name: &str| -> Value {
            serde_json::from_slice(
                &std::fs::read(root.join(format!("src/{name}/mashup/content.json"))).unwrap(),
            )
            .unwrap()
        };
        let content = read(filters);
        let slot = find_widget(&content["UI"], "flexcontainer-4").unwrap();
        assert_eq!(slot["Properties"]["flex-max-width"], "240px");
        let rows = slot["Widgets"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        for (index, (id, height)) in [(search, "78px"), (date, "64px"), (category, "86px")]
            .iter()
            .enumerate()
        {
            let row = &rows[index];
            let control = find_widget(row, id).unwrap();
            assert_eq!(control["Properties"]["Width"].as_f64(), Some(240.0));
            assert_eq!(control["Properties"]["ResponsiveLayout"], false);
            assert_eq!(
                control["Properties"]["Margin"],
                if index == 0 { "5 0" } else { "13 0 5" }
            );
            assert_eq!(row["Properties"]["flex-size"], "fixed");
            assert_eq!(row["Properties"]["flex-min-height"], *height);
            assert_eq!(row["Properties"]["flex-max-height"], *height);
            assert_eq!(row["Properties"]["flex-shrink"], 0);
            assert_eq!(row["Properties"]["justify-content"], "center");
        }
        let footer = &rows[3];
        assert_eq!(leaf_ids(footer), [apply, clear]);
        assert_eq!(footer["Properties"]["flex-grow"], 1);
        assert_eq!(footer["Properties"]["align-items"], "flex-end");
        assert_eq!(footer["Properties"]["Margin"], "0 5");
        for button in footer["Widgets"].as_array().unwrap() {
            assert_eq!(button["Properties"]["Width"], 110);
            assert_eq!(button["Properties"]["Margin"], "0");
        }
        let form = "Acme.Form_MU";
        let left = add(form, "text-field", "--in", "center", Some("Field 01"), None);
        let right = add(
            form,
            "text-field",
            "--right-of",
            &left,
            Some("Field 02"),
            None,
        );
        let textarea = add(
            form,
            "text-area",
            "--below",
            &left,
            Some("Description"),
            Some("--in-column"),
        );
        let date = add(
            form,
            "date-picker",
            "--below",
            &right,
            None,
            Some("--in-column"),
        );
        let enabled = add(
            form,
            "checkbox",
            "--below",
            &date,
            Some("Enabled"),
            Some("--in-column"),
        );
        let save = add(form, "button", "--below", &textarea, Some("Save"), None);
        let content = read(form);
        let center = find_widget(&content["UI"], "flexcontainer-2").unwrap();
        assert_eq!(center["Properties"]["flex-direction"], "column");
        let rows = center["Widgets"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["Properties"]["flex-grow"], 0);
        assert_eq!(rows[0]["Properties"]["flex-shrink"], 0);
        assert_eq!(rows[0]["Properties"]["flex-min-height"], "192px");
        let columns = rows[0]["Widgets"].as_array().unwrap();
        assert_eq!(leaf_ids(&columns[0]), [left, textarea]);
        assert_eq!(leaf_ids(&columns[1]), [right, date, enabled]);
        assert_eq!(columns[0]["Properties"]["flex-min-height"], "178px");
        assert_eq!(columns[0]["Properties"]["flex-max-height"], "178px");
        for column in columns {
            assert_eq!(column["Properties"]["flex-direction"], "column");
            assert_eq!(column["Properties"]["justify-content"], "flex-start");
            assert_eq!(column["Properties"]["align-items"], "flex-start");
            assert_eq!(column["Properties"]["flex-grow"], 1);
            for row in column["Widgets"].as_array().unwrap() {
                assert_eq!(row["Properties"]["flex-direction"], "row");
                assert_eq!(row["Properties"]["align-items"], "flex-start");
                assert_eq!(row["Properties"]["justify-content"], "flex-start");
            }
        }
        let textarea_row = &columns[0]["Widgets"][1]["Properties"];
        assert_eq!(textarea_row["flex-min-height"], "100px");
        assert_eq!(textarea_row["flex-grow"], 0);
        assert_eq!(textarea_row["flex-shrink"], 0);
        assert_eq!(leaf_ids(&rows[1]), [save]);
        assert_eq!(rows[1]["Properties"]["flex-grow"], 1);
        assert_eq!(rows[1]["Properties"]["flex-shrink"], 0);
        assert_eq!(rows[1]["Properties"]["align-items"], "flex-start");
    }
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
        assert_eq!(form["Properties"]["flex-grow"], 0);
        assert_eq!(form["Properties"]["flex-size"], "range");
        // The right stack is 3 x 78px + 46px; its wrapper keeps the existing 12px allowance.
        assert_eq!(form["Properties"]["flex-min-height"], "292px");
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
        assert_eq!(textarea_row["Properties"]["flex-grow"], 0);
        assert!(
            find_widget(textarea_row, &description).unwrap()["Properties"]["ResponsiveLayout"]
                == true
        );

        // Save sits immediately after the form; its row absorbs spare height below the button.
        let form_id = form["Properties"]["Id"].as_str().unwrap();
        let save = add("button", "Save", Some(("--below", form_id)), false);
        let content: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let center = find_widget(&content["UI"], "flexcontainer-2").unwrap();
        let rows = center["Widgets"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["Properties"]["Id"], form_id);
        assert_eq!(rows[1]["Properties"]["flex-grow"], 0);
        assert_eq!(rows[1]["Properties"]["flex-shrink"], 0);
        assert_eq!(rows[1]["Properties"]["flex-min-height"], "292px");
        assert_eq!(leaf_ids(&rows[2]), [save]);
        assert_eq!(rows[2]["Properties"]["flex-size"], "default");
        assert_eq!(rows[2]["Properties"]["flex-grow"], 1);
        assert_eq!(rows[2]["Properties"]["align-items"], "flex-start");

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
        let css = root.join("src/Acme.Retry_MU/mashup/custom.css");
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
            assert_eq!(applied["unchanged"], false);
            if mcp {
                let content = std::fs::read(&sidecar).unwrap();
                if !content.starts_with(b"\xEF\xBB\xBF") {
                    let mut marked = b"\xEF\xBB\xBF".to_vec();
                    marked.extend(content);
                    std::fs::write(&sidecar, marked).unwrap();
                }
            }
            let modified =
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(123456);
            for path in [&entity, &sidecar, &css] {
                std::fs::File::options()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_times(std::fs::FileTimes::new().set_modified(modified))
                    .unwrap();
            }
            let before = bytes();
            let mut planned_arguments = arguments.clone();
            planned_arguments["dry_run"] = json!(true);
            let planned = invoke(
                root,
                mcp,
                &cli[..cli.len() - 1],
                "mashup_recipe",
                planned_arguments,
            );
            assert_eq!(planned["unchanged"], true);
            assert_eq!(planned["applied"], false);
            let repeated = invoke(root, mcp, &cli, "mashup_recipe", arguments);
            assert_eq!(repeated["unchanged"], true);
            assert_eq!(repeated["applied"], false);
            assert!(repeated["added"].as_array().unwrap().is_empty());
            assert_eq!(bytes(), before);
            for path in [&entity, &sidecar, &css] {
                assert_eq!(
                    std::fs::metadata(path).unwrap().modified().unwrap(),
                    modified
                );
            }
            if !mcp {
                for args in [&cli[..], &cli[..cli.len() - 1]] {
                    let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
                        .current_dir(root)
                        .env_clear()
                        .args(args)
                        .output()
                        .unwrap();
                    assert!(output.status.success(), "{output:?}");
                    assert_eq!(
                        String::from_utf8(output.stdout).unwrap().trim(),
                        "recipe set-on-click already in Acme.Retry_MU; nothing to write"
                    );
                }
            }
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
fn added_tracker_omits_current_mashup_and_refuses_setting_the_output() {
    for mcp in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
        invoke(
            root,
            mcp,
            &["mashup", "new", "Acme.Steps_MU", "--apply"],
            "mashup_new",
            json!({"name":"Acme.Steps_MU", "dry_run":false}),
        );
        let added = invoke(
            root,
            mcp,
            &[
                "mashup",
                "add",
                "Acme.Steps_MU",
                "progress-tracker",
                "--apply",
            ],
            "mashup_add",
            json!({"mashup":"Acme.Steps_MU", "widget":"progress-tracker", "dry_run":false}),
        );
        let sidecar = root.join("src/Acme.Steps_MU/mashup/content.json");
        let before = std::fs::read(&sidecar).unwrap();
        let content: Value = serde_json::from_slice(&before).unwrap();
        assert!(properties(&content["UI"], "ptcsprogresstracker")
            .unwrap()
            .get("CurrentStepMashup")
            .is_none());
        let entity = root.join("Mashups/Acme.Steps_MU.xml");
        let entity_before = std::fs::read(&entity).unwrap();
        let refused = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .env_clear()
            .args([
                "mashup",
                "set",
                "Acme.Steps_MU",
                added["id"].as_str().unwrap(),
                "--prop",
                "CurrentStepMashup=Acme.Step_MU",
                "--apply",
            ])
            .output()
            .unwrap();
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("CurrentStepMashup is not editable")
        );
        assert_eq!(std::fs::read(&sidecar).unwrap(), before);
        assert_eq!(std::fs::read(&entity).unwrap(), entity_before);
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
        std::fs::write(root.join("DataShapes/Acme.Steps_DS.xml"), r#"<Entities><DataShapes><DataShape name="Acme.Steps_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="number" baseType="INTEGER"/><FieldDefinition name="caption" baseType="STRING"/><FieldDefinition name="state" baseType="STRING"/><FieldDefinition name="interactive" baseType="BOOLEAN"/><FieldDefinition name="mashup" baseType="MASHUPNAME"/><FieldDefinition name="start" baseType="DATETIME"/><FieldDefinition name="description" baseType="TEXT"/><FieldDefinition name="Notes" baseType="TEXT"/></FieldDefinitions></DataShape></DataShapes></Entities>"#).unwrap();
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
                    "--column",
                    "Notes",
                    "--editable-column",
                    "Notes",
                    "--column-editor",
                    "Notes=textarea",
                    "--preserve-whitespace",
                    "Notes",
                ],
                json!({"data":"data:Acme.Manager.GetSteps", "columns":["start:Started:180", "caption:Step", "Notes"], "editable_columns":["Notes"], "column_editors":["Notes=textarea"], "preserve_whitespace":["Notes"]}),
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
            if name == "columns" {
                let mut this_plan = plan.clone();
                let mut other_plan = invoke(root, !mcp, &cli, "mashup_recipe", arguments.clone());
                for result in [&mut this_plan, &mut other_plan] {
                    for envelope in ["dry_run", "notices", "duration_ms"] {
                        result.as_object_mut().unwrap().remove(envelope);
                    }
                }
                assert_eq!(this_plan, other_plan);
            }
            assert_eq!(std::fs::read(&entity).unwrap(), before.0);
            assert_eq!(std::fs::read(&sidecar).unwrap(), before.1);
            cli.push("--apply");
            arguments["dry_run"] = json!(false);
            let applied = invoke(root, mcp, &cli, "mashup_recipe", arguments.clone());
            assert_eq!(applied["applied"], true);
            if matches!(name, "set-on-click" | "columns") {
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
                    assert_eq!(columns.len(), 8);
                    assert_eq!(columns[0]["FieldName"], "start");
                    assert_eq!(columns[0]["FormatOptions"]["renderer"], "DATETIME");
                    assert_eq!(
                        columns[0]["FormatOptions"]["FormatString"],
                        "[[dateTimeFormat_Default]]"
                    );
                    assert_eq!(columns[2]["FieldName"], "Notes");
                    assert_eq!(columns[2]["cellEditor"], "textarea");
                    assert_eq!(columns[2]["preserveWhiteSpaces"], true);
                    assert_eq!(columns[2]["editable"], true);
                    let default_text = columns
                        .iter()
                        .find(|c| c["FieldName"] == "description")
                        .unwrap();
                    assert_eq!(default_text["cellEditor"], "textarea");
                    assert_eq!(default_text["preserveWhiteSpaces"], false);
                    assert!(columns[3..]
                        .iter()
                        .all(|column| column["inLayout"] == false && column["hidden"] == true));
                }
                "tracker" => {
                    let props = properties(&content["UI"], "ptcsprogresstracker").unwrap();
                    assert!(props.get("CurrentStepMashup").is_none());
                    assert_eq!(props["CurrentStepNumber"], 1);
                    assert_eq!(props["IsInteractive"], true);
                    assert_eq!(props["StepSize"], "large");
                    assert_eq!(props["StepLabelField"], "caption");
                    assert_eq!(props["StepMashupNameField"], "mashup");
                    let id = props["Id"].as_str().unwrap();
                    let stored = std::fs::read_to_string(&sidecar).unwrap().replace(
                        "\"Type\": \"ptcsprogresstracker\"",
                        "\"CurrentStepMashup\": \"\", \"Type\": \"ptcsprogresstracker\"",
                    );
                    std::fs::write(&sidecar, &stored).unwrap();
                    let synced = twaco::core::mashup::sync(
                        &std::fs::read(&entity).unwrap(),
                        &twaco::core::mashup::Assets {
                            content: stored.clone(),
                            css: std::fs::read_to_string(sidecar.with_file_name("custom.css"))
                                .unwrap(),
                        },
                    )
                    .unwrap()
                    .0;
                    std::fs::write(&entity, synced).unwrap();
                    cli[4] = id;
                    let mut repair_args = json!({"mashup":"Acme.Steps_MU", "recipe":"tracker", "widget":id,
                        "data":"data:Acme.Manager.GetSteps", "field_maps":["StepLabelField=caption"]});
                    let plan = invoke(
                        root,
                        mcp,
                        &cli[..cli.len() - 1],
                        "mashup_recipe",
                        repair_args.clone(),
                    );
                    assert_eq!(plan["unchanged"], false);
                    let text_plan = Command::new(env!("CARGO_BIN_EXE_twaco"))
                        .current_dir(root)
                        .env_clear()
                        .args(&cli[..cli.len() - 1])
                        .output()
                        .unwrap();
                    assert!(text_plan.status.success());
                    assert!(String::from_utf8_lossy(&text_plan.stderr).contains(
                        "removed stored CurrentStepMashup output; initialise selection through CurrentStepNumber"
                    ));
                    assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), stored);
                    repair_args["dry_run"] = json!(false);
                    invoke(root, mcp, &cli, "mashup_recipe", repair_args.clone());
                    let repaired = std::fs::read(&sidecar).unwrap();
                    let repaired_content: Value = serde_json::from_slice(&repaired).unwrap();
                    let repaired_props =
                        properties(&repaired_content["UI"], "ptcsprogresstracker").unwrap();
                    assert!(repaired_props.get("CurrentStepMashup").is_none());
                    assert_eq!(repaired_props["CurrentStepNumber"], 1);
                    let entity_before = std::fs::read(&entity).unwrap();
                    let repeated = invoke(root, mcp, &cli, "mashup_recipe", repair_args);
                    assert_eq!(repeated["unchanged"], true);
                    assert_eq!(std::fs::read(&sidecar).unwrap(), repaired);
                    assert_eq!(std::fs::read(&entity).unwrap(), entity_before);
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

fn layout_fixture(root: &Path, slider: bool, edit: impl FnOnce(&mut Value)) {
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
    invoke(
        root,
        false,
        &["mashup", "new", "Acme.Layout_MU", "--apply"],
        "",
        json!({}),
    );
    let content_path = root.join("src/Acme.Layout_MU/mashup/content.json");
    let mut content: Value =
        serde_json::from_slice(&std::fs::read(&content_path).unwrap()).unwrap();
    let container = |id: &str, direction: &str, widgets: Value| {
        json!({"Properties": {
        "Id": id, "Type":"flexcontainer", "DisplayName":id, "ResponsiveLayout":true,
        "positioning":"responsive", "Style":"DefaultContainerStyle", "flex-direction":direction,
        "flex-min-width":"", "flex-max-width":"", "Margin":"6 12 0", "CustomClass":"existing",
        "flex-size":"fixed", "flex-min-height":"48px", "flex-max-height":"48px"
    }, "Widgets": widgets})
    };
    content["UI"]["Widgets"] = if slider {
        json!([container(
            "flexcontainer-2",
            "row",
            json!([
                {"Properties":{"Id":"ptcsslider-3", "Type":"ptcsslider", "DisplayName":"Slider", "Margin":"5", "Width":200, "Height":113, "Value":7, "Label":"Range", "VerticalSlider":false, "ResponsiveLayout":false}},
                {"Properties":{"Id":"ptcsbutton-4", "Type":"ptcsbutton", "DisplayName":"Action", "Margin":"5", "Size":"standard", "Label":"Apply", "ButtonType":"primary", "ResponsiveLayout":false}}
            ])
        )])
    } else {
        json!([container(
            "flexcontainer-2",
            "column",
            json!([
                container("flexcontainer-3", "row", json!([])),
                container("flexcontainer-4", "column", json!([]))
            ])
        )])
    };
    content["UI"]["Properties"]["StyleProperties"] = json!({"data":{
        "flexcontainer-4":{"widget":"flex-container", "parts":{"$":{"$":{"background":"#ffffff", "border-width":"1px", "border-style":"solid"}}}}
    }});
    content["DataBindings"] = json!([{"SourceArea":"UI", "SourceId":"ptcsslider-3", "TargetArea":"UI", "TargetId":"ptcsbutton-4", "PropertyMaps":[{"SourceProperty":"Value", "TargetProperty":"Label"}]}]);
    content["Events"] = json!([{"sentinel":"preserve"}]);
    edit(&mut content);
    let css = ".existing { color: red; }\n";
    write_layout(root, &content, css);
}

fn write_layout(root: &Path, content: &Value, css: &str) {
    let entity = root.join("Mashups/Acme.Layout_MU.xml");
    let assets = twaco::core::mashup::Assets {
        content: format!("{}\n", serde_json::to_string_pretty(content).unwrap()),
        css: css.into(),
    };
    let (xml, _) = twaco::core::mashup::sync(&std::fs::read(&entity).unwrap(), &assets).unwrap();
    std::fs::write(entity, xml).unwrap();
    std::fs::write(
        root.join("src/Acme.Layout_MU/mashup/content.json"),
        assets.content,
    )
    .unwrap();
    std::fs::write(root.join("src/Acme.Layout_MU/mashup/custom.css"), css).unwrap();
}

fn layout_bytes(root: &Path) -> Vec<Vec<u8>> {
    [
        "Mashups/Acme.Layout_MU.xml",
        "src/Acme.Layout_MU/mashup/content.json",
        "src/Acme.Layout_MU/mashup/custom.css",
    ]
    .iter()
    .map(|p| std::fs::read(root.join(p)).unwrap())
    .collect()
}

fn layout_result(root: &Path) -> Value {
    let bytes = std::fs::read(root.join("Mashups/Acme.Layout_MU.xml")).unwrap();
    serde_json::from_str(&twaco::core::mashup::extract(&bytes).unwrap().content).unwrap()
}

#[test]
fn panel_css_transaction_reapplication_and_cli_mcp_parity() {
    for xml_only in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        layout_fixture(root, false, |_| {});
        let original = layout_result(root);
        if xml_only {
            std::fs::remove_dir_all(root.join("src/Acme.Layout_MU/mashup")).unwrap();
        }
        for compact in [Some("768"), Some("960"), None] {
            let mut cli = vec![
                "mashup",
                "recipe",
                "panel",
                "Acme.Layout_MU",
                "flexcontainer-2",
            ];
            let mut args =
                json!({"recipe":"panel", "mashup":"Acme.Layout_MU", "widget":"flexcontainer-2"});
            if let Some(n) = compact {
                cli.extend(["--compact", n]);
                args["compact"] = json!(n.parse::<u32>().unwrap());
            }
            let before = std::fs::read(root.join("Mashups/Acme.Layout_MU.xml")).unwrap();
            let planned = invoke(root, false, &cli, "mashup_recipe", args.clone());
            let mut other = invoke(root, true, &cli, "mashup_recipe", args.clone());
            for key in ["dry_run", "notices", "duration_ms"] {
                other.as_object_mut().unwrap().remove(key);
            }
            assert_eq!(planned, other);
            assert_eq!(
                std::fs::read(root.join("Mashups/Acme.Layout_MU.xml")).unwrap(),
                before
            );
            if !xml_only {
                assert!(planned["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|file| file.as_str().unwrap().ends_with("custom.css")));
            }
            cli.push("--apply");
            args["dry_run"] = json!(false);
            invoke(root, true, &cli, "mashup_recipe", args.clone());
            let after = std::fs::read(root.join("Mashups/Acme.Layout_MU.xml")).unwrap();
            let repeated = invoke(root, false, &cli, "mashup_recipe", args);
            assert_eq!(repeated["unchanged"], true);
            assert_eq!(
                std::fs::read(root.join("Mashups/Acme.Layout_MU.xml")).unwrap(),
                after
            );
            let result = layout_result(root);
            let props = &result["UI"]["Widgets"][0]["Properties"];
            assert_eq!(props["CustomClass"], "existing twaco-panel-flexcontainer-2");
            assert_eq!(props["flex-grow"], 1);
            assert_eq!(props["Margin"], "24 24 12");
            assert_eq!(props["flex-max-height"], "");
            assert_eq!(props["flex-size"], "range");
            let styles = &result["UI"]["Properties"]["StyleProperties"]["data"];
            assert_eq!(
                styles["flexcontainer-2"]["parts"]["$"]["$"]["background"],
                "#F7F7F7"
            );
            assert_eq!(
                styles["flexcontainer-3"]["parts"]["$"]["$"]["background"],
                "inherit"
            );
            assert_eq!(
                styles["flexcontainer-4"],
                original["UI"]["Properties"]["StyleProperties"]["data"]["flexcontainer-4"]
            );
            assert_eq!(
                result["UI"]["Widgets"][0]["Widgets"],
                original["UI"]["Widgets"][0]["Widgets"]
            );
            assert_eq!(result["DataBindings"], original["DataBindings"]);
            assert_eq!(result["Events"], original["Events"]);
            let css = result["CustomMashupCss"].as_str().unwrap();
            assert!(css.starts_with(".existing { color: red; }\n"));
            assert_eq!(css.matches(":start */").count(), 1);
            assert_eq!(
                css.matches("@media").count(),
                usize::from(compact.is_some())
            );
            if let Some(n) = compact {
                assert!(css.contains(&format!("@media (max-width: {n}px) {{")));
            }
            if !xml_only {
                let files = layout_bytes(root);
                assert_eq!(files[2], format!("{css}\n").as_bytes());
            }
        }
    }
}

#[test]
fn slider_action_measured_margins_preserve_controls_and_wiring() {
    for mcp in [false, true] {
        for labels in [None, Some(true), Some(false)] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            layout_fixture(root, true, |c| {
                if let Some(labels) = labels {
                    c["UI"]["Widgets"][0]["Widgets"][0]["Properties"]["DisplayMinMaxLabels"] =
                        json!(labels);
                }
                if labels == Some(false) {
                    c["UI"]["Widgets"][0]["Widgets"][1]["Properties"]["Size"] = json!("large");
                }
                if labels == Some(true) {
                    c["UI"]["Widgets"][0]["Properties"]["Style"] = json!("Acme.CustomStyle");
                }
                if let Some(labels) = labels {
                    c["UI"]["Properties"]["StyleProperties"]["data"]["flexcontainer-2"] = json!({"widget":"flex-container", "parts":{"$":{"$":{"background":if labels {"#ffffff"} else {"#F7F7F7"}}}}});
                }
            });
            let original = layout_result(root);
            let before = layout_bytes(root);
            let mut cli = vec![
                "mashup",
                "recipe",
                "slider-action",
                "Acme.Layout_MU",
                "ptcsslider-3",
                "ptcsbutton-4",
            ];
            let mut args = json!({"recipe":"slider-action", "mashup":"Acme.Layout_MU", "widget":"ptcsslider-3", "button":"ptcsbutton-4"});
            let plan = invoke(root, mcp, &cli, "mashup_recipe", args.clone());
            assert_eq!(layout_bytes(root), before);
            assert!(plan["notes"].to_string().contains("default 34px button"));
            cli.push("--apply");
            args["dry_run"] = json!(false);
            invoke(root, mcp, &cli, "mashup_recipe", args.clone());
            let after = layout_bytes(root);
            assert_eq!(
                invoke(root, mcp, &cli, "mashup_recipe", args)["unchanged"],
                true
            );
            assert_eq!(layout_bytes(root), after);
            assert_eq!(after[2], before[2]);
            let mut result = layout_result(root);
            let row = &mut result["UI"]["Widgets"][0];
            assert_eq!(row["Properties"]["align-items"], "flex-end");
            assert_eq!(row["Properties"]["flex-max-height"], "");
            assert_eq!(row["Widgets"][0]["Properties"]["Margin"], "0");
            assert_eq!(
                row["Widgets"][1]["Properties"]["Margin"],
                if labels == Some(false) {
                    "0 0 0 12"
                } else {
                    "0 0 18 12"
                }
            );
            for index in 0..2 {
                row["Widgets"][index]["Properties"]["Margin"] =
                    original["UI"]["Widgets"][0]["Widgets"][index]["Properties"]["Margin"].clone();
            }
            assert_eq!(row["Widgets"], original["UI"]["Widgets"][0]["Widgets"]);
            assert_eq!(result["DataBindings"], original["DataBindings"]);
            assert_eq!(result["Events"], original["Events"]);
            let background = &result["UI"]["Properties"]["StyleProperties"]["data"]
                ["flexcontainer-2"]["parts"]["$"]["$"]["background"];
            assert_eq!(
                background,
                if labels == Some(true) {
                    "#ffffff"
                } else if labels == Some(false) {
                    "#F7F7F7"
                } else {
                    "inherit"
                }
            );
        }
    }
}

fn layout_refused(root: &Path, cli: &[&str], arguments: Value) {
    let before = layout_bytes(root);
    let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .current_dir(root)
        .env_clear()
        .args(cli)
        .arg("--apply")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "{cli:?}: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let input = format!(
        "{}\n",
        json!({"jsonrpc":"2.0", "id":1,"method":"tools/call", "params":{"name":"mashup_recipe", "arguments":arguments}})
    );
    let mut out = Vec::new();
    twaco::mcp::serve_tools(
        root,
        &twaco::mcp::Selection::ALL,
        input.as_bytes(),
        &mut out,
    )
    .unwrap();
    let response: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert_eq!(layout_bytes(root), before);
}

#[test]
fn layout_recipes_refuse_unsafe_targets_and_options_without_writes() {
    for slider in [false, true] {
        let cases = if slider { 21 } else { 19 };
        for case in 0..cases {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            layout_fixture(root, slider, |c| {
                let p = &mut c["UI"]["Widgets"][0]["Properties"];
                match case {
                    0 => p["ResponsiveLayout"] = json!(false),
                    1 => p["positioning"] = json!("static"),
                    2 => p["flex-direction"] = json!(if slider { "column" } else { "row" }),
                    3 if slider => p["Type"] = json!("ptcslabel"),
                    3 => p["Style"] = json!("Acme.CustomStyle"),
                    4 if !slider => p["flex-min-width"] = json!("0px"),
                    5 if !slider => p["flex-max-width"] = json!("500px"),
                    4 => {
                        c["UI"]["Widgets"][0]["Widgets"][0]["Properties"]["VerticalSlider"] =
                            json!(true)
                    }
                    5 => {
                        c["UI"]["Widgets"][0]["Widgets"][0]["Properties"]["ResponsiveLayout"] =
                            json!(true)
                    }
                    6 => {
                        c["UI"]["Properties"]["StyleProperties"]["data"]["flexcontainer-4"]
                            ["parts"] = json!([])
                    }
                    7 => {
                        c["DataBindings"][0] = json!({"TargetArea":"UI", "TargetId":"flexcontainer-2", "PropertyMaps":[{"TargetProperty":"Margin"}]})
                    }
                    8 if !slider => {
                        c["UI"]["Widgets"][0]["Widgets"][0]["Properties"]["Style"] =
                            json!("Acme.CustomStyle")
                    }
                    8 => {
                        let controls = c["UI"]["Widgets"][0]["Widgets"].as_array_mut().unwrap();
                        controls.swap(0, 1);
                    }
                    9 if slider => {
                        let extra = c["UI"]["Widgets"][0]["Widgets"][1].clone();
                        c["UI"]["Widgets"][0]["Widgets"]
                            .as_array_mut()
                            .unwrap()
                            .push(extra);
                    }
                    10 if slider => {
                        c["UI"]["Widgets"][0]["Widgets"][1]["Properties"]["FillContainer"] =
                            json!(true)
                    }
                    11 if slider => {
                        c["DataBindings"][0] = json!({"TargetArea":"UI", "TargetId":"ptcsslider-3", "PropertyMaps":[{"TargetProperty":"Margin"}]})
                    }
                    12 if slider => {
                        c["UI"]["Widgets"][0]["Widgets"][1]["Properties"]["Type"] =
                            json!("ptcslabel")
                    }
                    15 if slider => {
                        let mut row = c["UI"]["Widgets"][0].clone();
                        row["Properties"]["Id"] = json!("flexcontainer-5");
                        let button = c["UI"]["Widgets"][0]["Widgets"]
                            .as_array_mut()
                            .unwrap()
                            .pop()
                            .unwrap();
                        row["Widgets"] = json!([button]);
                        c["UI"]["Widgets"].as_array_mut().unwrap().push(row);
                    }
                    16 if slider => {
                        c["UI"]["Widgets"][0]["Widgets"][1]["Properties"]["DisplayName"] =
                            json!("Slider")
                    }
                    13 if !slider => {
                        c["UI"]["Widgets"][0]["Properties"]["Type"] = json!("ptcslabel")
                    }
                    14 if !slider => {
                        c["UI"]["Widgets"][0]["Widgets"][0]["Properties"]["DisplayName"] =
                            json!("flexcontainer-2")
                    }
                    _ => {}
                }
            });
            if !slider && (9..13).contains(&case) {
                let content = layout_result(root);
                let css = ["/* twaco:panel:flexcontainer-2:start */\n", "/* twaco:panel:flexcontainer-2:end */\n", "/* twaco:panel:bad */\n", "/* twaco:panel:flexcontainer-2:start */\n/* twaco:panel:flexcontainer-2:start */\n"][case - 9];
                write_layout(root, &content, css);
            }
            let mut cli = vec![
                "mashup",
                "recipe",
                if slider { "slider-action" } else { "panel" },
                "Acme.Layout_MU",
                if slider {
                    "ptcsslider-3"
                } else {
                    "flexcontainer-2"
                },
            ];
            let mut args = json!({"recipe":if slider {"slider-action"} else {"panel"}, "mashup":"Acme.Layout_MU", "widget":if slider {"ptcsslider-3"} else {"flexcontainer-2"}, "dry_run":false});
            if slider {
                cli.push("ptcsbutton-4");
                args["button"] = json!("ptcsbutton-4");
            }
            if slider && case == 13 {
                cli.extend(["--compact", "768"]);
                args["compact"] = json!(768);
            }
            if slider && case == 14 {
                cli[4] = "ptcsbutton-4";
                args["widget"] = json!("ptcsbutton-4");
            }
            match (slider, case) {
                (false, 14) => {
                    cli[4] = "mashup-root";
                    args["widget"] = json!("mashup-root");
                }
                (false, 15) => {
                    cli[4] = "missing-widget";
                    args["widget"] = json!("missing-widget");
                }
                (false, 16) => {
                    cli.extend(["--compact", "769"]);
                    args["compact"] = json!(769);
                }
                (false, 17) | (true, 18) => {
                    cli.extend(["--say", "unrelated"]);
                    args["say"] = json!("unrelated");
                }
                (true, 16) => {
                    cli[4] = "Slider";
                    args["widget"] = json!("Slider");
                }
                (true, 17) => {
                    cli.pop();
                    args.as_object_mut().unwrap().remove("button");
                }
                (true, 19) => {
                    cli.push("extra");
                    args["button"] = json!("missing-widget");
                }
                (false, 18) | (true, 20) => {
                    let path = root.join("src/Acme.Layout_MU/mashup/content.json");
                    let mut content: Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    content["UI"]["Widgets"][0]["Properties"]["Margin"] = json!("stale");
                    std::fs::write(path, serde_json::to_vec_pretty(&content).unwrap()).unwrap();
                }
                _ => {}
            }
            layout_refused(root, &cli, args);
        }
    }
}

#[test]
fn panel_stops_at_contained_mashups_and_accepts_exact_prefixed_display_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    layout_fixture(root, false, |c| {
        c["UI"]["Widgets"][0]["Properties"]["DisplayName"] = json!("Details");
        let node = &mut c["UI"]["Widgets"][0]["Widgets"][0];
        node["Properties"]["Type"] = json!("mashupcontainer");
        node["Widgets"] = json!([{"Properties":{"Id":"flexcontainer-8", "Type":"flexcontainer", "Style":"Acme.CustomStyle", "ResponsiveLayout":true}, "Widgets":[]}]);
    });
    let before = layout_result(root)["UI"]["Widgets"][0]["Widgets"][0].clone();
    invoke(
        root,
        false,
        &[
            "mashup",
            "recipe",
            "panel",
            "Acme.Layout_MU",
            "ui:Details",
            "--apply",
        ],
        "",
        json!({}),
    );
    let result = layout_result(root);
    assert_eq!(result["UI"]["Widgets"][0]["Widgets"][0], before);
    assert!(result["UI"]["Properties"]["StyleProperties"]["data"]
        .get("flexcontainer-8")
        .is_none());
}

#[test]
fn card_preset_plans_and_applies_identically_through_cli_and_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    layout_fixture(root, false, |_| {});
    let cli = [
        "mashup",
        "recipe",
        "panel",
        "Acme.Layout_MU",
        "flexcontainer-2",
        "--style",
        "card",
    ];
    let args = json!({"recipe":"panel", "mashup":"Acme.Layout_MU", "widget":"flexcontainer-2", "style":"card"});
    let before = layout_bytes(root);
    let planned = invoke(root, false, &cli, "mashup_recipe", args.clone());
    let mut mcp_plan = invoke(root, true, &cli, "mashup_recipe", args.clone());
    for key in ["dry_run", "notices", "duration_ms"] {
        mcp_plan.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(planned, mcp_plan);
    assert_eq!(layout_bytes(root), before);
    let mut apply = args;
    apply["dry_run"] = json!(false);
    invoke(root, true, &cli, "mashup_recipe", apply);
    let after = layout_bytes(root);
    let mut cli = cli.to_vec();
    cli.push("--apply");
    assert_eq!(
        invoke(root, false, &cli, "mashup_recipe", json!({}))["unchanged"],
        true
    );
    assert_eq!(layout_bytes(root), after);
    let result = layout_result(root);
    assert_eq!(
        result["UI"]["Properties"]["StyleProperties"]["data"]["flexcontainer-2"]["parts"]["$"]["$"]
            ["background"],
        "#ffffff"
    );
}

#[test]
fn tree_and_tracker_options_plan_and_apply_through_cli_and_mcp() {
    for mcp in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(root.join("DataShapes")).unwrap();
        std::fs::write(root.join("Things/Acme.Manager.xml"), r#"<Entities><Things><Thing name="Acme.Manager" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetNodes"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Nodes_DS"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#).unwrap();
        std::fs::write(root.join("DataShapes/Acme.Nodes_DS.xml"), r#"<Entities><DataShapes><DataShape name="Acme.Nodes_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="id" baseType="STRING"/><FieldDefinition name="caption" baseType="STRING"/><FieldDefinition name="parentId" baseType="STRING"/><FieldDefinition name="stepNumber" baseType="NUMBER"/></FieldDefinitions></DataShape></DataShapes></Entities>"#).unwrap();
        invoke(
            root,
            mcp,
            &["mashup", "new", "Acme.Wizard_MU", "--apply"],
            "mashup_new",
            json!({"name":"Acme.Wizard_MU", "dry_run":false}),
        );
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "new",
                "Acme.Step_MU",
                "--param",
                "stepNumber:NUMBER:BOTH",
                "--apply",
            ],
            "mashup_new",
            json!({"name":"Acme.Step_MU", "params":["stepNumber:NUMBER:BOTH"], "dry_run":false}),
        );
        let host = invoke(
            root,
            mcp,
            &[
                "mashup",
                "add-contained",
                "Acme.Wizard_MU",
                "Acme.Step_MU",
                "--apply",
            ],
            "mashup_add_contained",
            json!({"mashup":"Acme.Wizard_MU", "child":"Acme.Step_MU", "dry_run":false}),
        );
        let host = host["id"].as_str().unwrap();
        let tree = invoke(
            root,
            mcp,
            &[
                "mashup",
                "add",
                "Acme.Wizard_MU",
                "tree",
                "--below",
                host,
                "--apply",
            ],
            "mashup_add",
            json!({"mashup":"Acme.Wizard_MU", "widget":"tree", "below":host, "dry_run":false}),
        );
        let tree = tree["id"].as_str().unwrap();
        let tracker = invoke(
            root,
            mcp,
            &[
                "mashup",
                "add",
                "Acme.Wizard_MU",
                "progress-tracker",
                "--below",
                tree,
                "--apply",
            ],
            "mashup_add",
            json!({"mashup":"Acme.Wizard_MU", "widget":"progress-tracker", "below":tree, "dry_run":false}),
        );
        let tracker_id = tracker["id"].as_str().unwrap();
        for (name, widget, flags, options) in [
            (
                "tree",
                tree,
                vec!["--field-map", "NameField=caption"],
                json!({"field_maps":["NameField=caption"]}),
            ),
            (
                "tracker",
                tracker_id,
                vec![
                    "--field-map",
                    "StepLabelField=caption",
                    "--initial-step",
                    "2",
                    "--step-size",
                    "small",
                    "--host",
                    host,
                ],
                json!({"field_maps":["StepLabelField=caption"], "initial_step":2, "step_size":"small", "host":host}),
            ),
        ] {
            let mut cli = vec![
                "mashup",
                "recipe",
                name,
                "Acme.Wizard_MU",
                widget,
                "--data",
                "data:Acme.Manager.GetNodes",
            ];
            cli.extend(flags);
            let mut args = options;
            for (key, value) in [
                ("mashup", "Acme.Wizard_MU"),
                ("recipe", name),
                ("widget", widget),
                ("data", "data:Acme.Manager.GetNodes"),
            ] {
                args[key] = json!(value);
            }
            let before = std::fs::read(root.join("Mashups/Acme.Wizard_MU.xml")).unwrap();
            invoke(root, mcp, &cli, "mashup_recipe", args.clone());
            assert_eq!(
                std::fs::read(root.join("Mashups/Acme.Wizard_MU.xml")).unwrap(),
                before
            );
            cli.push("--apply");
            args["dry_run"] = json!(false);
            invoke(root, mcp, &cli, "mashup_recipe", args);
        }
        let content: Value = serde_json::from_slice(
            &std::fs::read(root.join("src/Acme.Wizard_MU/mashup/content.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            properties(&content["UI"], "ptcstreeselector").unwrap()["NameField"],
            "caption"
        );
        let tracker = properties(&content["UI"], "ptcsprogresstracker").unwrap();
        assert_eq!(tracker["CurrentStepNumber"], 2);
        assert_eq!(tracker["StepSize"], "small");
        let edges: Vec<_> = content["DataBindings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|binding| binding["TargetId"] == tracker_id || binding["TargetId"] == host)
            .map(|binding| {
                (
                    binding["SourceId"].as_str().unwrap(),
                    binding["PropertyMaps"][0]["TargetProperty"]
                        .as_str()
                        .unwrap(),
                )
            })
            .collect();
        assert_eq!(
            edges,
            vec![
                (host, "CurrentStepNumber"),
                (tracker_id, "stepNumber"),
                (tracker_id, "Name"),
                ("GetNodes", "Data")
            ]
        );
    }
}
