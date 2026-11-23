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
        let fields = named(&content["UI"], "filters").unwrap();
        // Kit structure: controls sit directly in one fields column, no per-field rows.
        assert_eq!(fields["Properties"]["flex-direction"], "column");
        assert_eq!(
            leaf_ids(fields),
            [search.clone(), date.clone(), category.clone()]
        );
        for (index, id) in [&search, &date, &category].into_iter().enumerate() {
            let control = &fields["Widgets"][index];
            assert_eq!(control["Properties"]["Id"], id.as_str());
            assert!(control["Properties"]["Width"].as_f64().unwrap() <= 248.0);
            assert_eq!(control["Properties"]["ResponsiveLayout"], false);
        }
        let footer = named(&content["UI"], "filter-footer").unwrap();
        assert_eq!(leaf_ids(footer), [apply, clear]);
        assert_eq!(footer["Properties"]["LastContainer"], true);
        assert!(!content.to_string().contains("--remaining-space"));
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
        // V1 pages carry no generated filler: the page column itself is the remainder container.
        let rows = center["Widgets"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "{center}");
        assert!(rows.iter().all(|row| row["twacoLayoutFiller"].is_null()));
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
                if row["twacoLayoutFiller"] == true {
                    continue;
                }
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
                    "--date-format",
                    "dd-MM-yyyy",
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
                json!({"date_format":"dd-MM-yyyy", "data":"data:Acme.Manager.GetSteps", "columns":["start:Started:180", "caption:Step", "Notes"], "editable_columns":["Notes"], "column_editors":["Notes=textarea"], "preserve_whitespace":["Notes"]}),
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
                    assert_eq!(columns[0]["FormatOptions"]["FormatString"], "dd-MM-yyyy");
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
        &[
            "mashup",
            "new",
            "Acme.Layout_MU",
            "--rules",
            "legacy",
            "--apply",
        ],
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

fn n7_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
    for collection in ["Things", "DataShapes", "StateDefinitions"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
    }
    std::fs::write(root.join("Things/Acme.Manager.xml"), r#"<Entities><Things><Thing name="Acme.Manager" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetRows"><ParameterDefinitions><FieldDefinition name="query" baseType="STRING"/></ParameterDefinitions><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Rows_DS"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#).unwrap();
    std::fs::write(root.join("DataShapes/Acme.Rows_DS.xml"), r#"<Entities><DataShapes><DataShape name="Acme.Rows_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="Severity" baseType="STRING"/><FieldDefinition name="Value" baseType="NUMBER"/></FieldDefinitions></DataShape></DataShapes></Entities>"#).unwrap();
    std::fs::write(root.join("StateDefinitions/Acme.Status_ST.xml"), r#"<Entities><StateDefinitions><StateDefinition name="Acme.Status_ST" projectName="Acme"><content><![CDATA[{"stateType":"string","stateDefinitions":[{"defaultValue":"Low","displayString":"Low","defaultStyleDefinition":{"image":"Acme.Good_MD","displayString":""}},{"displayString":"Other","defaultStyleDefinition":"DefaultCollectionCellAlternateStyle"}]}]]></content></StateDefinition></StateDefinitions></Entities>"#).unwrap();
    dir
}

fn n7_new(root: &Path, mcp: bool, template: &str) {
    invoke(
        root,
        mcp,
        &[
            "mashup",
            "new",
            "Acme.Layout_MU",
            "--template",
            template,
            "--apply",
        ],
        "mashup_new",
        json!({"name":"Acme.Layout_MU", "template":template, "dry_run":false}),
    );
}

fn n7_content(root: &Path) -> Value {
    serde_json::from_slice(
        &std::fs::read(root.join("src/Acme.Layout_MU/mashup/content.json")).unwrap(),
    )
    .unwrap()
}

fn n7_add(root: &Path, mcp: bool, kind: &str, slot: &str) -> String {
    invoke(
        root,
        mcp,
        &[
            "mashup",
            "add",
            "Acme.Layout_MU",
            kind,
            "--in",
            slot,
            "--apply",
        ],
        "mashup_add",
        json!({"mashup":"Acme.Layout_MU", "widget":kind, "in":slot, "dry_run":false}),
    )["id"]
        .as_str()
        .unwrap()
        .into()
}

fn n7_recipe(
    root: &Path,
    mcp: bool,
    recipe: &str,
    widget: &str,
    flags: &[&str],
    mut options: Value,
) -> Value {
    let mut cli = vec![
        "mashup",
        "recipe",
        recipe,
        "Acme.Layout_MU",
        widget,
        "--apply",
    ];
    cli.extend(flags);
    options["mashup"] = json!("Acme.Layout_MU");
    options["recipe"] = json!(recipe);
    options["widget"] = json!(widget);
    options["dry_run"] = json!(false);
    invoke(root, mcp, &cli, "mashup_recipe", options)
}

fn named<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    if value["Properties"]["DisplayName"] == name {
        return Some(value);
    }
    value["Widgets"]
        .as_array()?
        .iter()
        .find_map(|child| named(child, name))
}

#[test]
fn n7_bound_recipes_warn_until_a_call_has_any_trigger() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "list-page");
        let grid = n7_add(root, mcp, "grid", "body");
        let columns = || {
            n7_recipe(
                root,
                mcp,
                "columns",
                &grid,
                &[
                    "--data",
                    "data:Acme.Manager.GetRows",
                    "--column",
                    "Severity",
                ],
                json!({"data":"data:Acme.Manager.GetRows", "columns":["Severity"]}),
            )
        };
        let result = columns();
        assert!(result["notes"].as_array().unwrap().iter().any(|note| note.as_str().unwrap().contains("twaco mashup on Acme.Layout_MU --event mashup:Loaded --invoke data:Acme.Manager.GetRows --apply")), "{result}");
        let button = n7_add(root, mcp, "button", "toolbar");
        let event = format!("ui:{button}.Clicked");
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "on",
                "Acme.Layout_MU",
                "--event",
                &event,
                "--invoke",
                "data:Acme.Manager.GetRows",
                "--apply",
            ],
            "mashup_on",
            json!({"mashup":"Acme.Layout_MU", "event":event, "invoke":"data:Acme.Manager.GetRows", "dry_run":false}),
        );
        let result = columns();
        assert!(
            !result["notes"].to_string().contains("has no trigger"),
            "{result}"
        );
    }
}

#[test]
fn n7_template_toolbar_accepts_end_and_keeps_its_height() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "list-page");
        let first = n7_add(root, mcp, "dropdown", "toolbar");
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "add",
                "Acme.Layout_MU",
                "button",
                "--right-of",
                &first,
                "--end",
                "--apply",
            ],
            "mashup_add",
            json!({"mashup":"Acme.Layout_MU", "widget":"button", "right_of":first, "end":true, "dry_run":false}),
        );
        let content = n7_content(root);
        let toolbar = named(&content["UI"], "toolbar").unwrap();
        assert_eq!(toolbar["Properties"]["justify-content"], "space-between");
        assert_eq!(toolbar["Properties"]["flex-size"], "fixed");
        assert_eq!(toolbar["Properties"]["flex-max-height"], "56px");
        assert_eq!(toolbar["Widgets"].as_array().unwrap().len(), 2);
    }
}

#[test]
fn n7_tabs_share_one_gutter_with_content_without_a_title_gap() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "new",
                "Acme.Content_MU",
                "--template",
                "tab-content",
                "--apply",
            ],
            "mashup_new",
            json!({"name":"Acme.Content_MU", "template":"tab-content", "dry_run":false}),
        );
        let child: Value = serde_json::from_slice(
            &std::fs::read(root.join("src/Acme.Content_MU/mashup/content.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            named(&child["UI"], "page").unwrap()["Properties"]["Margin"],
            ""
        );
        assert!(named(&child["UI"], "title").is_none());
        let result = invoke(
            root,
            mcp,
            &[
                "mashup",
                "new",
                "Acme.Layout_MU",
                "--template",
                "tabs",
                "--tab",
                "Device=Acme.Content_MU",
                "--apply",
            ],
            "mashup_new",
            json!({"name":"Acme.Layout_MU", "template":"tabs", "tabs":["Device=Acme.Content_MU"], "dry_run":false}),
        );
        assert!(!result["notes"].to_string().contains("may scroll"));
        let host = n7_content(root);
        assert_eq!(
            named(&host["UI"], "page").unwrap()["Properties"]["Margin"],
            "24"
        );
        assert_eq!(properties(&host["UI"], "ptcstabset").unwrap()["Margin"], "");
        assert_eq!(
            properties(&host["UI"], "mashupcontainer").unwrap()["Margin"],
            ""
        );
    }
}

#[test]
fn n7_columns_render_kit_status_icon_and_text_without_header_dividers() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "list-page");
        let grid = n7_add(root, mcp, "grid", "body");
        n7_recipe(
            root,
            mcp,
            "columns",
            &grid,
            &[
                "--data",
                "data:Acme.Manager.GetRows",
                "--column",
                "Severity",
                "--status",
                "Severity=Acme.Status_ST",
            ],
            json!({"data":"data:Acme.Manager.GetRows", "columns":["Severity"], "status":"Severity=Acme.Status_ST"}),
        );
        let content = n7_content(root);
        let column =
            &properties(&content["UI"], "ptcsgrid").unwrap()["ColumnFormat"]["formatInfo"][0];
        assert_eq!(column["FormatOptions"]["FormatString"], "full");
        assert_eq!(column["FormatOptions"]["renderer"], "STRING");
        assert_eq!(
            column["FormatOptions"]["formatInfo"]["StateFormats"][0]["state"]["image"],
            "Acme.Good_MD"
        );
        assert_eq!(
            content["UI"]["Properties"]["StyleProperties"]["data"][&grid]["parts"]["$"]["$"]
                ["--ptcs-header-vert-divider-width"],
            "0px"
        );
    }
}

#[test]
fn default_v1_filter_title_and_footer_match_kit_alignment_and_spacing() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "filter-panel");
        let title = n7_add(root, mcp, "label", "filters");
        n7_add(root, mcp, "text-field", "filters");
        let content = n7_content(root);
        let filters = named(&content["UI"], "filters").unwrap();
        assert_eq!(filters["Properties"]["align-items"], "flex-start");
        assert_eq!(filters["Properties"]["flex-grow"], 1);
        assert_eq!(
            find_widget(&content["UI"], &title).unwrap()["Properties"]["HorizontalAlignment"],
            "left"
        );
        let footer = named(&content["UI"], "filter-footer").unwrap();
        assert_eq!(footer["Properties"]["LastContainer"], true);
        // Kit footer: a column holding the divider row, then the button row.
        let area = named(&content["UI"], "filter-footer-area").unwrap();
        assert_eq!(area["Widgets"].as_array().unwrap().last().unwrap(), footer);
        let divider = &area["Widgets"][0];
        assert_eq!(divider["Properties"]["flex-max-height"], "1px");
        for (label, kind) in [("Apply", "primary"), ("Reset", "tertiary")] {
            let result = invoke(
                root,
                mcp,
                &[
                    "mashup",
                    "add",
                    "Acme.Layout_MU",
                    "button",
                    "--in",
                    "filter-footer",
                    "--label",
                    label,
                    "--apply",
                ],
                "mashup_add",
                json!({"mashup":"Acme.Layout_MU", "widget":"button", "in":"filter-footer", "label":label,"dry_run":false}),
            );
            let content = n7_content(root);
            let button = find_widget(&content["UI"], result["id"].as_str().unwrap()).unwrap();
            assert_eq!(button["Properties"]["ButtonType"], kind);
            assert_eq!(
                button["Properties"]["Margin"],
                if kind == "primary" {
                    "24 8 24 24"
                } else {
                    "24 24 24 8"
                }
            );
        }
    }
}

#[test]
fn n7_collapsible_reuses_filter_panels_native_toggle() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "filter-panel");
        let result = n7_recipe(
            root,
            mcp,
            "collapsible",
            "filter-column",
            &["--add-toggle", "--in", "content"],
            json!({"add_toggle":true,"in_container":"content"}),
        );
        assert!(
            result["notes"].to_string().contains("reusing its existing"),
            "{result}"
        );
        let content = n7_content(root);
        assert!(properties(&content["UI"], "ptcsbutton").is_none());
        assert_eq!(
            named(&content["UI"], "filter-column").unwrap()["Properties"]["ShowExpandCollapseTab"],
            true
        );
    }
}

#[test]
fn n7_search_toolbar_uses_the_same_grey_band_as_date_toolbars() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "filter-panel");
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "source",
                "add",
                "Acme.Layout_MU",
                "Acme.Manager",
                "GetRows",
                "--apply",
            ],
            "mashup_source_add",
            json!({"mashup":"Acme.Layout_MU","entity":"Acme.Manager","service":"GetRows","dry_run":false}),
        );
        n7_recipe(
            root,
            mcp,
            "toolbar",
            "content",
            &[
                "--then",
                "data:Acme.Manager.GetRows",
                "--search-param",
                "query",
            ],
            json!({"then":"data:Acme.Manager.GetRows","search_param":"query"}),
        );
        let content = n7_content(root);
        let styles = &content["UI"]["Properties"]["StyleProperties"]["data"];
        let toolbar = &named(&content["UI"], "content").unwrap()["Widgets"][0]["Properties"]["Id"];
        assert_eq!(
            styles[toolbar.as_str().unwrap()]["parts"]["$"]["$"]["background"],
            "#F7F7F7"
        );
        let field = properties(&content["UI"], "ptcstextfield").unwrap();
        assert_eq!(field["HintText"], "Search...");
    }
}

#[test]
fn n7_pagination_is_centered_wired_to_grid_and_safe_to_repeat() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "list-page");
        let grid = n7_add(root, mcp, "grid", "body");
        let apply = || {
            n7_recipe(
                root,
                mcp,
                "pagination",
                &grid,
                &["--data", "data:Acme.Manager.GetRows"],
                json!({"data":"data:Acme.Manager.GetRows"}),
            )
        };
        apply();
        let content = n7_content(root);
        apply();
        assert_eq!(n7_content(root), content);
        let pager = properties(&content["UI"], "ptcspagination").unwrap();
        let bindings = content["DataBindings"].as_array().unwrap();
        assert!(bindings.iter().any(|b| b["TargetId"] == pager["Id"]
            && b["PropertyMaps"][0]["TargetProperty"] == "ResultsNumber"));
        assert!(bindings.iter().any(|b| b["TargetId"] == grid
            && b["SourceArea"] == "UI"
            && b["PropertyMaps"][0]["TargetProperty"] == "Data"));
        fn centered(value: &Value, id: &Value) -> bool {
            value["Properties"]["justify-content"] == "center"
                && value["Widgets"].as_array().is_some_and(|children| {
                    children
                        .iter()
                        .any(|child| child["Properties"]["Id"] == *id)
                })
                || value["Widgets"]
                    .as_array()
                    .is_some_and(|children| children.iter().any(|child| centered(child, id)))
        }
        assert!(centered(&content["UI"], &pager["Id"]));
        assert!(content.to_string().contains("table.rows.slice"));
    }
}

#[test]
fn n7_narrow_date_picker_defaults_to_short_dates_and_explicit_format_wins() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "filter-panel");
        let date = n7_add(root, mcp, "datepicker", "filters");
        let content = n7_content(root);
        assert_eq!(
            find_widget(&content["UI"], &date).unwrap()["Properties"]["FormatToken"],
            "DD-MM-YY"
        );
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "add",
                "Acme.Layout_MU",
                "datepicker",
                "--in",
                "filters",
                "--prop",
                "FormatToken=YYYY-MM-DD",
                "--apply",
            ],
            "mashup_add",
            json!({"mashup":"Acme.Layout_MU","widget":"datepicker","in":"filters","properties":{"FormatToken":"YYYY-MM-DD"},"dry_run":false}),
        );
        let content = n7_content(root);
        assert!(content.to_string().contains("YYYY-MM-DD"));
    }
}

fn parity_new(root: &Path, mcp: bool, template: &str) {
    invoke(
        root,
        mcp,
        &[
            "mashup",
            "new",
            "Acme.Layout_MU",
            "--template",
            template,
            "--apply",
        ],
        "mashup_new",
        json!({"name":"Acme.Layout_MU","template":template,"dry_run":false}),
    );
}

#[test]
fn command_default_v1_page_fit_preserves_tall_content_and_pagination() {
    for mcp in [false, true] {
        for template in ["blank", "list-page"] {
            let dir = n7_fixture();
            let root = dir.path();
            parity_new(root, mcp, template);
            let slot = if template == "blank" {
                "center"
            } else {
                "body"
            };
            let grid = n7_add(root, mcp, "grid", slot);
            n7_recipe(
                root,
                mcp,
                "columns",
                &grid,
                &["--data", "data:Acme.Manager.GetRows", "--column", "Value"],
                json!({"data":"data:Acme.Manager.GetRows","columns":["Value"]}),
            );
            invoke(
                root,
                mcp,
                &[
                    "mashup",
                    "set",
                    "Acme.Layout_MU",
                    &grid,
                    "--prop",
                    "Height=1100",
                    "--apply",
                ],
                "mashup_set",
                json!({"mashup":"Acme.Layout_MU","widget":grid,"properties":{"Height":1100},"dry_run":false}),
            );
            n7_recipe(
                root,
                mcp,
                "pagination",
                &grid,
                &["--data", "data:Acme.Manager.GetRows"],
                json!({"data":"data:Acme.Manager.GetRows"}),
            );
            if template == "blank" {
                n7_recipe(root, mcp, "panel", slot, &[], json!({}));
                let css =
                    std::fs::read_to_string(root.join("src/Acme.Layout_MU/mashup/custom.css"))
                        .unwrap();
                assert!(css.contains("height: auto !important"));
                assert!(!css.contains("min-height: 0 !important"));
                assert!(css.contains("flex: 1 0 auto"));
            }
            let c = n7_content(root);
            fn check(node: &Value) {
                if node["Properties"]["Type"] == "flexcontainer" {
                    assert_eq!(node["Properties"]["ShowVerticalScrollbar"], false);
                    assert_eq!(node["Properties"]["scrolling"], "no-scroll");
                }
                for child in node["Widgets"].as_array().into_iter().flatten() {
                    check(child);
                }
            }
            check(&c["UI"]);
            assert!(properties(&c["UI"], "ptcspagination").is_some());
            let page = &c["UI"]["Widgets"][0]["Properties"];
            let floor = page["flex-min-height"]
                .as_str()
                .unwrap()
                .trim_end_matches("px")
                .parse::<u32>()
                .unwrap();
            assert!(floor >= 1100, "{page}");
        }
    }
}

#[test]
fn command_default_v1_filter_fields_start_above_a_pinned_footer() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        parity_new(root, mcp, "filter-panel");
        n7_add(root, mcp, "textfield", "filters");
        n7_add(root, mcp, "datepicker", "filters");
        n7_add(root, mcp, "button", "filter-footer");
        n7_add(root, mcp, "button", "filter-footer");
        let c = n7_content(root);
        let fields = &named(&c["UI"], "filters").unwrap()["Properties"];
        assert_eq!(fields["justify-content"], "flex-start");
        assert_eq!(fields["flex-grow"], 1);
        assert_eq!(fields["flex-shrink"], 1);
        assert_eq!(fields["flex-min-height"], "");
        assert_eq!(
            c["UI"]["Properties"]["StyleProperties"]["data"][named(&c["UI"], "filter-column")
                .unwrap()["Properties"]["Id"]
                .as_str()
                .unwrap()]["parts"]["$"]["$"]["max-height"],
            "100vh"
        );
        let footer = &named(&c["UI"], "filter-footer").unwrap()["Properties"];
        assert_eq!(footer["LastContainer"], true);
        assert_eq!(footer["flex-size"], "default");
    }
}

#[test]
fn command_columns_can_select_the_first_row() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "blank");
        let grid = n7_add(root, mcp, "grid", "center");
        n7_recipe(
            root,
            mcp,
            "columns",
            &grid,
            &[
                "--data",
                "data:Acme.Manager.GetRows",
                "--column",
                "Value",
                "--auto-select-first",
            ],
            json!({"data":"data:Acme.Manager.GetRows","columns":["Value"],"auto_select_first":true}),
        );
        let c = n7_content(root);
        assert_eq!(
            properties(&c["UI"], "ptcsgrid").unwrap()["DefaultSelectedRows"],
            "1"
        );
    }
}

#[test]
fn command_combo_series_share_secondary_axis_and_compact_legend_props() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "blank");
        let chart = n7_add(root, mcp, "ptcschartcombo", "center");
        n7_recipe(
            root,
            mcp,
            "chart",
            &chart,
            &[
                "--data",
                "data:Acme.Manager.GetRows",
                "--x",
                "Severity",
                "--series",
                "Value:bar",
                "--series",
                "Value:line:secondary",
            ],
            json!({"data":"data:Acme.Manager.GetRows","x":"Severity","series":["Value:bar","Value:line:secondary"]}),
        );
        let c = n7_content(root);
        let p = properties(&c["UI"], "ptcschartcombo").unwrap();
        assert_eq!(p["Series1Type"], "bar");
        assert_eq!(p["Series2Type"], "line");
        assert_eq!(p["DisplaySeries2On"], "yaxis2");
        assert_eq!(p["NumberOfYaxes"], 2);
        assert_eq!(p["YAxis2Label"], "Value");
        assert_eq!(p["YAxis2Position"], "right");
        assert_eq!(p["DisableLegendGrouping"], true);
        assert_eq!(p["LegendPosition"], "top");
        assert!(p["DataField3"].is_null() || p["DataField3"] == "");
        n7_recipe(
            root,
            mcp,
            "chart",
            &chart,
            &[
                "--data",
                "data:Acme.Manager.GetRows",
                "--x",
                "Severity",
                "--series",
                "Value:line",
            ],
            json!({"data":"data:Acme.Manager.GetRows","x":"Severity","series":["Value:line"]}),
        );
        let c = n7_content(root);
        let p = properties(&c["UI"], "ptcschartcombo").unwrap();
        assert_eq!(p["NumberOfYaxes"], 1);
        assert_eq!(p["DataField2"], "");
    }
}

#[test]
fn command_sparkline_fits_40px_and_binds_service_rows_directly() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "blank");
        n7_add(root, mcp, "label", "center");
        n7_recipe(
            root,
            mcp,
            "sparkline",
            "center",
            &[
                "--data",
                "data:Acme.Manager.GetRows",
                "--x",
                "Severity",
                "--value",
                "Value",
            ],
            json!({"data":"data:Acme.Manager.GetRows","x":"Severity","value":"Value"}),
        );
        let c = n7_content(root);
        let p = properties(&c["UI"], "ptcschartline").unwrap();
        assert_eq!(p["LabelType"], "label");
        for key in [
            "HorizontalAxisMaxHeight",
            "VerticalAxisMaxWidth",
            "LegendMaxWidth",
        ] {
            assert_eq!(p[key], 0);
        }
        for key in [
            "HorizontalZoom",
            "HorizontalRangeZoom",
            "HorizontalSliderZoom",
            "VerticalZoom",
            "VerticalSliderZoom",
            "ShowZoomButtons",
            "ShowHideLegend",
        ] {
            assert_eq!(p[key], false);
        }
        assert!(p.get("YAxisMinimumValues").is_none());
        assert!(p.get("YAxisMaximumValues").is_none());
        assert_eq!(p["Height"], 40);
        assert_eq!(p["ResponsiveLayout"], true);
        assert!(p.get("Width").is_none());
        assert_eq!(p["SparkView"], true);
        assert_eq!(p["HideXAxis"], true);
        assert_eq!(p["HideYAxis"], true);
        assert_eq!(p["HideLegend"], true);
        assert_eq!(p["DataField1"], "Value");
        assert_eq!(p["XAxisField"], "Severity");
        fn wrapper<'a>(v: &'a Value, id: &Value) -> Option<&'a Value> {
            if v["Widgets"]
                .as_array()
                .is_some_and(|a| a.iter().any(|c| c["Properties"]["Id"] == *id))
            {
                return Some(v);
            }
            v["Widgets"].as_array()?.iter().find_map(|c| wrapper(c, id))
        }
        let w = wrapper(&c["UI"], &p["Id"]).unwrap();
        assert_eq!(w["Properties"]["flex-min-height"], "40px");
        assert_eq!(w["Properties"]["flex-max-height"], "40px");
        let parts =
            &c["UI"]["Properties"]["StyleProperties"]["data"][p["Id"].as_str().unwrap()]["parts"];
        assert_eq!(parts["$"]["$"]["min-height"], "0px");
        assert_eq!(parts["chart-layout"]["$"]["padding-top"], "0px");
        for part in [
            "title-area",
            "notes-area",
            "legend-area",
            "action-bar-area",
            "zoom-xaxis",
            "zoom-yaxis",
            "xaxis",
            "yaxis",
            "yaxis2",
        ] {
            assert_eq!(parts[part]["$"]["display"], "none", "{part}");
        }
        let b = c["DataBindings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["TargetId"] == p["Id"])
            .unwrap();
        assert_eq!(b["SourceArea"], "Data");
        assert_eq!(b["SourceDetails"], "AllData");
        assert!(properties(&c["UI"], "ptcslabel").is_some());
        assert!(properties(&c["UI"], "expression2").is_none());
    }
}

#[test]
fn command_chart_and_sparkline_errors_preserve_entity_and_sidecars() {
    let dir = n7_fixture();
    let root = dir.path();
    n7_new(root, false, "blank");
    let chart = n7_add(root, false, "ptcschartcombo", "center");
    let content = root.join("src/Acme.Layout_MU/mashup/content.json");
    let entity = root.join("Mashups/Acme.Layout_MU.xml");
    let before = (
        std::fs::read(&content).unwrap(),
        std::fs::read(&entity).unwrap(),
    );
    for series in [
        "Value:area",
        "Value:line:tertiary",
        "Missing:bar",
        "Severity:line",
        "Value:bar:primary:extra",
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .env_clear()
            .args([
                "mashup",
                "recipe",
                "chart",
                "Acme.Layout_MU",
                &chart,
                "--data",
                "data:Acme.Manager.GetRows",
                "--x",
                "Severity",
                "--series",
                series,
                "--apply",
            ])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{series}");
        assert!(!out.stderr.is_empty());
        assert_eq!(
            (
                std::fs::read(&content).unwrap(),
                std::fs::read(&entity).unwrap()
            ),
            before
        );
    }
    for value in ["Missing", "Severity"] {
        let out = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .env_clear()
            .args([
                "mashup",
                "recipe",
                "sparkline",
                "Acme.Layout_MU",
                "center",
                "--data",
                "data:Acme.Manager.GetRows",
                "--x",
                "Severity",
                "--value",
                value,
                "--apply",
            ])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(
            (
                std::fs::read(&content).unwrap(),
                std::fs::read(&entity).unwrap()
            ),
            before
        );
    }
}

#[test]
fn creation_rules_survive_add_set_and_recipe_edits() {
    for mcp in [false, true] {
        for rules in [None, Some("v1"), Some("legacy")] {
            let dir = n7_fixture();
            let root = dir.path();
            let mut cli = vec!["mashup", "new", "Acme.Layout_MU", "--apply"];
            let mut arguments = json!({"name":"Acme.Layout_MU","dry_run":false});
            if let Some(rules) = rules {
                cli.extend(["--rules", rules]);
                arguments["rules"] = json!(rules);
            }
            invoke(root, mcp, &cli, "mashup_new", arguments);
            let v1 = rules != Some("legacy");
            let check = || {
                let content = n7_content(root);
                assert_eq!(
                    content.get("twacoPageRules"),
                    v1.then(|| json!("v1")).as_ref()
                );
                let why = invoke(
                    root,
                    mcp,
                    &["mashup", "why", "Acme.Layout_MU", "center"],
                    "mashup_why",
                    json!({"mashup":"Acme.Layout_MU","widget":"center"}),
                );
                assert_eq!(why["rules"], if v1 { "v1" } else { "legacy" });
            };
            check();
            let label = n7_add(root, mcp, "label", "center");
            check();
            invoke(
                root,
                mcp,
                &[
                    "mashup",
                    "set",
                    "Acme.Layout_MU",
                    &label,
                    "--prop",
                    "LabelText=Edited",
                    "--apply",
                ],
                "mashup_set",
                json!({"mashup":"Acme.Layout_MU","widget":label,"properties":{"LabelText":"Edited"},"dry_run":false}),
            );
            check();
            n7_recipe(root, mcp, "panel", "center", &[], json!({}));
            check();
            let css =
                std::fs::read_to_string(root.join("src/Acme.Layout_MU/mashup/custom.css")).unwrap();
            assert_eq!(css.contains("height: auto !important"), v1, "{css}");
            assert_eq!(css.contains("flex: 1 0 auto"), v1, "{css}");
        }
    }
}

#[test]
fn command_side_panel_recipe_builds_the_kit_column() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "blank");
        let column = n7_add(root, mcp, "flexcontainer", "center");
        let result = n7_recipe(
            root,
            mcp,
            "side-panel",
            &column,
            &[
                "--title",
                "Compare",
                "--info",
                "Pick assets",
                "--tab",
                "Assets",
            ],
            json!({"title":"Compare","info":"Pick assets","tab":"Assets"}),
        );
        assert_eq!(result["applied"], true, "{result}");
        let c = n7_content(root);
        let panel = find_widget(&c["UI"], &column).unwrap();
        assert_eq!(panel["Properties"]["flex-min-width"], "276px");
        assert_eq!(panel["Properties"]["flex-max-width"], "276px");
        assert_eq!(
            c["UI"]["Properties"]["StyleProperties"]["data"][&column]["parts"]["$"]["$"]
                ["background"],
            "#F7F7F7"
        );
        let tabs = properties(panel, "ptcstabset").unwrap();
        assert_eq!(tabs["Tab1Name"], "Assets");
        assert_eq!(tabs["NumberOfTabs"], 1);
        assert_eq!(
            properties(panel, "ptcsbutton").unwrap()["TooltipField"],
            "Pick assets"
        );
        let tabs_id = tabs["Id"].as_str().unwrap();
        let body = &find_widget(&c["UI"], tabs_id).unwrap()["Widgets"][0]["Properties"]["Id"];
        let field = n7_add(root, mcp, "text-field", body.as_str().unwrap());
        assert_eq!(
            find_widget(&n7_content(root)["UI"], &field).unwrap()["Properties"]["Width"],
            240
        );
        // A second run is refused: the column is no longer empty.
        let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .env_clear()
            .args([
                "mashup",
                "recipe",
                "side-panel",
                "Acme.Layout_MU",
                &column,
                "--title",
                "Again",
                "--apply",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("needs an empty container"));
    }
}

#[test]
fn command_empty_state_and_multiple_picker_serve_collections_grids_and_lists() {
    for mcp in [false, true] {
        let one = |kind: &str| {
            let dir = n7_fixture();
            n7_new(dir.path(), mcp, "blank");
            let id = n7_add(dir.path(), mcp, kind, "center");
            (dir, id)
        };
        let (dir, grid) = one("grid");
        n7_recipe(
            dir.path(),
            mcp,
            "empty-state",
            &grid,
            &["--say", "Nothing yet"],
            json!({"say":"Nothing yet"}),
        );
        assert_eq!(
            properties(&n7_content(dir.path())["UI"], "ptcsgrid").unwrap()["NoResultsLabel"],
            "Nothing yet"
        );
        let (dir, collection) = one("collection");
        n7_recipe(
            dir.path(),
            mcp,
            "empty-state",
            &collection,
            &["--say", "No results."],
            json!({"say":"No results."}),
        );
        let c = n7_content(dir.path());
        let shown = properties(&c["UI"], "ptcscollection").unwrap();
        assert_eq!(shown["NoDataText"], "No results.");
        assert_eq!(shown["NoDataIcon"], "cds:icon_not_visible");
        let (dir, list) = one("list");
        let root = dir.path();
        n7_recipe(
            root,
            mcp,
            "picker",
            &list,
            &[
                "--data",
                "data:Acme.Manager.GetRows",
                "--x",
                "Severity",
                "--multiple",
                "--no-none-row",
            ],
            json!({"data":"data:Acme.Manager.GetRows","x":"Severity","multiple":true,"no_none_row":true}),
        );
        let result = n7_recipe(
            root,
            mcp,
            "empty-state",
            &list,
            &[
                "--say",
                "No results.",
                "--data",
                "data:Acme.Manager.GetRows",
            ],
            json!({"say":"No results.","data":"data:Acme.Manager.GetRows"}),
        );
        assert_eq!(result["applied"], true, "{result}");
        let c = n7_content(root);
        let list = properties(&c["UI"], "ptcslist").unwrap();
        assert_eq!(list["MultiSelect"], true);
        assert_eq!(list["ClearSelectionItem"], false);
        // The list's notice is a button shown while its rows are empty.
        fn find<'a>(w: &'a Value, label: &str) -> Option<&'a Value> {
            if w["Properties"]["Label"] == label {
                return Some(w);
            }
            w["Widgets"].as_array()?.iter().find_map(|c| find(c, label))
        }
        let notice = find(&c["UI"], "No results.").expect("notice");
        assert_eq!(notice["Properties"]["SVGIcon"], "cds:icon_not_visible");
        let notice_row =
            parent_widget(&c["UI"], notice["Properties"]["Id"].as_str().unwrap()).unwrap();
        let list_row = parent_widget(&c["UI"], list["Id"].as_str().unwrap()).unwrap();
        assert_eq!(list_row["Properties"]["Visible"], false);
        let region =
            parent_widget(&c["UI"], list_row["Properties"]["Id"].as_str().unwrap()).unwrap();
        assert_eq!(region["Properties"]["flex-direction"], "row");
        assert_eq!(
            region["Properties"]["flex-min-height"],
            list_row["Properties"]["flex-min-height"]
        );
        assert_eq!(notice_row["Properties"]["justify-content"], "center");
        assert_eq!(notice_row["Properties"]["flex-max-height"], "");
        assert_eq!(notice_row["Properties"]["flex-min-height"], "100px");
        for row in [list_row, notice_row] {
            assert!(c["DataBindings"].as_array().unwrap().iter().any(|b| {
                b["TargetId"] == row["Properties"]["Id"]
                    && b["PropertyMaps"][0]["TargetProperty"] == "Visible"
            }));
        }
        assert!(c
            .to_string()
            .contains("!!(rows && (rows.rows || rows).length)"));
    }
}

#[test]
fn command_recipe_flags_for_batch_2a_are_refused_on_other_recipes() {
    let dir = n7_fixture();
    let root = dir.path();
    n7_new(root, false, "blank");
    let grid = n7_add(root, false, "grid", "center");
    for (flags, why) in [
        (
            vec!["card", "--multiple", "--title", "T"],
            "--multiple is for recipe picker",
        ),
        (
            vec!["card", "--info", "x", "--title", "T"],
            "--info is for recipe side-panel",
        ),
    ] {
        let mut args = vec!["mashup", "recipe", flags[0], "Acme.Layout_MU", &grid];
        args.extend(&flags[1..]);
        let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .env_clear()
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{flags:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(why),
            "{flags:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn parent_widget<'a>(widget: &'a Value, id: &str) -> Option<&'a Value> {
    let children = widget["Widgets"].as_array()?;
    if children.iter().any(|w| w["Properties"]["Id"] == id) {
        return Some(widget);
    }
    children.iter().find_map(|w| parent_widget(w, id))
}

#[test]
fn command_dials_clear_the_placeholder_and_preserve_authored_captions() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "kpi-tile");
        let dial = n7_add(root, mcp, "kpi-dial", "value");
        assert_eq!(
            find_widget(&n7_content(root)["UI"], &dial).unwrap()["Properties"]["Label"],
            ""
        );
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "set",
                "Acme.Layout_MU",
                &dial,
                "--prop",
                "Label=Utilization",
                "--apply",
            ],
            "mashup_set",
            json!({"mashup":"Acme.Layout_MU","widget":dial,"properties":{"Label":"Utilization"},"dry_run":false}),
        );
        assert_eq!(
            find_widget(&n7_content(root)["UI"], &dial).unwrap()["Properties"]["Label"],
            "Utilization"
        );
    }
}

#[test]
fn command_titled_collapsible_filters_keep_the_kit_fields_column() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "filter-panel");
        n7_add(root, mcp, "label", "filters");
        let row = n7_add(root, mcp, "flexcontainer", "filters");
        let field = n7_add(root, mcp, "text-field", &row);
        n7_add(root, mcp, "datepicker", "filters");
        n7_add(root, mcp, "dropdown", "filters");
        n7_recipe(
            root,
            mcp,
            "collapsible",
            "filter-column",
            &["--tab", "inside"],
            json!({"tab":"inside"}),
        );
        let c = n7_content(root);
        assert_eq!(
            find_widget(&c["UI"], &field).unwrap()["Properties"]["Width"],
            240
        );
        let fields = named(&c["UI"], "filters").unwrap();
        assert!(!c.to_string().contains("--remaining-space"));
        // An authored row stays as written; no per-row pins or height estimates are generated.
        assert_eq!(fields["Properties"]["flex-direction"], "column");
        assert_eq!(fields["Properties"]["justify-content"], "flex-start");
        assert_eq!(fields["Properties"]["flex-min-height"], "");
        assert!(fields["Widgets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|child| child["Properties"]["Type"] == "flexcontainer"));
        assert_eq!(
            named(&c["UI"], "filter-footer").unwrap()["Properties"]["LastContainer"],
            true
        );
    }
}

#[test]
fn command_card_collection_starts_without_a_selected_card() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "blank");
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "new",
                "Acme.Cell_MU",
                "--template",
                "kpi-tile",
                "--param",
                "Name:STRING:IN",
                "--apply",
            ],
            "mashup_new",
            json!({"name":"Acme.Cell_MU","template":"kpi-tile","params":["Name:STRING:IN"],"dry_run":false}),
        );
        let list = n7_add(root, mcp, "list", "center");
        n7_recipe(
            root,
            mcp,
            "picker",
            &list,
            &["--data", "data:Acme.Manager.GetRows", "--x", "Severity"],
            json!({"data":"data:Acme.Manager.GetRows","x":"Severity"}),
        );
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "collection",
                "add",
                "Acme.Layout_MU",
                "Acme.Cell_MU",
                "--data",
                "data:Acme.Manager.GetRows",
                "--map",
                "Severity=Name",
                "--preset",
                "cards",
                "--below",
                &list,
                "--apply",
            ],
            "mashup_collection_add",
            json!({"mashup":"Acme.Layout_MU","cell_mashup":"Acme.Cell_MU","data":"data:Acme.Manager.GetRows","map":{"Severity":"Name"},"preset":"cards","below":list,"dry_run":false}),
        );
        let c = n7_content(root);
        assert_eq!(
            properties(&c["UI"], "ptcscollection").unwrap()["AutoSelectFirstRow"],
            false
        );
    }
}

#[test]
fn command_navigation_button_equivalent_keeps_compact_full_width_styling() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "blank");
        let button = n7_add(root, mcp, "button", "center");
        let styles = [
            "width=100%",
            "box-sizing=border-box",
            "border-radius=0px",
            "padding-left=24px",
            "label:font-size=14px",
            "label:font-weight=600",
            "label:color=#080909",
            "background=#e4e7e9",
        ];
        let mut args = vec![
            "mashup",
            "set",
            "Acme.Layout_MU",
            &button,
            "--prop",
            "Size=standard",
            "--prop",
            "ButtonType=transparent",
            "--prop",
            "LabelAlignment=left",
            "--prop",
            "Margin=0",
            "--apply",
        ];
        for style in styles {
            args.extend(["--style", style]);
        }
        invoke(
            root,
            mcp,
            &args,
            "mashup_set",
            json!({"mashup":"Acme.Layout_MU","widget":button,
            "properties":{"Size":"standard","ButtonType":"transparent","LabelAlignment":"left","Margin":"0"},
            "styles":styles,"dry_run":false}),
        );
        let c = n7_content(root);
        let style = &c["UI"]["Properties"]["StyleProperties"]["data"][&button]["parts"];
        assert_eq!(style["label"]["$"]["font-size"], "14px");
        assert_eq!(style["label"]["$"]["font-weight"], "600");
        assert_eq!(style["$"]["$"]["width"], "100%");
        assert_eq!(style["$"]["$"]["padding-left"], "24px");
        assert_eq!(style["$"]["$"]["background"], "#e4e7e9");
    }
}

#[test]
fn command_card_grid_section_and_input_variant_build_the_kit_gallery() {
    for mcp in [false, true] {
        let dir = n7_fixture();
        let root = dir.path();
        n7_new(root, mcp, "blank");
        let column = n7_add(root, mcp, "flexcontainer", "center");
        let result = n7_recipe(
            root,
            mcp,
            "card-grid",
            &column,
            &["--per-row", "2", "--cells", "3"],
            json!({"per_row": 2, "cells": 3}),
        );
        assert_eq!(result["applied"], true, "{result}");
        let c = n7_content(root);
        let gallery = find_widget(&c["UI"], &column).unwrap();
        let rows = gallery["Widgets"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert_eq!(row["Properties"]["flex-grow"], 1);
            assert_eq!(row["Properties"]["flex-basis"], "0px");
            let cells = row["Widgets"].as_array().unwrap();
            assert_eq!(cells.len(), 2);
            for cell in cells {
                assert_eq!(cell["Properties"]["flex-grow"], 1);
                assert_eq!(cell["Properties"]["flex-basis"], "0px");
            }
        }
        let cell = rows[0]["Widgets"][0]["Properties"]["Id"]
            .as_str()
            .unwrap()
            .to_string();
        let result = n7_recipe(
            root,
            mcp,
            "section",
            &cell,
            &["--title", "Standard Input", "--align", "center"],
            json!({"title": "Standard Input", "align": "center"}),
        );
        assert_eq!(result["applied"], true, "{result}");
        let c = n7_content(root);
        let section = find_widget(&c["UI"], &cell).unwrap();
        let [bar, body] = section["Widgets"].as_array().unwrap().as_slice() else {
            panic!("section has a title row and a body row: {section}");
        };
        assert_eq!(bar["Properties"]["flex-min-height"], "40px");
        assert_eq!(bar["Properties"]["flex-max-height"], "40px");
        assert_eq!(bar["Widgets"][0]["Properties"]["LabelType"], "sub-header");
        assert_eq!(
            bar["Widgets"][0]["Properties"]["LabelText"],
            "Standard Input"
        );
        assert_eq!(body["Properties"]["LastContainer"], true);
        assert_eq!(body["Properties"]["justify-content"], "center");
        assert_eq!(body["Properties"]["align-items"], "center");
        assert_eq!(
            c["UI"]["Properties"]["StyleProperties"]["data"][&cell]["parts"]["$"]["$"]
                ["border-style"],
            "solid"
        );
        let field = n7_add(
            root,
            mcp,
            "text-field",
            body["Properties"]["Id"].as_str().unwrap(),
        );
        let result = n7_recipe(
            root,
            mcp,
            "input-variant",
            &field,
            &["--variant", "focus-glow"],
            json!({"variant": "focus-glow"}),
        );
        assert_eq!(result["applied"], true, "{result}");
        let parts = &n7_content(root)["UI"]["Properties"]["StyleProperties"]["data"][&field]
            ["parts"]["text-box"];
        assert_eq!(parts["focus"]["border-color"], "#0088CE");
        assert!(parts["focus"]["box-shadow"]
            .as_str()
            .unwrap()
            .contains("#0088CE"));
        // A named colour in --style means the same as it does in a page spec.
        let label = n7_add(
            root,
            mcp,
            "label",
            body["Properties"]["Id"].as_str().unwrap(),
        );
        invoke(
            root,
            mcp,
            &[
                "mashup",
                "set",
                "Acme.Layout_MU",
                &label,
                "--style",
                "label:color=primary",
                "--style",
                "background=lime",
                "--apply",
            ],
            "mashup_set",
            json!({"mashup":"Acme.Layout_MU","widget":label,
                "styles":["label:color=primary","background=lime"],"dry_run":false}),
        );
        let style =
            &n7_content(root)["UI"]["Properties"]["StyleProperties"]["data"][&label]["parts"];
        assert_eq!(style["label"]["$"]["color"], "#0088CE");
        assert_eq!(style["$"]["$"]["background"], "#CCFF00");
    }
}

#[test]
fn command_gallery_recipes_refuse_unsafe_targets_and_other_recipes_flags() {
    let dir = n7_fixture();
    let root = dir.path();
    n7_new(root, false, "blank");
    let column = n7_add(root, false, "flexcontainer", "center");
    let field = n7_add(root, false, "text-field", &column);
    let run = |recipe: &str, widget: &str, flags: &[&str]| {
        let mut args = vec!["mashup", "recipe", recipe, "Acme.Layout_MU", widget];
        args.extend(flags);
        args.push("--apply");
        let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .env_clear()
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{recipe} {flags:?}");
        String::from_utf8_lossy(&output.stderr).to_string()
    };
    let other = n7_add(root, false, "flexcontainer", "center");
    for (recipe, widget, flags, why) in [
        (
            "card",
            &other,
            vec!["--title", "T", "--align", "center"],
            "--align is for recipe section",
        ),
        (
            "card",
            &other,
            vec!["--title", "T", "--per-row", "2"],
            "--per-row is for recipe card-grid",
        ),
        (
            "card",
            &other,
            vec!["--title", "T", "--cells", "2"],
            "--cells is for recipe card-grid",
        ),
        (
            "card",
            &other,
            vec!["--title", "T", "--variant", "rounded"],
            "--variant is for recipe input-variant",
        ),
        (
            "section",
            &other,
            vec!["--title", "T", "--align", "middle"],
            "--align is start or center",
        ),
        ("section", &other, vec![], "section needs --title"),
        (
            "card-grid",
            &other,
            vec!["--per-row", "2"],
            "card-grid needs --cells",
        ),
        (
            "card-grid",
            &other,
            vec!["--per-row", "0", "--cells", "2"],
            "--per-row is 1 to 12",
        ),
        (
            "card-grid",
            &column,
            vec!["--per-row", "2", "--cells", "2"],
            "needs an empty container",
        ),
        (
            "input-variant",
            &field,
            vec!["--variant", "glossy"],
            "--variant is rounded",
        ),
        (
            "input-variant",
            &other,
            vec!["--variant", "rounded"],
            "needs a text field",
        ),
    ] {
        let stderr = run(recipe, widget, &flags);
        assert!(stderr.contains(why), "{recipe} {flags:?}: {stderr}");
    }
}
