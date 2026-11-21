use super::{compile::compile, schema};
use crate::core::{
    config::Solution,
    lock,
    mashup::{self, authoring},
};
use serde_json::Value;

const SPEC: &str = "version: 1\nrules: v1\npage:\n  name: Acme.Alerts_MU\n  template: list-page\n  title: Alerts\n  data:\n    alerts: {service: Acme.Data.GetAlerts, on: load, params: {limit: 10}}\n  slots:\n    toolbar:\n      - button: {id: refresh, text: Refresh}\n    body:\n      - grid:\n          id: alerts_grid\n          data: alerts\n          columns:\n            - {field: Name, title: Device, width: 180}\n  events:\n    - {on: refresh.Clicked, invoke: alerts}\n";

fn solution() -> (tempfile::TempDir, Solution) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("twaco.toml"),
        "[[project]]\nname = 'Acme'\nroot = 'acme'\n",
    )
    .unwrap();
    let folder = dir.path().join("acme/Things");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Acme.Data.xml"), r#"<Entities><Things><Thing name="Acme.Data" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetAlerts"><ParameterDefinitions><FieldDefinition name="limit" baseType="INTEGER"/></ParameterDefinitions><ResultType baseType="INFOTABLE"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#).unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution)
}

#[test]
fn schema_reports_unknown_fields_duplicate_keys_and_ids_at_the_spec() {
    for (text, message, line) in [
        (
            SPEC.replace("version: 1", "version: 2"),
            "version must be 1",
            1,
        ),
        (
            SPEC.replace("rules: v1", "rules: legacy"),
            "rules must be v1",
            2,
        ),
        (
            SPEC.replace("  title: Alerts", "  mystery: Alerts"),
            "unknown field",
            6,
        ),
        (
            SPEC.replace("  title: Alerts", "  title: Alerts\n  title: Again"),
            "duplicate",
            7,
        ),
        (
            SPEC.replace("id: alerts_grid", "id: refresh"),
            "duplicate id",
            14,
        ),
        (
            SPEC.replace("data: alerts", "data: missing"),
            "unknown data",
            15,
        ),
    ] {
        let error = schema::parse(&text).unwrap_err();
        assert!(error.message.contains(message), "{error}");
        assert_eq!(error.line, line, "{error}");
        assert!(error.column > 0, "{error}");
    }
}

#[test]
fn compile_is_byte_identical_and_only_returns_artifacts() {
    let (dir, solution) = solution();
    let before = crate::core::workspace::entities(&solution).len();
    let first = compile(&solution, "Acme", SPEC).unwrap();
    assert_eq!(first, compile(&solution, "Acme", SPEC).unwrap());
    assert_eq!(before, crate::core::workspace::entities(&solution).len());
    assert!(!dir.path().join("src").exists());
    assert_eq!(
        mashup::extract(&first.entity).unwrap().content,
        first.content
    );
    assert!(!first.content.contains('\r'));
    assert!(!first.entity.contains(&b'\r'));
    let content: Value = serde_json::from_str(&first.content).unwrap();
    assert_eq!(content["Events"].as_array().unwrap().len(), 2);
    assert_eq!(content["DataBindings"].as_array().unwrap().len(), 1);
    assert_eq!(
        content["Data"]["Things_Acme.Data"]["Services"][0]["Parameters"]["limit"],
        10
    );
}

#[test]
fn unrelated_nodes_leave_ids_and_wiring_uuids_stable() {
    let (_dir, solution) = solution();
    let first = compile(&solution, "Acme", SPEC).unwrap();
    let second = compile(
        &solution,
        "Acme",
        &SPEC.replace(
            "    toolbar:",
            "    title:\n      - label: {id: subtitle, text: Details}\n    toolbar:",
        ),
    )
    .unwrap();
    for (node, id) in &first.ids {
        assert_eq!(Some(id), second.ids.get(node));
    }
    let a: Value = serde_json::from_str(&first.content).unwrap();
    let b: Value = serde_json::from_str(&second.content).unwrap();
    for key in ["Data", "DataBindings", "Events"] {
        assert_eq!(a[key], b[key], "{key}");
    }
}

#[test]
fn invalid_symbolic_references_and_contracts_have_locations() {
    let (_dir, solution) = solution();
    for (text, message) in [
        (
            SPEC.replace("refresh.Clicked", "missing.Clicked"),
            "unknown reference",
        ),
        (
            SPEC.replace("refresh.Clicked", "refresh.Missing"),
            "Missing",
        ),
        (
            SPEC.replace("invoke: alerts", "invoke: missing"),
            "unknown reference",
        ),
        (SPEC.replace("GetAlerts", "Missing"), "Missing"),
        (SPEC.replace("on: load", "on: every 0s"), "refresh"),
    ] {
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.message.contains(message), "{error}");
        assert!(error.line > 0 && error.column > 0, "{error}");
    }
}

#[test]
fn nested_raw_widgets_styles_explicit_size_and_bindings_use_authoring() {
    let (_dir, solution) = solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Nested_MU\n  template: blank\n  slots:\n    center:\n      - widget:\n          id: panel\n          type: flexcontainer\n          surface: true\n          size: fit\n          children:\n            - heading: {id: heading, text: Details, style: ['color=#123456']}\n            - widget: {id: input, type: ptcstextfield, props: {Text: Acme}}\n            - label: {id: value, props: {LabelText: {bind: input.Text}}}\n";
    let first = compile(&solution, "Acme", text).unwrap();
    assert_eq!(first, compile(&solution, "Acme", text).unwrap());
    let content: Value = serde_json::from_str(&first.content).unwrap();
    assert_eq!(content["DataBindings"].as_array().unwrap().len(), 1);
    assert!(
        content["UI"]["Properties"]["StyleProperties"]["data"][&first.ids["panel"]].is_object()
    );
}

#[test]
fn source_refresh_and_node_triggers_compile() {
    let (_dir, solution) = solution();
    for on in ["every 30s", "refresh.Clicked"] {
        let text = SPEC.replace("on: load", &format!("on: {on}")).replace(
            "  events:\n    - {on: refresh.Clicked, invoke: alerts}\n",
            "",
        );
        let artifacts = compile(&solution, "Acme", &text).unwrap();
        let value: Value = serde_json::from_str(&artifacts.content).unwrap();
        if on == "every 30s" {
            assert_eq!(
                value["Data"]["Things_Acme.Data"]["Services"][0]["RefreshInterval"],
                30
            );
        } else {
            assert_eq!(value["Events"].as_array().unwrap().len(), 1);
        }
    }
}

fn normalized(content: &str) -> Value {
    fn clean(v: &mut Value) {
        match v {
            Value::Object(map) => {
                for key in [
                    "Id",
                    "id_index",
                    "twacoPageRules",
                    "SourceId",
                    "TargetId",
                    "EventTriggerId",
                    "EventHandlerId",
                    "DisplayName",
                ] {
                    map.remove(key);
                }
                for v in map.values_mut() {
                    clean(v);
                }
            }
            Value::Array(a) => {
                for v in a {
                    clean(v);
                }
            }
            _ => {}
        }
    }
    let mut value: Value = serde_json::from_str(content).unwrap();
    clean(&mut value);
    value
}

#[test]
fn spec_matches_new_add_set_command_structure() {
    let (_dir, solution) = solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Simple_MU\n  template: blank\n  slots:\n    center:\n      - label: {id: greeting, text: Hello, props: {LabelType: header}}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let request = authoring::NewRequest {
        name: "Acme.Simple_MU".into(),
        project: None,
        template: "blank".into(),
        gadget: false,
        layout: None,
        primary_menu: None,
        menu_entity: None,
        secondary_menu: None,
        theme: None,
        master: false,
        visibility: vec![],
        params: vec![],
        tabs: vec![],
        from: None,
        replacements: vec![],
    };
    let plan = authoring::plan_new(&solution, &request).unwrap();
    let guard = lock::acquire_for(&solution, "page test").unwrap();
    authoring::apply_new(&solution, &plan, &guard).unwrap();
    let add = authoring::plan_add(
        &solution,
        &authoring::AddRequest {
            mashup: request.name,
            widget: "ptcslabel".into(),
            inputs: vec![],
            placement: Some(mashup::edit::Placement::In("center".into())),
            label: Some("Hello".into()),
            properties: vec![("LabelType".into(), "header".into())],
        },
    )
    .unwrap();
    let expected = mashup::extract(&add.edit.entity_after).unwrap();
    assert_eq!(normalized(&built.content), normalized(&expected.content));
}

#[test]
fn status_columns_share_the_existing_state_definition_recipe() {
    let (dir, solution) = solution();
    let folder = dir.path().join("acme/StateDefinitions");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Acme.Status.xml"), r##"<Entities><StateDefinitions><StateDefinition name="Acme.Status" projectName="Acme"><content><![CDATA[{"stateType":"string","stateDefinitions":[{"displayString":"Ready","defaultValue":"ready","defaultStyleDefinition":{"backgroundColor":"#00aa00"}},{"displayString":"Other","defaultStyleDefinition":{"backgroundColor":"#aaaaaa"}}]}]]></content></StateDefinition></StateDefinitions></Entities>"##).unwrap();
    let text = SPEC.replace(
        "{field: Name, title: Device, width: 180}",
        "{field: Name, status: Acme.Status}\n            - {field: State, status: Acme.Status}",
    );
    let built = compile(&solution, "Acme", &text).unwrap();
    assert_eq!(built, compile(&solution, "Acme", &text).unwrap());
    let document = mashup::edit::parse(&built.content).unwrap();
    let grid = document.find_exact(&built.ids["alerts_grid"]).unwrap();
    let columns = document.at(&grid.path)["Properties"]["ColumnFormat"]["formatInfo"]
        .as_array()
        .unwrap();
    assert_eq!(columns.len(), 2);
    for column in columns {
        assert_eq!(
            column["FormatOptions"]["formatInfo"]["StateDefinition"],
            "Acme.Status"
        );
    }
    let error = compile(
        &solution,
        "Acme",
        &text.replace("Acme.Status", "Acme.Missing"),
    )
    .unwrap_err();
    assert!(error.message.contains("StateDefinition"));
    assert_eq!(error.line, 17);
}

#[test]
fn sibling_order_and_fixed_sizing_keep_stable_ids_and_literal_text() {
    let (_dir, solution) = solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Sizing_MU\n  template: blank\n  slots:\n    center:\n      - widget:\n          id: panel\n          type: flexcontainer\n          size: {fixed: 100}\n          children:\n            - label: {id: first, text: ptcslabel-4}\n            - button: {id: second, label: Apply, size: {fixed: 40}}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let document = mashup::edit::parse(&built.content).unwrap();
    let panel = document.find_exact(&built.ids["panel"]).unwrap();
    assert_eq!(
        document.at(&panel.path)["Properties"]["flex-min-height"],
        "100px"
    );
    let first = document.find_exact(&built.ids["first"]).unwrap();
    assert_eq!(
        document.at(&first.path)["Properties"]["LabelText"],
        "ptcslabel-4"
    );
    let second = document.find_exact(&built.ids["second"]).unwrap();
    assert_eq!(document.at(&second.path)["Properties"]["Height"], 40);
    let edited = spec.replace(
        "            - label: {id: first",
        "            - label: {id: unrelated, text: Acme}\n            - label: {id: first",
    );
    let changed = compile(&solution, "Acme", &edited).unwrap();
    for (node, id) in &built.ids {
        assert_eq!(Some(id), changed.ids.get(node));
    }
}

#[test]
fn duplicate_nested_keys_and_invalid_nodes_are_located() {
    for text in [
        String::new(),
        SPEC.replace("params: {limit: 10}", "params: {limit: 10, limit: 20}"),
        SPEC.replace(
            "id: refresh, text: Refresh",
            "id: refresh, text: Refresh, mystery: true",
        ),
        SPEC.replace("id: refresh, text: Refresh", "text: Refresh"),
        SPEC.replace("- button:", "- card:"),
        SPEC.replace("template: list-page", "template: [broken"),
        SPEC.replace("width: 180", "width: 180, width: 200"),
        SPEC.replace("id: alerts_grid", "id: alerts"),
    ] {
        let error = schema::parse(&text).unwrap_err();
        assert!(error.line > 0 && error.column > 0, "{error}");
    }
}

#[test]
fn raw_function_widgets_do_not_become_layout_anchors_and_tabs_have_distinct_roles() {
    let (_dir, solution) = solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Raw_MU\n  template: blank\n  slots:\n    center:\n      - widget: {id: expression, type: expression2}\n      - widget: {id: tabs, type: ptcstabset, props: {NumberOfTabs: 2}}\n";
    let built = compile(&solution, "Acme", text).unwrap();
    assert_eq!(built, compile(&solution, "Acme", text).unwrap());
    let document = mashup::edit::parse(&built.content).unwrap();
    let tabs = document.find_exact(&built.ids["tabs"]).unwrap();
    let children = document.at(&tabs.path)["Widgets"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    assert_ne!(
        children[0]["Properties"]["Id"],
        children[1]["Properties"]["Id"]
    );
    assert!(document.problems().is_empty(), "{:?}", document.problems());
}

#[test]
fn dotted_service_names_use_the_shared_authoring_resolver() {
    let (dir, solution) = solution();
    let path = dir.path().join("acme/Things/Acme.Data.xml");
    let xml = std::fs::read_to_string(&path).unwrap();
    std::fs::write(path, xml.replace("GetAlerts", "Get.Alerts")).unwrap();
    let text = SPEC.replace("GetAlerts", "Get.Alerts");
    let built = compile(&solution, "Acme", &text).unwrap();
    let value: Value = serde_json::from_str(&built.content).unwrap();
    assert_eq!(
        value["Data"]["Things_Acme.Data"]["Services"][0]["Target"],
        "Get.Alerts"
    );
}
