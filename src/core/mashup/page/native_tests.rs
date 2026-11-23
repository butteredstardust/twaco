use super::*;

fn native_solution() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = element_solution();
    let child = compile(&solution, "Acme", "version: 1\nrules: v1\npage:\n  name: Acme.Child_MU\n  template: blank\n  parameters: {id: STRING}\n").unwrap();
    std::fs::write(
        solution.root.join("acme/Mashups/Acme.Child_MU.xml"),
        child.entity,
    )
    .unwrap();
    (dir, solution)
}

#[test]
fn native_goldens_and_derive_round_trips() {
    let (_dir, solution) = native_solution();
    let folder = fixtures().join("native");
    let mut paths: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".page.yaml"))
        .collect();
    paths.sort();
    assert_eq!(paths.len(), 23);
    for path in paths {
        let text = std::fs::read_to_string(&path).unwrap();
        let built =
            compile(&solution, "Acme", &text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            built,
            compile(&solution, "Acme", &text).unwrap(),
            "{}",
            path.display()
        );
        if path.file_name().unwrap() == "value_literal.page.yaml" {
            let content: Value = serde_json::from_str(&built.content).unwrap();
            assert!(content["Events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["EventTriggerArea"] == "Mashup"
                    && e["EventTriggerEvent"] == "Loaded"
                    && e["EventHandlerId"] == built.ids["target--value"]
                    && e["EventHandlerService"] == "Evaluate"));
        }
        let actual = serde_json::to_string_pretty(&serde_json::json!({"entity": String::from_utf8(built.entity.clone()).unwrap(), "content": serde_json::from_str::<Value>(&built.content).unwrap(), "css": built.css, "ids": built.ids})).unwrap() + "\n";
        let golden = path.with_extension("golden.json");
        if std::env::var_os("TWACO_BLESS").is_some() {
            std::fs::write(&golden, &actual).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&golden).unwrap(),
            actual,
            "{}",
            path.display()
        );
        let derived = super::super::derive::derive("Acme.Native_MU", &built.content).unwrap();
        assert_eq!(
            derived.raw,
            0,
            "{}: {:?}\n{}",
            path.display(),
            derived.notes,
            derived.spec
        );
        let rebuilt = compile(&solution, "Acme", &derived.spec)
            .unwrap_or_else(|e| panic!("{}: {e}\n{}", path.display(), derived.spec));
        assert_eq!(
            shape(&built.content),
            shape(&rebuilt.content),
            "{}\n{}",
            path.display(),
            derived.spec
        );
    }
}

#[test]
fn native_fields_match_command_properties() {
    use authoring::session::Session;
    let (_dir, solution) = native_solution();
    for (element, widget, properties) in [
        (
            "status_message: {id: target, message: Failed, message_type: error}",
            "statusmessage",
            vec![("Message", "Failed"), ("MessageType", "error")],
        ),
        (
            "rich_text: {id: target, content: '<h2>Acme</h2>', read_only: true, show_toolbar: false, font_size: 18}",
            "ptcsrte",
            vec![("Data", "<h2>Acme</h2>"), ("ReadOnly", "true"), ("HideToolbar", "true"), ("DefaultFontSize", "18")],
        ),
        (
            "events_router: {id: target, input_count: 3, data_type: BOOLEAN}",
            "eventsrouter",
            vec![("NumberOfInputs", "3"), ("DataType", "BOOLEAN")],
        ),
        (
            "link: {id: target, text: Details, url: /acme, target: _blank}",
            "ptcslink",
            vec![
                ("Text", "Details"),
                ("LinkURL", "/acme"),
                ("TargetWindow", "_blank"),
            ],
        ),
        (
            "image: {id: target, source: Acme.Logo_MD, scaling: contain, alt: Logo}",
            "ptcsimage",
            vec![
                ("SourceURL", "/Thingworx/MediaEntities/Acme.Logo_MD"),
                ("Scaling", "contain"),
                ("AlternateText", "Logo"),
            ],
        ),
        (
            "icon: {id: target, icon: info, size: lg}",
            "ptcsicon",
            vec![("SVGIcon", "cds:icon_info"), ("Size", "large")],
        ),
        (
            "divider: {id: target, orientation: vertical}",
            "ptcsdivider",
            vec![("VerticalDivider", "true")],
        ),
        (
            "checkbox: {id: target, label: Include, value: true}",
            "ptcscheckbox",
            vec![("Label", "Include"), ("State", "true")],
        ),
        (
            "radio: {id: target, label: Active, group: Acme, value: true}",
            "ptcsradio",
            vec![
                ("Label", "Active"),
                ("Radiogroup", "Acme"),
                ("State", "true"),
            ],
        ),
        (
            "toggle: {id: target, label: Enabled, value: true}",
            "ptcstogglebutton",
            vec![("Label", "Enabled"), ("State", "true")],
        ),
        (
            "textarea: {id: target, label: Notes, hint: Enter, value: Note, max_length: 500}",
            "ptcstextarea",
            vec![
                ("Label", "Notes"),
                ("HintText", "Enter"),
                ("Text", "Note"),
                ("MaxNumberOfCharacters", "500"),
            ],
        ),
        (
            "date_picker: {id: target, label: As of, date_only: false, format: YYYY}",
            "ptcsdatepicker",
            vec![
                ("Label", "As of"),
                ("DateRange", "false"),
                ("DateOnly", "false"),
                ("FormatToken", "YYYY"),
            ],
        ),
        (
            "pagination: {id: target, mode: server, page_size: 25, page_number: 2}",
            "ptcspagination",
            vec![("PageSize", "25"), ("PageNumber", "2")],
        ),
    ] {
        let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Native_MU\n  template: blank\n  slots:\n    center:\n      - {element}\n");
        let built = compile(&solution, "Acme", &text).unwrap();
        let mut command =
            Session::new("Acme.Native_MU", "Acme", "blank", "PTC Convergence Theme").unwrap();
        let placed = command
            .add(
                &solution,
                &authoring::AddRequest {
                    mashup: "Acme.Native_MU".into(),
                    widget: widget.into(),
                    inputs: vec![],
                    placement: if matches!(widget, "eventsrouter" | "statusmessage") {
                        None
                    } else {
                        Some(mashup::edit::Placement::In("center".into()))
                    },
                    label: None,
                    properties: properties
                        .into_iter()
                        .map(|(k, v)| (k.into(), v.into()))
                        .collect(),
                },
            )
            .unwrap();
        let document = mashup::edit::parse(&built.content).unwrap();
        let spec_path = document.find_exact(&built.ids["target"]).unwrap().path;
        let command_path = command.document.find_exact(&placed).unwrap().path;
        let mut spec = document.at(&spec_path)["Properties"].clone();
        let mut primitive = command.document.at(&command_path)["Properties"].clone();
        for value in [&mut spec, &mut primitive] {
            for key in ["Id", "DisplayName"] {
                value.as_object_mut().unwrap().remove(key);
            }
        }
        assert_eq!(spec, primitive, "{element}");
    }
}

#[test]
fn bad_fields_and_contracts_are_located() {
    let (_dir, solution) = native_solution();
    for (body, message) in [
        ("events_router: {id: target, input_count: 0}", "input_count"),
        ("events_router: {id: target, routes: [42]}", "routes"),
        ("image: {id: target, scaling: stretch}", "scaling"),
        ("icon: {id: target, icon_size: enormous}", "icon_size"),
        (
            "divider: {id: target, orientation: diagonal}",
            "orientation",
        ),
        ("checkbox: {id: target, url: /acme}", "url"),
        ("link: {id: target, target: elsewhere}", "target"),
        (
            "link: {id: target, url: /acme, mashup: Acme.Child_MU}",
            "url or mashup",
        ),
        (
            "textarea: {id: target, orientation: vertical}",
            "orientation",
        ),
        ("date_picker: {id: target, group: Acme}", "group"),
        ("value_display: {id: target, max_length: 500}", "max_length"),
        ("embed: {id: target}", "embed needs mashup"),
        ("radio: {id: target, options: []}", "nonempty list"),
        (
            "pagination: {id: target, mode: server, page_size: 0}",
            "page_size",
        ),
        (
            "chart: {id: target, kind: schedule, data: rows, x: Name}",
            "nested resource intervals",
        ),
    ] {
        let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Native_MU\n  template: blank\n  slots:\n    center:\n      - {body}\n");
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.message.contains(message), "{body}: {error}");
        assert!(error.line >= 8 && error.column > 0, "{body}: {error}");
    }
    for data in [
        "{expression: '1'}",
        "{expression: '1', output: WRONG}",
        "{expression: '1', output: NUMBER, service: Acme.Data.GetRows}",
        "{service: Acme.Data.GetRows, output: NUMBER}",
    ] {
        let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Native_MU\n  template: blank\n  data:\n    bad: {data}\n");
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.line >= 7 && error.column > 0, "{error}");
    }
}

#[test]
fn bindable_native_fields_keep_their_platform_addresses() {
    let (_dir, solution) = native_solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Bindings_MU\n  template: blank\n  parameters: {Text: STRING, Image: IMAGELINK, Flag: BOOLEAN, When: DATETIME}\n  slots:\n    center:\n      - link: {id: link, text: {bind: param.Text}, url: {bind: param.Text}}\n      - image: {id: image, source: {bind: param.Image}, alt: {bind: param.Text}}\n      - icon: {id: icon, source: {bind: param.Image}, alt: {bind: param.Text}}\n      - checkbox: {id: check, label: {bind: param.Text}, value: {bind: param.Flag}}\n      - radio: {id: radio, group: {bind: param.Text}, value: {bind: param.Flag}}\n      - toggle: {id: toggle, value: {bind: param.Flag}}\n      - textarea: {id: notes, value: {bind: param.Text}}\n      - date_picker: {id: when, value: {bind: param.When}}\n      - value_display: {id: value, value: {bind: param.Text}}\n";
    let built = compile(&solution, "Acme", text).unwrap();
    let value: Value = serde_json::from_str(&built.content).unwrap();
    let bindings = value["DataBindings"].as_array().unwrap();
    for (node, property) in [
        ("link", "Text"),
        ("link", "LinkURL"),
        ("image", "SourceURL"),
        ("image", "AlternateText"),
        ("icon", "SourceURL"),
        ("icon", "AlternateText"),
        ("check", "Label"),
        ("check", "State"),
        ("radio", "Radiogroup"),
        ("radio", "State"),
        ("toggle", "State"),
        ("notes", "Text"),
        ("when", "DateTime"),
        ("value", "Data"),
    ] {
        assert!(
            bindings.iter().any(|b| b["TargetId"] == built.ids[node]
                && b["PropertyMaps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["TargetProperty"] == property)),
            "{node}.{property}"
        );
    }
    let derived = super::super::derive::derive("Acme.Bindings_MU", &built.content).unwrap();
    let rebuilt = compile(&solution, "Acme", &derived.spec).unwrap();
    assert_eq!(shape(&built.content), shape(&rebuilt.content));
}

#[test]
fn expression_types_evaluation_and_forward_bindings_are_preserved() {
    let (_dir, solution) = native_solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Expressions_MU\n  template: blank\n  data:\n    first: {expression: 'Count > 3', inputs: {Count: {type: NUMBER, value: 4}}, output: BOOLEAN, auto_evaluate: false, fire_on_first_value: true, data_change: VALUE}\n    second: {expression: '!Flag', inputs: {Flag: first}, output: BOOLEAN}\n  slots:\n    center:\n      - checkbox: {id: result, value: {bind: second}}\n      - button: {id: run, text: Evaluate}\n  events:\n    - {on: run.Clicked, invoke: first}\n";
    let built = compile(&solution, "Acme", text).unwrap();
    let document = mashup::edit::parse(&built.content).unwrap();
    let first = document.find_exact(&built.ids["$data:first"]).unwrap();
    let props = &document.at(&first.path)["Properties"];
    assert_eq!(props["OutputBaseType"], "BOOLEAN");
    assert_eq!(props["AutoEvaluate"], false);
    assert_eq!(props["FireOnFirstValue"], true);
    assert_eq!(props["DataChangeType"], "VALUE");
    assert_eq!(props["Count"].as_f64(), Some(4.0));
    assert_eq!(props["_currentParameterDefs"][0]["BaseType"], "NUMBER");
    let derived = super::super::derive::derive("Acme.Expressions_MU", &built.content).unwrap();
    let rebuilt = compile(&solution, "Acme", &derived.spec).unwrap();
    assert_eq!(shape(&built.content), shape(&rebuilt.content));
    let error = compile(&solution, "Acme", &text.replace("value: 4", "value: wrong")).unwrap_err();
    assert!(error.line == 7 && error.column > 0, "{error}");
}

#[test]
fn navigation_refuses_unknown_parameters_and_bad_modes() {
    let (_dir, solution) = native_solution();
    let text = std::fs::read_to_string(fixtures().join("native/navigate.page.yaml")).unwrap();
    for (text, needle) in [
        (text.replace("mode: popup", "mode: sideways"), "mode"),
        (text.replace("params: {id:", "params: {missing:"), "missing"),
        (
            text.replace("Acme.Child_MU", "Acme.Missing_MU"),
            "Acme.Missing_MU",
        ),
    ] {
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.message.contains(needle), "{error}");
        assert!(error.line >= 13 && error.column > 0, "{error}");
    }
}

#[test]
fn kit_variant_properties_and_deliberate_raw_reasons() {
    let (_dir, solution) = native_solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Variants_MU\n  template: blank\n  slots:\n    center:\n      - widget: {id: select, type: ptcsdropdown, props: {HintText: Category}}\n      - widget: {id: dial, type: ptcskpidial, props: {Layout: horizontal, Direction: 'left to right', StartAngle: 15, EndAngle: 280, Value: 12}}\n      - widget: {id: grid, type: ptcsgrid}\n      - widget: {id: pages, type: ptcspagination, props: {PageSize: 20, PageNumber: 3}}\n      - widget: {id: tabs, type: ptcstabset, props: {NumberOfTabs: 2, Items: [{name: Summary, value: summary}, {name: Details, value: details}]}}\n";
    let built = compile(&solution, "Acme", text).unwrap();
    let derived = super::super::derive::derive("Acme.Variants_MU", &built.content).unwrap();
    assert_eq!(derived.raw, 0, "{}", derived.spec);
    for needle in [
        "dropdown:",
        "unbound: true",
        "kpi_dial:",
        "kind: horizontal",
        "StartAngle: 15",
        "grid:",
        "mode: server",
        "label: Summary",
        "label: Details",
    ] {
        assert!(derived.spec.contains(needle), "{needle}\n{}", derived.spec);
    }
    let rebuilt = compile(&solution, "Acme", &derived.spec).unwrap();
    assert_eq!(shape(&built.content), shape(&rebuilt.content));

    let content = serde_json::json!({"UI": {"Properties": {}, "Widgets": [
        {"Properties": {"Id": "shape-1", "Type": "shape"}},
        {"Properties": {"Id": "googlemap-2", "Type": "googlemap"}},
        {"Properties": {"Id": "ptcsdynamicpanel-3", "Type": "ptcsdynamicpanel"}},
        {"Properties": {"Id": "ptcsmenubar-4", "Type": "ptcsmenubar"}},
        {"Properties": {"Id": "eventsrouter-5", "Type": "eventsrouter", "NumberOfInputs": "0", "DataType": "BOOLEAN"}},
        {"Properties": {"Id": "navigationfunction-6", "Type": "navigationfunction", "TargetMashup": ""}}
    ]}, "Data": {}, "DataBindings": [], "Events": []});
    let derived = super::super::derive::derive("Acme.Raw_MU", &content.to_string()).unwrap();
    assert_eq!(derived.raw, 5);
    assert!(derived.spec.contains("menu_bar:"));
    for kind in [
        "shape",
        "googlemap",
        "ptcsdynamicpanel",
        "eventsrouter",
        "navigationfunction",
    ] {
        assert!(
            derived
                .notes
                .iter()
                .any(|n| n.starts_with(&format!("{kind}:"))),
            "{kind}: {:?}",
            derived.notes
        );
    }
}

#[test]
fn navigation_and_expressions_can_read_the_selected_grid_field() {
    let (_dir, solution) = native_solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Selected_MU\n  template: blank\n  data:\n    rows: {service: Acme.Data.GetRows}\n    late: {expression: 'Count > 3', inputs: {Count: grid.selected.Count}, output: BOOLEAN}\n  slots:\n    center:\n      - grid: {id: grid, data: rows}\n      - button: {id: open_btn, text: Open}\n  events:\n    - {on: open_btn.Clicked, navigate: {mashup: Acme.Child_MU, mode: popup, params: {id: grid.SelectedRow.Name}}}\n";
    let built = compile(&solution, "Acme", text).unwrap();
    assert!(built.ids.contains_key("grid.selected.Name"));
    assert!(built.ids.contains_key("grid.selected.Count"));
    let value: Value = serde_json::from_str(&built.content).unwrap();
    let binding = value["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| {
            b["SourceId"] == built.ids["grid.selected.Name"]
                && b["PropertyMaps"][0]["TargetProperty"] == "id"
        })
        .unwrap();
    assert_eq!(binding["PropertyMaps"][0]["SourceProperty"], "Output");
}

#[test]
fn build_extract_and_rebuild_keep_the_authored_spec_exact() {
    let (_dir, solution) = native_solution();
    let text = std::fs::read_to_string(fixtures().join("native/expression.page.yaml")).unwrap();
    let path = solution.root.join("native.page.yaml");
    std::fs::write(&path, &text).unwrap();
    let mut plan =
        super::super::build::plan(&solution, "Acme.Native_MU", Some("Acme"), Some(&path)).unwrap();
    let guard = lock::acquire_for(&solution, "native page test").unwrap();
    plan.apply(&solution, &guard).unwrap();
    drop(guard);
    let extracted = super::super::extract::extract(&solution, "Acme.Native_MU").unwrap();
    assert!(extracted.exact);
    assert_eq!(extracted.spec, text);
    assert_eq!(extracted.raw, 0);
    assert!(extracted.elements >= 2);
    let plan = super::super::build::plan(&solution, "Acme.Native_MU", None, None).unwrap();
    assert_eq!(plan.report["files"], serde_json::json!([]));
}

#[test]
fn function_and_embed_paths_match_command_planners() {
    use authoring::{session::Session, AddRequest, Recipe};
    let (_dir, solution) = native_solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Commands_MU\n  template: blank\n  parameters: {id: STRING, Count: NUMBER}\n  data:\n    late: {expression: 'Count > 3', inputs: {Count: param.Count}, output: BOOLEAN}\n  slots:\n    center:\n      - checkbox: {id: late_flag, value: {bind: late}}\n      - button: {id: open, text: Open}\n      - embed: {id: detail, mashup: Acme.Child_MU, params: {id: {bind: param.id}}}\n  events:\n    - {on: open.Clicked, navigate: {mashup: Acme.Child_MU, mode: popup, params: {id: {bind: param.id}}}}\n";
    let built = compile(&solution, "Acme", text).unwrap();
    let mut command =
        Session::new("Acme.Commands_MU", "Acme", "blank", "PTC Convergence Theme").unwrap();
    command.add_parameter("id:STRING:IN").unwrap();
    command.add_parameter("Count:NUMBER:IN").unwrap();
    let check = command
        .add(
            &solution,
            &AddRequest {
                mashup: "Acme.Commands_MU".into(),
                widget: "ptcscheckbox".into(),
                inputs: vec![],
                placement: Some(mashup::edit::Placement::In("center".into())),
                label: None,
                properties: vec![],
            },
        )
        .unwrap();
    let button = command
        .add(
            &solution,
            &AddRequest {
                mashup: "Acme.Commands_MU".into(),
                widget: "ptcsbutton".into(),
                inputs: vec![],
                placement: Some(mashup::edit::Placement::In("center".into())),
                label: Some("Open".into()),
                properties: vec![],
            },
        )
        .unwrap();
    let child = command
        .contained(&solution, "center", "Acme.Child_MU")
        .unwrap();
    let expression = command
        .add(
            &solution,
            &AddRequest {
                mashup: "Acme.Commands_MU".into(),
                widget: "expression2".into(),
                inputs: vec!["Count:NUMBER".into()],
                placement: None,
                label: None,
                properties: vec![
                    ("Expression".into(), "result = (Count > 3);".into()),
                    ("OutputBaseType".into(), "BOOLEAN".into()),
                    ("AutoEvaluate".into(), "true".into()),
                    ("FireOnFirstValue".into(), "false".into()),
                ],
            },
        )
        .unwrap();
    let (model, _) = crate::core::types::load_model(&solution);
    for (from, to) in [
        ("mashup:Count".to_string(), format!("ui:{expression}.Count")),
        (
            format!("ui:{expression}.Output"),
            format!("ui:{check}.State"),
        ),
        ("mashup:id".into(), format!("ui:{child}.id")),
    ] {
        command.bind(&solution, &model, None, &from, &to).unwrap();
    }
    command
        .recipe(
            &solution,
            Recipe::Popup {
                trigger: format!("ui:{button}.Clicked"),
                target: "Acme.Child_MU".into(),
                window: "popup".into(),
                size: None,
                title: None,
                pass: vec![("id".into(), "mashup:id".into())],
                refresh: None,
            },
        )
        .unwrap();
    assert_eq!(shape(&built.content), shape(&command.document.to_content()));
}

#[test]
fn chart_raw_fallback_keeps_the_data_binding() {
    let content = serde_json::json!({
        "UI": {"Properties": {}, "Widgets": [{"Properties": {"Id": "ptcschartpie-1", "Type": "ptcschartpie", "DataField": "Name"}}]},
        "Data": {"Things_Acme.Data": {"EntityName": "Acme.Data", "EntityType": "Things", "Services": [{"Name": "GetRows", "Target": "GetRows"}]}},
        "DataBindings": [{"SourceArea": "Data", "SourceId": "GetRows", "SourceSection": "Things_Acme.Data", "TargetArea": "UI", "TargetId": "ptcschartpie-1", "PropertyMaps": [{"SourceProperty": "", "TargetProperty": "Data"}]}], "Events": []
    });
    let derived =
        super::super::derive::derive("Acme.Unconfigured_MU", &content.to_string()).unwrap();
    assert_eq!(derived.raw_types.get("ptcschartpie"), Some(&1));
    assert!(
        derived.spec.contains("Data: {bind: GetRows}"),
        "{}",
        derived.spec
    );
    assert!(derived.notes.iter().any(|s| s.starts_with("ptcschartpie:")));
}
