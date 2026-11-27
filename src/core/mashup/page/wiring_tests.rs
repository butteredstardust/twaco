use super::super::{derive, extract};
use super::*;

fn wiring_solution() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = element_solution();
    let definitions: String = ["Start", "Stop", "Reset", "GetStatus"]
        .iter()
        .map(|name| format!("<ServiceDefinition name=\"{name}\"><ResultType baseType=\"STRING\"/></ServiceDefinition>"))
        .collect();
    std::fs::write(solution.root.join("acme/Things/Acme.Actions.xml"), format!("<Entities><Things><Thing name=\"Acme.Actions\" projectName=\"Acme\"><ThingShape><ServiceDefinitions>{definitions}</ServiceDefinitions></ThingShape></Thing></Things></Entities>")).unwrap();
    (dir, solution)
}

fn specs() -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(fixtures().join("wiring"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".page.yaml"))
        .collect();
    paths.sort();
    assert_eq!(paths.len(), 4);
    paths
}

#[test]
fn wiring_and_rich_text_goldens() {
    let (_dir, solution) = wiring_solution();
    for path in specs() {
        let spec = std::fs::read_to_string(&path).unwrap();
        let built =
            compile(&solution, "Acme", &spec).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let actual = serde_json::to_string_pretty(&serde_json::json!({
            "entity": String::from_utf8(built.entity).unwrap(),
            "content": serde_json::from_str::<Value>(&built.content).unwrap(),
            "css": built.css, "ids": built.ids,
        }))
        .unwrap()
            + "\n";
        let golden = path.with_extension("golden.json");
        if std::env::var_os("TWACO_BLESS").is_some() {
            std::fs::write(&golden, &actual).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(golden).unwrap(),
            actual,
            "{}",
            path.display()
        );
    }
}

#[test]
fn chains_errors_multiple_bindings_and_rich_text_derive_and_rebuild() {
    let (_dir, solution) = wiring_solution();
    for path in specs() {
        let spec = std::fs::read_to_string(&path).unwrap();
        let built = compile(&solution, "Acme", &spec).unwrap();
        let derived = derive::derive("Acme.Rebuilt_MU", &built.content).unwrap();
        assert_eq!(derived.raw, 0, "{}", derived.spec);
        assert!(
            !derived.notes.iter().any(|n| n.contains("left out")),
            "{:?}",
            derived.notes
        );
        let rebuilt = compile(&solution, "Acme", &derived.spec)
            .unwrap_or_else(|e| panic!("{e}\n{}", derived.spec));
        assert_eq!(
            shape(&built.content),
            shape(&rebuilt.content),
            "{}\n{}",
            path.display(),
            derived.spec
        );
        let value: Value = serde_json::from_str(&built.content).unwrap();
        let (bindings, events) = match path.file_name().unwrap().to_str().unwrap() {
            "chain.page.yaml" => (0, 3),
            "status.page.yaml" => (1, 3),
            "multiple.page.yaml" => (3, 0),
            "rich_text.page.yaml" => (1, 1),
            _ => unreachable!(),
        };
        assert_eq!(value["DataBindings"].as_array().unwrap().len(), bindings);
        assert_eq!(value["Events"].as_array().unwrap().len(), events);
    }
}

#[test]
fn handmade_wiring_adopts_with_every_endpoint_and_id_kept() {
    let (_dir, solution) = wiring_solution();
    for path in specs() {
        let spec = std::fs::read_to_string(path).unwrap();
        let spec = spec.replace("  template: blank", "  template: blank\n  layout: as-built");
        let name = schema::parse(&spec).unwrap().page.value.name.value;
        super::adopt::handmade(&solution, &spec, &name);
        let before = super::adopt::current(&solution, &name);
        let derived = extract::extract(&solution, &name).unwrap();
        super::adopt::write_spec(&solution, &name, &derived.spec);
        let plan = super::super::build::plan(&solution, &name, None, None).unwrap();
        assert!(!plan.conflicts(), "{}\n{}", plan.report, derived.spec);
        super::adopt::apply(&solution, &name);
        assert_eq!(
            before,
            super::adopt::current(&solution, &name),
            "{}",
            derived.spec
        );
    }
}

#[test]
fn wiring_and_native_option_errors_point_to_the_authored_field() {
    let (_dir, solution) = wiring_solution();
    for (tail, message, line) in [
        (
            "  events:\n    - {on: missing.done, invoke: rows}\n",
            "unknown reference",
            9,
        ),
        (
            "  events:\n    - {on: rows.error, show: absent}\n",
            "unknown status_message",
            9,
        ),
        (
            "  events:\n    - {on: rows.error, show: output}\n",
            "status_message",
            9,
        ),
        (
            "  events:\n    - {on: rows.error, show: output, invoke: rows}\n",
            "exactly one",
            9,
        ),
        (
            "  bindings:\n    - {from: missing.result, to: output.LabelText}\n",
            "unknown reference",
            9,
        ),
        (
            "  bindings:\n    - {from: rows.Name, to: output.NoSuchProperty}\n",
            "NoSuchProperty",
            9,
        ),
    ] {
        let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Errors_MU\n  template: blank\n  data: {{rows: {{service: Acme.Data.GetRows}}}}\n  slots: {{center: [{{label: {{id: output}}}}]}}\n{tail}");
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.message.contains(message), "{error}");
        assert_eq!(error.line, line, "{error}");
        assert!(error.column > 0);
    }
    for (field, message) in [
        ("message_type: fatal", "message_type"),
        ("font_size: 15", "DefaultFontSize"),
    ] {
        let element = if field.starts_with("message") {
            "status_message"
        } else {
            "rich_text"
        };
        let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Errors_MU\n  template: blank\n  slots:\n    center:\n      - {element}:\n          id: target\n          {field}\n");
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.message.contains(message), "{error}");
        assert_eq!(error.line, 10, "{error}");
    }
}

#[test]
fn command_on_and_bind_emit_the_same_chains_errors_and_multiple_sources() {
    use authoring::session::Session;
    let (_dir, solution) = wiring_solution();
    let base = "version: 1\nrules: v1\npage:\n  name: Acme.Parity_MU\n  template: blank\n  data:\n    start: {service: Acme.Actions.Start}\n    stop: {service: Acme.Actions.Stop}\n    status: {service: Acme.Actions.GetStatus}\n  slots:\n    center:\n      - label: {id: output}\n      - status_message: {id: failed, message: Failed, message_type: error}\n";
    let built = compile(&solution, "Acme", base).unwrap();
    let mut command = Session::existing(
        "Acme.Parity_MU",
        built.entity,
        mashup::edit::parse(&built.content).unwrap(),
        built.css,
    );
    let (model, _) = crate::core::types::load_model(&solution);
    let platform = crate::core::types::cached_platform(&solution);
    for service in ["Start", "Stop"] {
        let alias = service.to_lowercase();
        command
            .on(
                &model,
                platform.as_ref(),
                &format!("data:Acme.Actions.{service}[as {alias}].done"),
                "data:Acme.Actions.GetStatus[as status]",
            )
            .unwrap();
        command
            .on(
                &model,
                platform.as_ref(),
                &format!("data:Acme.Actions.{service}[as {alias}].error"),
                &format!("ui:{}.ShowMessage", built.ids["failed"]),
            )
            .unwrap();
        command
            .bind(
                &solution,
                &model,
                platform.as_ref(),
                &format!("data:Acme.Actions.{service}[as {alias}].result"),
                &format!("ui:{}.LabelText", built.ids["output"]),
            )
            .unwrap();
    }
    let spec = format!("{base}  events:\n    - {{on: start.done, invoke: status}}\n    - {{on: stop.done, invoke: status}}\n    - {{on: start.error, show: failed}}\n    - {{on: stop.error, show: failed}}\n  bindings:\n    - {{from: start.result, to: output.LabelText}}\n    - {{from: stop.result, to: output.LabelText}}\n");
    let page = compile(&solution, "Acme", &spec).unwrap();
    assert_eq!(shape(&page.content), shape(&command.document.to_content()));
    for event in command.document.value()["Events"].as_array().unwrap() {
        assert!(matches!(
            event["EventTriggerEvent"].as_str(),
            Some("ServiceInvokeCompleted" | "Data_Error")
        ));
    }
}

#[test]
fn twenty_refreshes_five_additional_bindings_and_three_failures_survive_derive() {
    let (_dir, solution) = wiring_solution();
    let mut data = serde_json::Map::new();
    for n in 0..10 {
        data.insert(
            format!("write{n}"),
            serde_json::json!({"service": "Acme.Actions.Start"}),
        );
    }
    data.insert(
        "status".into(),
        serde_json::json!({"service": "Acme.Actions.GetStatus"}),
    );
    data.insert(
        "rows".into(),
        serde_json::json!({"service": "Acme.Data.GetRows"}),
    );
    let mut events = vec![];
    for n in 0..10 {
        for target in ["status", "rows"] {
            events.push(serde_json::json!({"on": format!("write{n}.done"), "invoke": target}));
        }
    }
    for n in 0..3 {
        events.push(serde_json::json!({"on": format!("write{n}.error"), "show": "failed"}));
    }
    let mut nodes = vec![
        serde_json::json!({"status_message": {"id": "failed", "message": "Failed", "message_type": "error"}}),
    ];
    let mut bindings = vec![];
    for (n, sources) in [2, 3, 3].into_iter().enumerate() {
        nodes.push(serde_json::json!({"label": {"id": format!("out{n}")}}));
        for source in 0..sources {
            bindings.push(serde_json::json!({"from": format!("write{source}.result"), "to": format!("out{n}.LabelText")}));
        }
    }
    let spec = serde_json::json!({"version": 1, "rules": "v1", "page": {
        "name": "Acme.Refreshes_MU", "template": "blank", "data": data,
        "slots": {"center": nodes}, "events": events, "bindings": bindings,
    }})
    .to_string();
    let built = compile(&solution, "Acme", &spec).unwrap();
    let derived = derive::derive("Acme.Refreshes_MU", &built.content).unwrap();
    assert_eq!(
        derived.value["page"]["bindings"].as_array().unwrap().len(),
        5
    );
    assert_eq!(
        derived.value["page"]["events"].as_array().unwrap().len(),
        23
    );
    let rebuilt = compile(&solution, "Acme", &derived.spec).unwrap();
    assert_eq!(
        shape(&built.content),
        shape(&rebuilt.content),
        "{}",
        derived.spec
    );
}

#[test]
fn rich_text_content_font_sizes_and_height_use_shared_add_and_set() {
    use authoring::session::Session;
    let (_dir, solution) = wiring_solution();
    let text = std::fs::read_to_string(fixtures().join("wiring/rich_text.page.yaml")).unwrap();
    let built = compile(&solution, "Acme", &text).unwrap();
    let document = mashup::edit::parse(&built.content).unwrap();
    let header = document.find_exact(&built.ids["header"]).unwrap();
    let props = &document.at(&header.path)["Properties"];
    assert_eq!(props["Data"], "<h2>Acme status</h2>");
    assert_eq!(props["DefaultFontSize"], "18px");
    assert_eq!(props["ReadOnly"], true);
    assert_eq!(props["HideToolbar"], true);
    assert_eq!(props["Height"], 56);
    let details = document.find_exact(&built.ids["details"]).unwrap();
    assert_eq!(
        document.at(&details.path)["Properties"]["DefaultFontSize"],
        "14px"
    );
    assert!(document.at(&details.path)["Properties"]["Height"].is_null());

    let mut command = Session::existing("Acme.RichText_MU", built.entity, document, built.css);
    command
        .set(
            &solution,
            &authoring::SetRequest {
                force: false,
                mashup: "Acme.RichText_MU".into(),
                widget: built.ids["header"].clone(),
                properties: vec![
                    ("Data".into(), serde_json::json!("<p>Edited</p>")),
                    ("DefaultFontSize".into(), serde_json::json!(24)),
                ],
                style_from: None,
                styles: vec![],
            },
        )
        .unwrap();
    let header = command.document.find_exact(&built.ids["header"]).unwrap();
    assert_eq!(
        command.document.at(&header.path)["Properties"]["Data"],
        "<p>Edited</p>"
    );
    assert_eq!(
        command.document.at(&header.path)["Properties"]["DefaultFontSize"],
        "24px"
    );

    let mut numeric: Value = serde_json::from_str(&built.content).unwrap();
    fn number(widget: &mut Value) {
        if widget["Properties"]["Type"] == "ptcsrte" {
            widget["Properties"]["DefaultFontSize"] = serde_json::json!(14);
        }
        for child in widget["Widgets"].as_array_mut().into_iter().flatten() {
            number(child);
        }
    }
    number(&mut numeric["UI"]);
    let derived = derive::derive("Acme.RichText_MU", &numeric.to_string()).unwrap();
    assert!(derived.spec.contains("rich_text:"), "{}", derived.spec);
    assert!(derived.spec.contains("font_size: 14"), "{}", derived.spec);
    compile(&solution, "Acme", &derived.spec).unwrap();
}
