use super::{compile::compile, schema};
use crate::core::{
    config::Solution,
    lock,
    mashup::{self, authoring},
};
use serde_json::Value;
use std::collections::BTreeMap;

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
        rules: None,
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
    let mut expected = normalized(&expected.content);
    // The unversioned primitive retains its legacy gutter; specs use the v1 page gutter.
    expected["UI"]["Widgets"][0]["Properties"]["Margin"] = serde_json::json!("24");
    expected["UI"]["Widgets"][0]["Properties"]["ShowVerticalScrollbar"] = serde_json::json!(false);
    expected["UI"]["Widgets"][0]["Properties"]["scrolling"] = serde_json::json!("no-scroll");
    assert_eq!(normalized(&built.content), expected);
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

fn create_page(solution: &Solution) {
    let spec = solution.root.join("Acme.page.yaml");
    std::fs::write(&spec, SPEC).unwrap();
    let mut plan =
        super::build::plan(solution, "Acme.Alerts_MU", Some("Acme"), Some(&spec)).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(solution, &guard).unwrap();
}

fn page_files(solution: &Solution) -> Vec<std::path::PathBuf> {
    vec![
        solution.root.join("acme/Mashups/Acme.Alerts_MU.xml"),
        solution
            .src_root()
            .join("Acme.Alerts_MU/mashup/content.json"),
        solution.src_root().join("Acme.Alerts_MU/mashup/custom.css"),
        solution
            .src_root()
            .join("Acme.Alerts_MU/.page-baseline.json"),
        solution.src_root().join("Acme.Alerts_MU/page.yaml"),
    ]
}

fn hand_edit(solution: &Solution) {
    let files = page_files(solution);
    let content = std::fs::read_to_string(&files[1])
        .unwrap()
        .replace("Refresh", "Hand edited");
    let assets = mashup::Assets {
        content: content.clone(),
        css: std::fs::read_to_string(&files[2]).unwrap(),
    };
    let entity = mashup::sync(&std::fs::read(&files[0]).unwrap(), &assets)
        .unwrap()
        .0;
    std::fs::write(&files[0], entity).unwrap();
    std::fs::write(&files[1], content).unwrap();
}

#[test]
fn build_new_spec_plans_then_creates_and_rebuild_is_byte_identical() {
    let (_dir, solution) = solution();
    let spec = solution.root.join("Acme.page.yaml");
    std::fs::write(&spec, SPEC).unwrap();
    let plan = super::build::plan(&solution, "Acme.Alerts_MU", None, Some(&spec)).unwrap();
    assert!(!solution.src_root().exists());
    assert_eq!(plan.report["nodes"][0]["status"], "created");
    create_page(&solution);
    let paths = page_files(&solution);
    let before: Vec<_> = paths
        .iter()
        .map(std::fs::read)
        .collect::<Result<_, _>>()
        .unwrap();
    let mut plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    assert_eq!(plan.report["files"], serde_json::json!([]));
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(&solution, &guard).unwrap();
    let after: Vec<_> = paths
        .iter()
        .map(std::fs::read)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(before, after);
    let baseline: Value = serde_json::from_slice(&after[3]).unwrap();
    assert_eq!(baseline["format"], 1);
    assert_eq!(
        baseline["ids"]["refresh"],
        compile(&solution, "Acme", SPEC).unwrap().ids["refresh"]
    );
}

#[test]
fn hand_edits_survive_independent_builds_and_never_enter_baseline() {
    let (_dir, solution) = solution();
    create_page(&solution);
    hand_edit(&solution);
    let files = page_files(&solution);
    std::fs::write(
        &files[4],
        SPEC.replace("title: Alerts", "title: Alerts today"),
    )
    .unwrap();
    let mut plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    assert!(!plan.report["hand_edits_kept"]
        .as_array()
        .unwrap()
        .is_empty());
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(&solution, &guard).unwrap();
    assert!(std::fs::read_to_string(&files[1])
        .unwrap()
        .contains("Hand edited"));
    assert!(!std::fs::read_to_string(&files[3])
        .unwrap()
        .contains("Hand edited"));
    drop(guard);
    let again = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(!again.conflicts());
    assert_eq!(again.report["files"], serde_json::json!([]));
}

#[test]
fn conflicts_are_located_and_write_nothing() {
    let (_dir, solution) = solution();
    create_page(&solution);
    hand_edit(&solution);
    let paths = page_files(&solution);
    std::fs::write(&paths[4], SPEC.replace("text: Refresh", "text: Generated")).unwrap();
    let before: Vec<_> = paths
        .iter()
        .map(std::fs::read)
        .collect::<Result<_, _>>()
        .unwrap();
    let mut plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(plan.conflicts(), "{}", plan.report);
    assert!(plan.report["conflicts"][0]["path"]
        .as_str()
        .unwrap()
        .contains("ptcsbutton-"));
    assert_eq!(plan.report["conflicts"][0]["line"], 11);
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    assert!(plan.apply(&solution, &guard).is_err());
    let after: Vec<_> = paths
        .iter()
        .map(std::fs::read)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn first_adoption_is_safe_only_when_generated_values_already_match() {
    let (_dir, solution) = solution();
    create_page(&solution);
    let paths = page_files(&solution);
    std::fs::remove_file(&paths[3]).unwrap();
    let mut plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(&solution, &guard).unwrap();
    drop(guard);
    std::fs::remove_file(&paths[3]).unwrap();
    hand_edit(&solution);
    let plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(plan.conflicts());
    assert!(!paths[3].exists());
}

#[test]
fn stale_build_checks_every_input_even_on_a_noop() {
    let (_dir, solution) = solution();
    create_page(&solution);
    let paths = page_files(&solution);
    for path in paths
        .iter()
        .chain(std::iter::once(&solution.root.join("twaco.toml")))
    {
        let mut plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
        let old = std::fs::read(path).unwrap();
        let mut changed = old.clone();
        changed.push(b' ');
        std::fs::write(path, &changed).unwrap();
        let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
        let error = plan.apply(&solution, &guard).unwrap_err();
        assert!(
            error.to_string().contains("changed since planning"),
            "{error}"
        );
        assert_eq!(std::fs::read(path).unwrap(), changed);
        std::fs::write(path, old).unwrap();
    }
}

#[test]
fn build_transaction_rolls_back_each_failed_step() {
    let (_dir, solution) = solution();
    create_page(&solution);
    let paths = page_files(&solution);
    std::fs::write(&paths[4], SPEC.replace("text: Refresh", "text: Generated")).unwrap();
    let before: Vec<_> = paths
        .iter()
        .map(std::fs::read)
        .collect::<Result<_, _>>()
        .unwrap();
    for fail in 0..3 {
        let plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
        let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
        let tx = plan.transaction(&solution).unwrap();
        let error = tx
            .apply_with(&guard, &mut |step, _| {
                if step == fail {
                    Err(std::io::Error::other("injected failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(error.changed_nothing(), "{error}");
        let after: Vec<_> = paths
            .iter()
            .map(std::fs::read)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(before, after);
    }
}

#[test]
fn css_blank_lines_and_unmanaged_values_survive_a_changed_build() {
    let (_dir, solution) = solution();
    create_page(&solution);
    let files = page_files(&solution);
    let content = std::fs::read_to_string(&files[1]).unwrap();
    let mut value: Value = serde_json::from_str(&content).unwrap();
    value["AcmeExtra"] = serde_json::json!({"number":1.0,"escaped":"ab"});
    let content = mashup::to_sidecar(&value)
        .replace("\"number\": 1.0", "\"number\": 1.00")
        .replace("\"escaped\": \"ab\"", "\"escaped\": \"a\\u0062\"");
    let css = "/* Acme handmade */\n.Acme { color: red; }\n\n\n";
    let assets = mashup::Assets {
        content: content.clone(),
        css: css.into(),
    };
    let entity = mashup::sync(&std::fs::read(&files[0]).unwrap(), &assets)
        .unwrap()
        .0;
    // Sidecars must hold the same CSS value as XML before build.
    let content = content.replace(
        "\"Data\":",
        &format!(
            "\"CustomMashupCss\": {},\n  \"Data\":",
            serde_json::to_string(&mashup::without_terminator(css)).unwrap()
        ),
    );
    std::fs::write(&files[0], entity).unwrap();
    std::fs::write(&files[1], content).unwrap();
    std::fs::write(&files[2], css).unwrap();
    std::fs::write(&files[4], SPEC.replace("text: Refresh", "text: Generated")).unwrap();
    let mut plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(&solution, &guard).unwrap();
    drop(guard);
    let content = std::fs::read_to_string(&files[1]).unwrap();
    assert!(content.contains("\"number\": 1.00"));
    assert!(content.contains("\"escaped\": \"a\\u0062\""));
    assert_eq!(std::fs::read_to_string(&files[2]).unwrap(), css);
    assert!(mashup::sync(
        &std::fs::read(&files[0]).unwrap(),
        &mashup::Assets {
            content,
            css: css.into()
        }
    )
    .unwrap()
    .1
    .is_empty());
    let plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert_eq!(plan.report["files"], serde_json::json!([]));
}

#[test]
fn safe_adoption_allows_unmanaged_properties() {
    let (_dir, solution) = solution();
    create_page(&solution);
    let files = page_files(&solution);
    let content = std::fs::read_to_string(&files[1])
        .unwrap()
        .replace("\"UI\":", "\"AcmeExtra\": {\"value\": 1.00},\n  \"UI\":");
    let assets = mashup::Assets {
        content: content.clone(),
        css: std::fs::read_to_string(&files[2]).unwrap(),
    };
    let entity = mashup::sync(&std::fs::read(&files[0]).unwrap(), &assets)
        .unwrap()
        .0;
    std::fs::write(&files[0], &entity).unwrap();
    std::fs::write(&files[1], &content).unwrap();
    std::fs::remove_file(&files[3]).unwrap();
    let mut plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(&solution, &guard).unwrap();
    assert_eq!(std::fs::read(&files[0]).unwrap(), entity);
    assert_eq!(std::fs::read_to_string(&files[1]).unwrap(), content);
}

#[test]
fn corrupt_or_unsupported_baselines_are_refused_without_writes() {
    let (_dir, solution) = solution();
    create_page(&solution);
    let paths = page_files(&solution);
    let baseline: Value = serde_json::from_slice(&std::fs::read(&paths[3]).unwrap()).unwrap();
    for (field, value) in [
        ("format", serde_json::json!(2)),
        ("compiler", serde_json::json!("future")),
        ("rules", serde_json::json!("future")),
        ("content", Value::Null),
    ] {
        let mut bad = baseline.clone();
        bad[field] = value;
        std::fs::write(&paths[3], bad.to_string()).unwrap();
        let before: Vec<_> = paths
            .iter()
            .map(std::fs::read)
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(super::build::plan(&solution, "Acme.Alerts_MU", None, None).is_err());
        let after: Vec<_> = paths
            .iter()
            .map(std::fs::read)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(before, after);
    }
}

#[test]
fn new_spec_failure_rolls_back_all_created_artifacts_and_the_copy() {
    let (_dir, solution) = solution();
    let spec = solution.root.join("Acme.page.yaml");
    std::fs::write(&spec, SPEC).unwrap();
    for fail in 0..5 {
        let plan = super::build::plan(&solution, "Acme.Alerts_MU", None, Some(&spec)).unwrap();
        let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
        let tx = plan.transaction(&solution).unwrap();
        let error = tx
            .apply_with(&guard, &mut |step, _| {
                if step == fail {
                    Err(std::io::Error::other("injected creation failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(error.changed_nothing(), "{error}");
        assert!(page_files(&solution).iter().all(|p| !p.exists()));
        assert_eq!(std::fs::read_to_string(&spec).unwrap(), SPEC);
    }
}

#[test]
fn removed_nodes_are_reported_and_removed_atomically() {
    let (_dir, solution) = solution();
    create_page(&solution);
    let paths = page_files(&solution);
    let old: Value = serde_json::from_slice(&std::fs::read(&paths[3]).unwrap()).unwrap();
    let id = old["ids"]["refresh"].as_str().unwrap();
    let spec = SPEC
        .replace(
            "    toolbar:\n      - button: {id: refresh, text: Refresh}\n",
            "",
        )
        .replace(
            "  events:\n    - {on: refresh.Clicked, invoke: alerts}\n",
            "  events: []\n",
        );
    std::fs::write(&paths[4], spec).unwrap();
    let mut plan = super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let removed = plan.report["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["node"] == "refresh")
        .unwrap();
    assert_eq!(removed["status"], "changed");
    assert_eq!(removed["removed"], true);
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(&solution, &guard).unwrap();
    assert!(!std::fs::read_to_string(&paths[1]).unwrap().contains(id));
    assert!(!std::fs::read_to_string(&paths[3]).unwrap().contains(id));
}

fn element_solution() -> (tempfile::TempDir, Solution) {
    let (dir, _) = solution();
    let folder = dir.path().join("acme");
    std::fs::write(folder.join("Things/Acme.Data.xml"), r#"<Entities><Things><Thing name="Acme.Data" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetRows"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Rows_DS"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#).unwrap();
    std::fs::create_dir_all(folder.join("DataShapes")).unwrap();
    std::fs::write(folder.join("DataShapes/Acme.Rows_DS.xml"),r#"<Entities><DataShapes><DataShape name="Acme.Rows_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="Name" baseType="STRING" ordinal="0"/><FieldDefinition name="Count" baseType="NUMBER" ordinal="1"/></FieldDefinitions></DataShape></DataShapes></Entities>"#).unwrap();
    let child = authoring::session::Session::new(
        "Acme.Child_MU",
        "Acme",
        "tab-content",
        "PTC Convergence Theme",
    )
    .unwrap();
    std::fs::create_dir_all(folder.join("Mashups")).unwrap();
    std::fs::write(folder.join("Mashups/Acme.Child_MU.xml"), child.entity).unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution)
}

#[test]
fn every_element_and_mixed_nesting_match_golden_artifacts() {
    let (_dir, solution) = element_solution();
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    let mut paths: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".page.yaml"))
        .collect();
    paths.sort();
    assert_eq!(paths.len(), 22);
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
        let actual = serde_json::to_string_pretty(&serde_json::json!({
            "entity":String::from_utf8(built.entity).unwrap(),
            "content":serde_json::from_str::<Value>(&built.content).unwrap(),"css":built.css,"ids":built.ids
        })).unwrap()+"\n";
        let mut problems = vec![];
        mashup::validate::check_content(&built.content, &mut problems);
        assert!(
            !problems
                .iter()
                .any(|p| p.severity == mashup::validate::Severity::Error),
            "{}: {problems:?}",
            path.display()
        );
        let golden = path.with_extension("golden.json");
        if std::env::var_os("TWACO_BLESS").is_some() {
            std::fs::write(&golden, &actual).unwrap();
        }
        assert_eq!(
            actual,
            std::fs::read_to_string(golden)
                .unwrap()
                .replace("\r\n", "\n"),
            "{}",
            path.display()
        );
    }
}

#[test]
fn element_schema_errors_are_located() {
    let (_dir, solution) = element_solution();
    for body in [
        "page_header: {id: header}",
        "toolbar: {id: tools, band: invalid}",
        "kpi_row: {id: tiles, tiles: []}",
        "card: {id: card}",
        "filter_panel: {id: filters, footer: [delete]}",
        "tabs: {id: views, tabs: [{label: Details}]}",
        "chart: {id: chart, kind: area, data: rows, x: Name}",
        "spacer: {id: space, mystery: true}",
        "date_range: {id: date, mystery: true}",
        "dropdown: {id: choice, options: missing, display: Name}",
        "text_field: {id: query, mystery: true}",
        "pagination: {id: pager, for: absent}",
    ] {
        let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Errors_MU\n  template: blank\n  slots:\n    center:\n      - {body}\n");
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.line >= 8 && error.column > 0, "{body}: {error}");
    }
}

fn canonical(content: &str) -> Value {
    let document = mashup::edit::parse(content).unwrap();
    let ids: std::collections::BTreeMap<_, _> = document
        .widgets()
        .iter()
        .enumerate()
        .map(|(i, w)| (w.id.clone(), format!("widget-{i}")))
        .collect();
    fn replace(value: &mut Value, ids: &std::collections::BTreeMap<String, String>) {
        match value {
            Value::String(s) => {
                if let Some(id) = ids.get(s) {
                    *s = id.clone();
                } else if uuid::Uuid::parse_str(s).is_ok() {
                    *s = "uuid".into();
                }
            }
            Value::Object(map) => {
                map.remove("twacoLayoutAuthored");
                let old = std::mem::take(map);
                for (key, mut value) in old {
                    replace(&mut value, ids);
                    map.insert(ids.get(&key).cloned().unwrap_or(key), value);
                }
            }
            Value::Array(items) => {
                for v in items {
                    replace(v, ids);
                }
            }
            _ => {}
        }
    }
    let mut value: Value = serde_json::from_str(content).unwrap();
    replace(&mut value, &ids);
    normalized(&value.to_string())
}

#[test]
fn elements_match_equivalent_primitive_recipe_sequences() {
    use authoring::{session::Session, AddRequest, Recipe, SourceAddRequest};
    let (_dir, solution) = element_solution();
    for (body, kind, props) in [
        (
            "card: {id: target, title: Status, value: 42, status: Healthy, kind: ok}",
            "flexcontainer",
            vec![("flex-direction".into(), "column".into())],
        ),
        (
            "chart: {id: target, kind: line, data: rows, x: Name}",
            "ptcschartline",
            vec![],
        ),
        (
            "dropdown: {id: target, options: rows, display: Name}",
            "ptcsdropdown",
            vec![],
        ),
        (
            "grid: {id: target, data: rows}\n      - pagination: {id: pager, for: target}",
            "ptcsgrid",
            vec![],
        ),
    ] {
        let text=format!("version: 1\nrules: v1\npage:\n  name: Acme.Elements_MU\n  template: blank\n  data:\n    rows: {{service: Acme.Data.GetRows, on: load}}\n  slots:\n    center:\n      - {body}\n");
        let built = compile(&solution, "Acme", &text).unwrap();
        let mut primitive =
            Session::new("Acme.Elements_MU", "Acme", "blank", "PTC Convergence Theme").unwrap();
        let target = primitive
            .add(
                &solution,
                &AddRequest {
                    mashup: "Acme.Elements_MU".into(),
                    widget: kind.into(),
                    inputs: vec![],
                    placement: Some(mashup::edit::Placement::In("center".into())),
                    label: None,
                    properties: props,
                },
            )
            .unwrap();
        primitive
            .source(
                &solution,
                &SourceAddRequest {
                    mashup: "Acme.Elements_MU".into(),
                    entity: "Acme.Data".into(),
                    service: "GetRows".into(),
                    dynamic: false,
                    on_load: true,
                    alias: Some("rows".into()),
                    entity_from: None,
                    refresh: None,
                    cache: false,
                    cache_keys: vec![],
                    cache_strategy: None,
                    max_result_sets: None,
                    param_values: vec![],
                },
            )
            .unwrap();
        let data = "data:Acme.Data.GetRows[as rows]".to_string();
        let target_id = target.clone();
        let recipe = match kind {
            "flexcontainer" => Recipe::Card {
                container: target,
                caption: "Status".into(),
                value: Some("42".into()),
                status: Some("Healthy".into()),
                kind: "ok".into(),
            },
            "ptcschartline" => Recipe::Chart {
                series: vec![],
                widget: target,
                data,
                x: "Name".into(),
                value: None,
                kind: None,
                title: None,
                x_title: None,
                y_title: None,
                stacked: false,
            },
            "ptcsdropdown" => Recipe::Picker {
                widget: target,
                data,
                display: "Name".into(),
                value: None,
                selected: None,
                no_none_row: false,
                auto_select_first: false,
                multiple: false,
            },
            _ => Recipe::Pagination {
                widget: target,
                data,
            },
        };
        primitive.recipe(&solution, recipe).unwrap();
        if kind == "flexcontainer" {
            // Page cards title with the regular label size (v1 rule), not the recipe's sub-header.
            let card = primitive
                .document
                .find_exact("flexcontainer-3")
                .unwrap()
                .path;
            let caption = primitive
                .document
                .widgets()
                .into_iter()
                .find(|w| w.path.starts_with(&card) && w.type_id == "ptcslabel")
                .unwrap()
                .path;
            primitive
                .document
                .set_property(&caption, "LabelType", serde_json::json!("label"));
        }
        let parsed = schema::parse(&text).unwrap();
        let mut ids = std::collections::BTreeMap::from([("target".to_string(), target_id)]);
        if let Some(widget) = primitive
            .document
            .widgets()
            .into_iter()
            .find(|w| w.type_id == "ptcspagination")
        {
            ids.insert("pager".to_string(), widget.id);
        }
        super::elements::finish_layout(&mut primitive, &parsed.page.value, &ids).unwrap();
        assert_eq!(
            canonical(&built.content),
            canonical(&primitive.document.to_content()),
            "{kind}"
        );
    }
}

#[test]
fn old_id_policy_baselines_are_recognised_and_refused() {
    let (_dir, solution) = solution();
    let built = compile(&solution, "Acme", SPEC).unwrap();
    let baseline = super::baseline::Baseline::generated(&built, "Acme.Alerts_MU", "Acme");
    let mut old: Value = serde_json::from_slice(&baseline.bytes()).unwrap();
    old.as_object_mut().unwrap().remove("id_policy");
    let error = super::baseline::Baseline::read(&serde_json::to_vec(&old).unwrap()).unwrap_err();
    assert!(
        error.contains("id policy") && error.contains("version"),
        "{error}"
    );
}

#[test]
fn pagination_forward_grid_and_widget_references_are_supported() {
    let (_dir, solution) = element_solution();
    let text="version: 1\nrules: v1\npage:\n  name: Acme.Pages_MU\n  template: blank\n  data:\n    rows: {service: Acme.Data.GetRows, on: load}\n  slots:\n    center:\n      - pagination: {id: pages, for: grid}\n      - label: {id: number, props: {LabelText: {bind: pages.PageNumber}}}\n      - grid: {id: grid, data: rows, columns: [Name, Count]}\n";
    let built = compile(&solution, "Acme", text).unwrap();
    assert!(built.ids.contains_key("pages"));
    let content: Value = serde_json::from_str(&built.content).unwrap();
    assert!(content["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["SourceId"] == built.ids["pages"] && b["TargetId"] == built.ids["number"]));
}

#[test]
fn bands_spacers_equal_tiles_and_filter_footer_keep_their_contracts() {
    let (_dir, solution) = element_solution();
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    for fixture in ["toolbar", "mixed"] {
        let text = std::fs::read_to_string(folder.join(format!("{fixture}.page.yaml"))).unwrap();
        let built = compile(&solution, "Acme", &text).unwrap();
        let doc = mashup::edit::parse(&built.content).unwrap();
        let props = |name: &str| {
            let w = doc.find_exact(&built.ids[name]).unwrap();
            doc.at(&w.path)["Properties"].clone()
        };
        assert_eq!(props("tools")["flex-min-height"], "48px");
        assert_eq!(props("tools")["flex-max-height"], "48px");
        assert_eq!(props("space")["flex-grow"], 1);
        if fixture == "mixed" {
            for tile in ["total", "open"] {
                assert_eq!(props(tile)["flex-grow"], 1);
                assert_eq!(props(tile)["flex-basis"], "0px");
            }
            assert_eq!(props("filters--apply")["ButtonType"], "primary");
            assert_eq!(props("filters--reset")["ButtonType"], "tertiary");
        }
    }
}

#[test]
fn plain_toolbar_and_explicit_size_override_the_band_default() {
    let (_dir, solution) = element_solution();
    let spec = |fields| {
        format!("version: 1\nrules: v1\npage:\n  name: Acme.Toolbar_MU\n  template: blank\n  slots:\n    center:\n      - toolbar: {{id: tools, {fields}}}\n")
    };
    let plain = compile(&solution, "Acme", &spec("band: false")).unwrap();
    let doc = mashup::edit::parse(&plain.content).unwrap();
    let tool = doc.find_exact(&plain.ids["tools"]).unwrap();
    assert_ne!(doc.at(&tool.path)["Properties"]["flex-min-height"], "48px");
    let transparent = compile(&solution, "Acme", &spec("surface: false")).unwrap();
    let content: Value = serde_json::from_str(&transparent.content).unwrap();
    assert_eq!(
        content["UI"]["Properties"]["StyleProperties"]["data"][&transparent.ids["tools"]]["parts"]
            ["$"]["$"]["background"],
        "transparent"
    );
    let sized = compile(&solution, "Acme", &spec("size: {fixed: 72}")).unwrap();
    let doc = mashup::edit::parse(&sized.content).unwrap();
    let tool = doc.find_exact(&sized.ids["tools"]).unwrap();
    assert_eq!(doc.at(&tool.path)["Properties"]["flex-min-height"], "72px");
}

#[test]
fn recipe_ids_and_binding_uuids_survive_unrelated_nodes() {
    let (_dir, solution) = element_solution();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mashup/pages/pagination.page.yaml");
    let text = std::fs::read_to_string(path).unwrap();
    let first = compile(&solution, "Acme", &text).unwrap();
    let second = compile(
        &solution,
        "Acme",
        &text.replace(
            "    center:\n",
            "    center:\n      - label: {id: unrelated, text: Acme}\n",
        ),
    )
    .unwrap();
    for (node, id) in first.ids {
        assert_eq!(second.ids[&node], id);
    }
    let a: Value = serde_json::from_str(&first.content).unwrap();
    let b: Value = serde_json::from_str(&second.content).unwrap();
    assert_eq!(a["DataBindings"], b["DataBindings"]);
    assert_eq!(a["Events"], b["Events"]);
}

#[test]
fn column_shorthand_is_identical_to_explicit_fields() {
    let (_dir, solution) = element_solution();
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mashup/pages/grid.page.yaml"),
    )
    .unwrap();
    assert_eq!(
        compile(&solution, "Acme", &text).unwrap(),
        compile(
            &solution,
            "Acme",
            &text.replace("columns: [Name,", "columns: [{field: Name},")
        )
        .unwrap()
    );
}

fn benchmark(id: &str) {
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    let solution = Solution::load(&folder.join("bench_solution/twaco.toml")).unwrap();
    let spec = std::fs::read_to_string(folder.join(format!("bench_{id}/page.yaml"))).unwrap();
    let built = compile(&solution, "Acme.Bench", &spec).unwrap();
    assert_eq!(built, compile(&solution, "Acme.Bench", &spec).unwrap());
    let mut problems = vec![];
    mashup::validate::check_content(&built.content, &mut problems);
    assert!(
        !problems
            .iter()
            .any(|p| p.severity == mashup::validate::Severity::Error),
        "{problems:?}"
    );
    let mut golden_value = serde_json::json!({
        "entity": String::from_utf8(built.entity).unwrap(),
        "content": serde_json::from_str::<Value>(&built.content).unwrap(),
        "css": built.css, "ids": built.ids
    });
    if !built.cells.is_empty() {
        golden_value["cells"] = built
            .cells
            .iter()
            .map(|cell| {
                serde_json::json!({
                    "name": cell.name, "spec": cell.spec,
                    "entity": String::from_utf8(cell.artifacts.entity.clone()).unwrap(),
                    "content": serde_json::from_str::<Value>(&cell.artifacts.content).unwrap(),
                    "css": cell.artifacts.css, "ids": cell.artifacts.ids
                })
            })
            .collect();
    }
    let actual = serde_json::to_string_pretty(&golden_value).unwrap()
        + "
";
    let golden = folder.join(format!("bench_{id}/page.golden.json"));
    if std::env::var_os("TWACO_BLESS").is_some() {
        std::fs::write(&golden, &actual).unwrap();
    }
    assert_eq!(
        actual,
        std::fs::read_to_string(golden)
            .unwrap()
            .replace("\r\n", "\n")
    );
}

#[test]
fn benchmark_a_golden() {
    benchmark("a");
}
#[test]
fn benchmark_h_golden() {
    benchmark("h");
}
#[test]
fn benchmark_j_golden() {
    benchmark("j");
}
#[test]
fn benchmark_k_golden() {
    benchmark("k");
}

#[test]
fn filter_fields_use_the_kit_column_and_footer_structure() {
    let built = compiled_benchmark("k");
    let doc = doc_of(&built);
    let fields = doc.find("filters").unwrap();
    let p = &doc.at(&fields.path)["Properties"];
    assert_eq!(p["flex-size"], "default");
    assert_eq!(p["flex-grow"], 1);
    assert_eq!(p["flex-min-height"], "");
    let stack = &fields.path[..fields.path.len() - 1];
    assert_eq!(doc.at(stack)["Properties"]["DisplayName"], "filter-stack");
    assert_eq!(doc.at(stack)["Properties"]["LastContainer"], true);
    assert_eq!(doc.at(stack)["Properties"]["flex-size"], "default");
    let controls = doc.at(&fields.path)["Widgets"].as_array().unwrap();
    assert!(!controls.is_empty());
    assert!(controls
        .iter()
        .all(|c| c["Properties"]["Type"] != "flexcontainer"));
    assert!(!built.content.contains("--remaining-space"));
    let footer = doc.find("filter-footer").unwrap();
    let area = &footer.path[..footer.path.len() - 1];
    assert_eq!(doc.at(area)["Properties"]["flex-direction"], "column");
    assert_eq!(
        doc.at(area)["Widgets"][0]["Properties"]["flex-min-height"],
        "1px"
    );
    assert_eq!(doc.at(&footer.path)["Properties"]["LastContainer"], true);
}
#[test]
fn benchmark_n_golden() {
    benchmark("n");
}
#[test]
fn benchmark_o_golden() {
    benchmark("o");
}
#[test]
fn benchmark_q1_golden() {
    benchmark("q1");
}

#[test]
fn page_header_with_help_and_actions_keeps_its_height_in_a_column() {
    let (_dir, solution) = element_solution();
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    let text = std::fs::read_to_string(folder.join("page_header.page.yaml")).unwrap();
    let built = compile(&solution, "Acme", &text).unwrap();
    let doc = mashup::edit::parse(&built.content).unwrap();
    let header = doc.find_exact(&built.ids["header"]).unwrap();
    let props = &doc.at(&header.path)["Properties"];
    // The spacer's grow rule gave the header a 0px basis, which collapsed a row in a column.
    assert_eq!(props["flex-basis"], "auto");
    assert_eq!(props["flex-grow"], 0);
}

#[test]
fn nested_tabs_with_a_status_grid_compile_on_a_small_stack_and_runaway_nesting_is_a_located_error()
{
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    let solution = Solution::load(&folder.join("bench_solution/twaco.toml")).unwrap();
    let spec = std::fs::read_to_string(folder.join("bench_q1/page.yaml")).unwrap();
    // The main thread has 1 MiB; the page worker must not depend on the caller's stack.
    let small = std::thread::Builder::new()
        .stack_size(256 << 10)
        .spawn(move || compile(&solution, "Acme.Bench", &spec).is_ok())
        .unwrap()
        .join()
        .unwrap();
    assert!(small);
    let mut text = String::from(
        "version: 1
rules: v1
page:
  name: Acme.Deep_MU
  template: blank
  slots:
    center:
",
    );
    let mut pad = String::from("      ");
    for level in 0..40 {
        text.push_str(&format!(
            "{pad}- widget:
{pad}    id: w{level}
{pad}    type: flexcontainer
{pad}    children:
"
        ));
        pad.push_str("      ");
    }
    let error = schema::parse(&text).unwrap_err();
    assert!(error.message.contains("budget"), "{error}");
    assert!(error.line > 1, "{error}");
}

#[test]
fn alerts_layout_has_one_gutter_centered_fixed_band_and_only_spacer_grows() {
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    let solution = Solution::load(&folder.join("bench_solution/twaco.toml")).unwrap();
    let spec = std::fs::read_to_string(folder.join("bench_j/page.yaml")).unwrap();
    let built = compile(&solution, "Acme.Bench", &spec).unwrap();
    let doc = mashup::edit::parse(&built.content).unwrap();
    let root = doc.find("center").unwrap();
    assert_eq!(doc.at(&root.path)["Properties"]["Margin"], "24");
    let toolbar = doc.find_exact(&built.ids["tools"]).unwrap();
    let props = &doc.at(&toolbar.path)["Properties"];
    assert_eq!(props["flex-min-height"], "48px");
    assert_eq!(props["flex-max-height"], "48px");
    assert_eq!(props["flex-grow"], 0);
    assert_eq!(props["flex-shrink"], 0);
    assert_eq!(props["align-items"], "center");
    let spacer = doc.find_exact(&built.ids["space"]).unwrap();
    for widget in doc.widgets().into_iter().filter(|w| {
        w.path.starts_with(&toolbar.path) && w.path != toolbar.path && w.type_id == "flexcontainer"
    }) {
        let props = &doc.at(&widget.path)["Properties"];
        let grows = spacer.path.starts_with(&widget.path);
        assert_eq!(
            props["flex-grow"],
            if grows { 1 } else { 0 },
            "{}",
            widget.id
        );
        if widget.path != spacer.path {
            assert_eq!(props["align-items"], "center", "{}", widget.id);
        }
    }
    let date = doc.find_exact(&built.ids["dates"]).unwrap();
    assert_eq!(doc.at(&date.path)["Properties"]["Width"], 340);
    assert_eq!(doc.at(&date.path)["Properties"]["FormatToken"], "DD-MM-YY");
    for widget in doc
        .widgets()
        .into_iter()
        .filter(|w| w.display_name == "body")
    {
        assert_eq!(doc.at(&widget.path)["Properties"]["Margin"], "");
    }
    assert!(!doc.widgets().iter().any(|w| w.display_name == "toolbar"));
}

fn compiled_benchmark(id: &str) -> super::compile::Artifacts {
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    let solution = Solution::load(&folder.join("bench_solution/twaco.toml")).unwrap();
    let spec = std::fs::read_to_string(folder.join(format!("bench_{id}/page.yaml"))).unwrap();
    compile(&solution, "Acme.Bench", &spec).unwrap()
}

#[test]
fn dashboard_charts_stay_in_their_panels_and_share_remaining_height() {
    let built = compiled_benchmark("a");
    let doc = mashup::edit::parse(&built.content).unwrap();
    let charts = doc.find_exact(&built.ids["charts"]).unwrap();
    let props = &doc.at(&charts.path)["Properties"];
    assert_eq!(props["flex-size"], "range");
    assert_eq!(props["flex-min-height"], "348px");
    assert_eq!(props["flex-max-height"], "");
    assert_eq!(props["flex-grow"], 1);
    assert_eq!(props["flex-shrink"], 0);
    assert_eq!(props["ShowVerticalScrollbar"], false);
    for (chart, panel, tools) in [
        (
            "distribution",
            "distribution_panel",
            "distribution_panel--toolbar",
        ),
        ("trend", "trend_panel", "trend_panel--toolbar"),
    ] {
        let chart = doc.find_exact(&built.ids[chart]).unwrap();
        let panel = doc.find_exact(&built.ids[panel]).unwrap();
        assert!(chart.path.starts_with(&panel.path));
        assert!(panel.path.starts_with(&charts.path));
        assert_eq!(
            doc.at(&panel.path)["Properties"]["flex-min-height"],
            "348px"
        );
        let tools = doc.find_exact(&built.ids[tools]).unwrap();
        assert_eq!(doc.at(&tools.path)["Properties"]["flex-min-height"], "48px");
        assert_eq!(doc.at(&tools.path)["Properties"]["align-items"], "center");
    }
    // Tiles and detail cards size to content (about 130px and 170px with their captions).
    for name in ["overview", "focus_row"] {
        let node = doc.find_exact(&built.ids[name]).unwrap();
        let props = &doc.at(&node.path)["Properties"];
        assert_eq!(props["flex-grow"], 0, "{name}");
        assert_eq!(props["flex-shrink"], 0, "{name}");
    }
    // One shared chart row fits at 900; its floor survives a shorter viewport.
    let minimum = 348 + 130 + 170 + 48 + 43;
    assert!(minimum < 900);
    assert!(minimum > 717);
}

#[test]
fn filter_footer_buttons_fit_inside_the_column_gutters() {
    let built = compiled_benchmark("k");
    let doc = mashup::edit::parse(&built.content).unwrap();
    let footer = doc.find("filter-footer").unwrap();
    let props = &doc.at(&footer.path)["Properties"];
    assert_eq!(props["flex-min-width"], "280px");
    assert_eq!(props["flex-max-width"], "280px");
    assert_eq!(props["Margin"], "");
    assert_eq!(props["flex-size"], "default");
    let mut widths = vec![];
    for name in ["filters--apply", "filters--reset"] {
        let button = doc.find_exact(&built.ids[name]).unwrap();
        assert_eq!(&button.path[..button.path.len() - 1], footer.path);
        let props = &doc.at(&button.path)["Properties"];
        assert_eq!(
            props["Margin"],
            if widths.is_empty() {
                "24 8 24 24"
            } else {
                "24 24 24 8"
            }
        );
        widths.push(props["Width"].as_u64().unwrap());
    }
    assert_eq!(widths, [108, 108]);
    assert_eq!(widths.iter().sum::<u64>() + 64, 280);
}

#[test]
fn metrics_load_and_apply_keep_all_filters_but_unset_dates_stay_empty() {
    let built = compiled_benchmark("k");
    let doc = mashup::edit::parse(&built.content).unwrap();
    let dates = doc.find_exact(&built.ids["filter_dates"]).unwrap();
    assert_eq!(
        doc.at(&dates.path)["Properties"]["InitializeWithCurrentDateTime"],
        false
    );
    for name in ["version", "status", "category"] {
        let field = doc.find_exact(&built.ids[name]).unwrap();
        let props = &doc.at(&field.path)["Properties"];
        assert_eq!(props["SelectedText"], "");
        assert_eq!(props["AutoSelectFirstRow"], false);
    }
    let content: Value = serde_json::from_str(&built.content).unwrap();
    let calls = content["Data"]["Things_Acme.BenchData"]["Services"]
        .as_array()
        .unwrap();
    assert!(calls
        .iter()
        .any(|c| c["Name"] == "metrics" && c["Target"] == "FilterMetrics"));
    let events = content["Events"].as_array().unwrap();
    assert!(events
        .iter()
        .any(|e| e["EventTriggerEvent"] == "Loaded" && e["EventHandlerService"] == "metrics"));
    assert!(events
        .iter()
        .any(|e| e["EventTriggerId"] == built.ids["filters--apply"]
            && e["EventHandlerService"] == "metrics"));
    for (field, source, param) in [
        ("query", "Text", "query"),
        ("version", "SelectedText", "version"),
        ("status", "SelectedText", "status"),
        ("category", "SelectedText", "category"),
        ("filter_dates", "StartDate", "startDate"),
        ("filter_dates", "EndDate", "endDate"),
    ] {
        assert!(content["DataBindings"].as_array().unwrap().iter().any(|b| {
            b["SourceId"] == built.ids[field]
                && b["TargetId"] == "metrics"
                && b["PropertyMaps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["SourceProperty"] == source && p["TargetProperty"] == param)
        }));
    }
}

#[test]
fn content_cards_and_kpi_rows_do_not_take_chart_height() {
    let (_dir, solution) = element_solution();
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    for (fixture, name) in [("card", "card"), ("kpi_row", "summary")] {
        let text = std::fs::read_to_string(folder.join(format!("{fixture}.page.yaml"))).unwrap();
        let built = compile(&solution, "Acme", &text).unwrap();
        let doc = mashup::edit::parse(&built.content).unwrap();
        let node = doc.find_exact(&built.ids[name]).unwrap();
        let props = &doc.at(&node.path)["Properties"];
        assert_eq!(props["flex-grow"], 0);
        assert_eq!(props["flex-shrink"], 0);
        assert_eq!(props["flex-basis"], "auto");
    }
}

#[test]
fn explicit_date_initialization_overrides_the_bound_filter_default() {
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    let solution = Solution::load(&folder.join("bench_solution/twaco.toml")).unwrap();
    let spec = std::fs::read_to_string(folder.join("bench_k/page.yaml"))
        .unwrap()
        .replace(
            "Label: Date, Width: 228",
            "Label: Date, InitializeWithCurrentDateTime: true, Width: 228",
        );
    let built = compile(&solution, "Acme.Bench", &spec).unwrap();
    let doc = mashup::edit::parse(&built.content).unwrap();
    let dates = doc.find_exact(&built.ids["filter_dates"]).unwrap();
    assert_eq!(
        doc.at(&dates.path)["Properties"]["InitializeWithCurrentDateTime"],
        true
    );
}

fn element_fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/mashup/pages/{name}.page.yaml")),
    )
    .unwrap()
}

fn element_props(built: &super::compile::Artifacts, id: &str) -> Value {
    let doc = mashup::edit::parse(&built.content).unwrap();
    let widget = doc.find_exact(&built.ids[id]).unwrap();
    doc.at(&widget.path)["Properties"].clone()
}

fn error_page(body: &str, tail: &str) -> String {
    format!(
        "version: 1\nrules: v1\npage:\n  name: Acme.Errors_MU\n  template: blank\n  data:\n    rows: {{service: Acme.Data.GetRows, on: load}}\n  slots:\n    center:\n      - {body}\n{tail}"
    )
}

#[test]
fn trend_tiles_carry_value_unit_direction_and_delta_in_one_centred_row() {
    let (_dir, solution) = element_solution();
    let built = compile(&solution, "Acme", &element_fixture("trend_tile")).unwrap();
    let card = element_props(&built, "total");
    assert_eq!(card["align-items"], "stretch");
    assert_eq!(card["flex-direction"], "column");
    let figure = element_props(&built, "total--figure");
    assert_eq!(figure["justify-content"], "center");
    assert_eq!(figure["align-items"], "center");
    assert_eq!(
        element_props(&built, "total--value")["LabelType"],
        "large-header"
    );
    assert_eq!(element_props(&built, "total--unit")["LabelText"], "alerts");
    assert_eq!(
        element_props(&built, "total--arrow")["LabelText"],
        "\u{25B2}"
    );
    assert_eq!(
        element_props(&built, "stable--arrow")["LabelText"],
        "\u{25AC}"
    );
    let content: Value = serde_json::from_str(&built.content).unwrap();
    let styles = &content["UI"]["Properties"]["StyleProperties"]["data"];
    for id in ["total--arrow", "total--delta"] {
        assert_eq!(
            styles[&built.ids[id]]["parts"]["label"]["$"]["color"],
            "#3C9A5F"
        );
    }
    for (field, id) in [("Count", "total--value"), ("Name", "total--delta")] {
        assert!(content["DataBindings"].as_array().unwrap().iter().any(|b| {
            b["TargetId"] == built.ids[id]
                && b["PropertyMaps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["SourceProperty"] == field && p["TargetProperty"] == "LabelText")
        }));
    }
}

#[test]
fn trend_tile_errors_are_located() {
    let (_dir, solution) = element_solution();
    for body in [
        "trend_tile: {id: tile, value: {bind: rows.Count}}",
        "trend_tile: {id: tile, label: Total}",
        "trend_tile: {id: tile, label: Total, value: 3}",
        "trend_tile: {id: tile, label: Total, value: {bind: rows.Count}, trend: sideways}",
        "trend_tile: {id: tile, label: Total, value: {bind: rows.Count}, delta: 3}",
        "card: {id: tile, title: Total, trend: up}",
    ] {
        let error = compile(&solution, "Acme", &error_page(body, "")).unwrap_err();
        assert!(error.line >= 10 && error.column > 0, "{body}: {error}");
    }
}

#[test]
fn detail_pairs_share_a_content_height_row_with_caption_left_and_value_right() {
    let (_dir, solution) = element_solution();
    let built = compile(&solution, "Acme", &element_fixture("detail_pair")).unwrap();
    assert_eq!(element_props(&built, "details")["align-items"], "stretch");
    for id in ["first", "second"] {
        let row = element_props(&built, id);
        assert_eq!(row["flex-direction"], "row");
        assert_eq!(row["flex-grow"], 0);
        assert_eq!(row["flex-shrink"], 0);
        assert_eq!(row["flex-max-height"], "");
        let caption = element_props(&built, &format!("{id}--caption"));
        assert_eq!(caption["HorizontalAlignment"], "left");
        let value = element_props(&built, &format!("{id}--value"));
        assert_eq!(value["HorizontalAlignment"], "right");
        assert_eq!(
            element_props(&built, &format!("{id}--spacer"))["flex-grow"],
            1
        );
    }
    assert!(built.ids.contains_key("first--dot"));
    assert!(built.ids.contains_key("second--dot"));
    let content: Value = serde_json::from_str(&built.content).unwrap();
    assert_eq!(
        content["UI"]["Properties"]["StyleProperties"]["data"][&built.ids["first--dot"]]["parts"]
            ["label"]["$"]["color"],
        "#4CAF50"
    );
}

#[test]
fn buttons_take_a_variant_an_icon_and_a_selection_gate() {
    let (_dir, solution) = element_solution();
    let built = compile(&solution, "Acme", &element_fixture("button_options")).unwrap();
    let preview = element_props(&built, "preview");
    assert_eq!(preview["ButtonType"], "transparent");
    assert_eq!(preview["SVGIcon"], "cds:icon_view");
    assert_eq!(element_props(&built, "save")["ButtonType"], "primary");
    let content: Value = serde_json::from_str(&built.content).unwrap();
    let bindings = content["DataBindings"].as_array().unwrap();
    let expression = bindings
        .iter()
        .find(|b| b["SourceId"] == built.ids["results"])
        .and_then(|b| b["TargetId"].as_str())
        .unwrap()
        .to_string();
    assert!(expression.starts_with("expression2-"));
    assert!(bindings.iter().any(|b| b["SourceId"] == expression
        && b["TargetId"] == built.ids["preview"]
        && b["PropertyMaps"][0]["TargetProperty"] == "Disabled"));
    assert!(bindings
        .iter()
        .any(|b| b["SourceId"] == built.ids["results"]
            && b["PropertyMaps"][0]["SourceProperty"] == "RowSelected"));
    assert_eq!(
        compile(&solution, "Acme", &element_fixture("button_options")).unwrap(),
        built
    );
}

#[test]
fn button_option_errors_are_located() {
    let (_dir, solution) = element_solution();
    let grid = "      - grid: {id: results, data: rows, columns: [Name]}\n";
    for body in [
        "button: {id: b, label: B, variant: loud}",
        "button: {id: b, label: B, icon: 'cds:icon_no_such_thing'}",
        "button: {id: b, label: B, enabled_when: results.picked}",
        "button: {id: b, label: B, enabled_when: missing.selected}",
        "label: {id: b, text: B, variant: primary}",
    ] {
        let error = compile(&solution, "Acme", &error_page(body, grid)).unwrap_err();
        assert!(error.line >= 10 && error.column > 0, "{body}: {error}");
    }
}

#[test]
fn narrow_date_ranges_use_short_hints_and_wide_ones_keep_the_automatic_hint() {
    let built = compiled_benchmark("k");
    let narrow = element_props(&built, "filter_dates");
    assert_eq!(narrow["StartDateHintText"], "From");
    assert_eq!(narrow["EndDateHintText"], "To");
    assert_eq!(
        element_props(&built, "dates")["StartDateHintText"],
        "Autohint"
    );
    let (_dir, solution) = element_solution();
    let wide = compile(&solution, "Acme", &element_fixture("date_range")).unwrap();
    assert_eq!(
        element_props(&wide, "dates")["StartDateHintText"],
        "Autohint"
    );
}

#[test]
fn benchmark_specs_use_the_kit_button_and_selection_conventions() {
    let built = compiled_benchmark("n");
    for (id, icon) in [
        ("preview", "cds:icon_view"),
        ("download", "cds:icon_download"),
    ] {
        let props = element_props(&built, id);
        assert_eq!(props["ButtonType"], "transparent");
        assert_eq!(props["SVGIcon"], icon);
    }
    assert!(!built.ids.contains_key("refresh"));
    let built = compiled_benchmark("h");
    assert!(built.ids.contains_key("pages"));
    let details = element_props(&built, "details");
    assert_eq!(details["ButtonType"], "transparent");
    assert_eq!(details["SVGIcon"], "cds:icon_view");
    assert!(!built.ids.contains_key("refresh"));
}

#[test]
fn panel_frames_toolbar_and_chart_and_gives_equal_flexible_width() {
    let (_dir, solution) = element_solution();
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mashup/pages/panel.page.yaml"),
    )
    .unwrap();
    let built = compile(&solution, "Acme", &text).unwrap();
    let panel = element_props(&built, "counts_panel");
    assert_eq!(panel["flex-direction"], "column");
    assert_eq!(panel["flex-grow"], 1);
    let doc = mashup::edit::parse(&built.content).unwrap();
    let id = &built.ids["counts_panel"];
    let style = &doc.root_properties()["StyleProperties"]["data"][id.as_str()]["parts"]["$"]["$"];
    assert_eq!(style["background"], "#F7F7F7");
    assert_eq!(style["border-style"], "solid");
    // Title, toolbar band and the chart wrapper sit in the panel, in that order.
    let root = doc.find_exact(id).unwrap();
    let kinds: Vec<_> = doc.at(&root.path)["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["Properties"]["Id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds.len(), 2);
    assert!(built.ids.contains_key("counts_panel--title"));
    assert!(built.ids.contains_key("counts_panel--toolbar"));
    assert!(
        doc.at(&doc
            .find_exact(&built.ids["counts_panel--toolbar"])
            .unwrap()
            .path)["Properties"]["flex-max-height"]
            == "48px"
    );
}

#[test]
fn slider_and_kpi_dials_use_the_kit_properties_and_bind_to_the_slider() {
    let (_dir, solution) = element_solution();
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages");
    let slider = compile(
        &solution,
        "Acme",
        &std::fs::read_to_string(folder.join("slider.page.yaml")).unwrap(),
    )
    .unwrap();
    let props = element_props(&slider, "level");
    assert_eq!(props["Maximum"], 200);
    assert_eq!(props["Value"], 35);
    assert_eq!(props["DisplayMinMaxLabels"], false);
    assert_eq!(props["Label"], "Level");
    let built = compile(
        &solution,
        "Acme",
        &std::fs::read_to_string(folder.join("kpi_dial.page.yaml")).unwrap(),
    )
    .unwrap();
    let half = element_props(&built, "dial");
    assert_eq!(half["Layout"], "dial");
    assert_eq!(half["StartAngle"], 90);
    assert_eq!(half["EndAngle"], 270);
    assert_eq!(half["UnitOfMeasure"], "(%)");
    assert_eq!(half["ShowUOMLabel"], true);
    let horizontal = element_props(&built, "bar");
    assert_eq!(horizontal["Layout"], "horizontal");
    assert_eq!(horizontal["Direction"], "left to right");
    assert_eq!(horizontal["MaximumValue"], 50);
    let content: Value = serde_json::from_str(&built.content).unwrap();
    let bound = |source: &str, target: &str| {
        content["DataBindings"].as_array().unwrap().iter().any(|b| {
            b["SourceId"] == built.ids[source].as_str()
                && b["TargetId"] == built.ids[target].as_str()
        })
    };
    assert!(bound("level", "dial"));
    assert!(!bound("level", "bar"));
}

#[test]
fn page_cards_use_the_regular_label_size_for_titles() {
    let built = compiled_benchmark("a");
    let doc = mashup::edit::parse(&built.content).unwrap();
    let card = doc.find_exact(&built.ids["focus_1"]).unwrap();
    let caption = doc
        .widgets()
        .into_iter()
        .find(|w| w.path.starts_with(&card.path) && w.type_id == "ptcslabel")
        .unwrap();
    assert_eq!(doc.at(&caption.path)["Properties"]["LabelType"], "label");
}

#[test]
fn combo_chart_draws_the_listed_series_as_bars_and_lines() {
    let built = compiled_benchmark("a");
    let props = element_props(&built, "trend");
    assert_eq!(props["NumberOfSeries"], 2);
    assert_eq!(props["DataField1"], "Value");
    assert_eq!(props["Series1Type"], "bar");
    assert_eq!(props["DataField2"], "Value02");
    assert_eq!(props["Series2Type"], "line");
    let doc = mashup::edit::parse(&built.content).unwrap();
    assert_eq!(
        doc.at(&doc.find_exact(&built.ids["trend"]).unwrap().path)["Properties"]["Type"],
        "ptcschartcombo"
    );
}

#[test]
fn panel_slider_dial_and_combo_errors_are_located() {
    let (_dir, solution) = element_solution();
    for body in [
        "slider: {id: s, min: 5, max: 5}",
        "slider: {id: s, mystery: 1}",
        "kpi_dial: {id: d, kind: spiral}",
        "kpi_dial: {id: d, labels: true}",
        "chart: {id: c, kind: combo, data: rows, x: Name}",
        "chart: {id: c, kind: combo, data: rows, x: Name, series: [{field: Count, kind: area}]}",
        "chart: {id: c, kind: bar, data: rows, x: Name, series: [{field: Count, kind: bar}]}",
        "panel: {id: p, toolbar: 3}",
        "label: {id: l, text: x, toolbar: [{label: {id: z, text: y}}]}",
    ] {
        let error = compile(&solution, "Acme", &error_page(body, "")).unwrap_err();
        assert!(error.line >= 10 && error.column > 0, "{body}: {error}");
    }
}

#[test]
fn benchmark_q2_golden() {
    benchmark("q2");
}
#[test]
fn benchmark_q3_golden() {
    benchmark("q3");
}
#[test]
fn benchmark_q4_golden() {
    benchmark("q4");
}

#[test]
fn combined_columns_share_fill_without_collapsing_chart_or_status_cards() {
    let built = compiled_benchmark("q2");
    let doc = mashup::edit::parse(&built.content).unwrap();
    for id in ["split", "metric_grid", "chart_side", "status_side"] {
        let widget = doc.find_exact(&built.ids[id]).unwrap();
        let path = if widget.type_id == "flexcontainer" {
            widget.path
        } else {
            widget.path[..widget.path.len() - 1].to_vec()
        };
        let props = &doc.at(&path)["Properties"];
        assert_eq!(props["flex-basis"], "0px", "{id}");
        assert_eq!(
            props["flex-grow"],
            if id == "chart_side" { 2 } else { 1 },
            "{id}"
        );
    }
    for id in ["st1", "st2", "st3"] {
        assert_eq!(element_props(&built, id)["flex-grow"], 0);
        assert_eq!(element_props(&built, id)["flex-shrink"], 0);
    }
    let chart = doc.find_exact(&built.ids["output_trend"]).unwrap();
    let minimum = doc.at(&chart.path[..chart.path.len() - 1])["Properties"]["flex-min-height"]
        .as_str()
        .unwrap()
        .trim_end_matches("px")
        .parse::<u32>()
        .unwrap();
    assert!(minimum >= 240);
}

#[test]
fn combined_chart_row_is_equal_and_page_owns_overflow_through_pagination() {
    let built = compiled_benchmark("q3");
    let doc = mashup::edit::parse(&built.content).unwrap();
    let mut parents = vec![];
    for id in ["c_pie", "c_bar", "c_line"] {
        let chart = doc.find_exact(&built.ids[id]).unwrap();
        let path = chart.path[..chart.path.len() - 1].to_vec();
        parents.push(path[..path.len() - 1].to_vec());
        let props = &doc.at(&path)["Properties"];
        assert_eq!(props["flex-grow"], 1);
        assert_eq!(props["flex-basis"], "0px");
        assert_eq!(element_props(&built, id)["LegendPosition"], "bottom");
    }
    assert!(parents.windows(2).all(|p| p[0] == p[1]));
    let pager = doc.find_exact(&built.ids["pages"]).unwrap();
    for length in 1..pager.path.len() {
        let props = &doc.at(&pager.path[..length])["Properties"];
        if props["Type"] == "flexcontainer" {
            assert_eq!(props["ShowVerticalScrollbar"], false);
            assert_ne!(props["scrolling"], "scroll");
        }
    }
}

#[test]
fn field_only_filter_reserves_280px_and_keeps_footer_and_dates_inside() {
    let built = compiled_benchmark("q1");
    let props = element_props(&built, "filters");
    assert_eq!(props["flex-basis"], "280px");
    assert_eq!(props["flex-grow"], 0);
    assert_eq!(element_props(&built, "main")["flex-grow"], 1);
    let date = element_props(&built, "period");
    assert_eq!(date["Width"], 240);
    assert_eq!(date["StartDateHintText"], "From");
    let doc = mashup::edit::parse(&built.content).unwrap();
    let fields = doc.find("filters").unwrap();
    assert_eq!(doc.at(&fields.path)["Properties"]["flex-grow"], 1);
    assert_eq!(
        doc.at(&fields.path)["Properties"]["justify-content"],
        "flex-start"
    );
    let footer = doc.find("filter-footer").unwrap();
    assert_eq!(doc.at(&footer.path)["Properties"]["flex-grow"], 1);
    assert_eq!(doc.at(&footer.path)["Properties"]["flex-max-height"], "");
    assert_eq!(doc.at(&fields.path)["Properties"]["flex-shrink"], 1);
    assert_eq!(doc.at(&fields.path)["Properties"]["flex-basis"], "auto");
    // Direct controls stack by their native heights and margins.
    assert_eq!(doc.at(&fields.path)["Properties"]["flex-min-height"], "");
    assert_eq!(
        doc.at(&fields.path)["Properties"]["ShowVerticalScrollbar"],
        false
    );
    let column = doc.find("filter-column").unwrap();
    assert_eq!(
        doc.root_properties()["StyleProperties"]["data"][&column.id]["parts"]["$"]["$"]
            ["max-height"],
        "calc(100vh - 48px)"
    );
}

#[test]
fn selected_fields_read_the_named_grid_and_preserve_field_types_and_dot_alignment() {
    let built = compiled_benchmark("q4");
    let doc = mashup::edit::parse(&built.content).unwrap();
    assert_eq!(element_props(&built, "fleet")["RowSelection"], "single");
    let value: Value = serde_json::from_str(&built.content).unwrap();
    for field in ["ThingName", "Status", "Model"] {
        let expression = &built.ids[&format!("fleet.selected.{field}")];
        assert!(value["DataBindings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["SourceId"] == built.ids["fleet"]
                && b["TargetId"] == *expression
                && b["PropertyMaps"][0]["SourceProperty"] == "SelectedRows"));
        let widget = doc.find_exact(expression).unwrap();
        assert!(doc.at(&widget.path)["Properties"]["Expression"]
            .as_str()
            .unwrap()
            .contains(&format!("[\"{field}\"]")));
    }
    for pair in ["d_name", "d_status", "d_model"] {
        assert_eq!(element_props(&built, &format!("{pair}--dot"))["Width"], 16);
    }
}

#[test]
fn layout_options_and_selection_errors_are_located_and_raw_layout_remains_supported() {
    let (_dir, solution) = element_solution();
    for body in [
        "row: {id: r, weight: 0}",
        "column: {id: r, weight: -1}",
        "label: {id: r, text: A, gap: 4}",
        "label: {id: r, text: A, legend: false}",
        "label: {id: r, props: {LabelText: {bind: missing.selected.Name}}}",
        "label: {id: r, props: {LabelText: {bind: grid.selected.Missing}}}",
    ] {
        let text = error_page(body, "      - grid: {id: grid, data: rows}\n");
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.line >= 10 && error.column > 0, "{body}: {error}");
    }
    let text = error_page(
        "row: {id: r, gap: 12, children: [{label: {id: a, text: A}}, {label: {id: b, text: B}}]}",
        "",
    );
    let built = compile(&solution, "Acme", &text).unwrap();
    assert_eq!(element_props(&built, "r")["flex-direction"], "row");
    let raw = text.replace(
        "row: {id: r, gap: 12,",
        "widget: {id: r, type: flexcontainer, props: {flex-direction: row},",
    );
    compile(&solution, "Acme", &raw).unwrap();
    let text = element_fixture("chart_line").replace("kind: line", "kind: line, legend: false");
    let built = compile(&solution, "Acme", &text).unwrap();
    assert!(built.content.contains("\"HideLegend\": true"));
}

#[test]
fn selection_stays_on_its_grid_with_shared_data_and_pagination() {
    let (_dir, solution) = element_solution();
    let text = error_page("label: {id: selected, props: {LabelText: {bind: first.selected.Name}}}", "      - grid: {id: first, data: rows}\n      - pagination: {id: pager, for: first}\n      - grid: {id: second, data: rows}\n");
    let built = compile(&solution, "Acme", &text).unwrap();
    let value: Value = serde_json::from_str(&built.content).unwrap();
    let expression = &built.ids["first.selected.Name"];
    let sources: Vec<_> = value["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["TargetId"] == *expression)
        .collect();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0]["SourceId"], built.ids["first"]);
    assert_eq!(
        sources[0]["PropertyMaps"][0]["SourceProperty"],
        "SelectedRows"
    );
    assert_eq!(
        sources[0]["PropertyMaps"][0]["SourcePropertyBaseType"],
        "INFOTABLE"
    );
    assert!(built.content.contains("rows.rows || rows"));
    assert_eq!(element_props(&built, "first")["RowSelection"], "single");
    assert_eq!(built, compile(&solution, "Acme", &text).unwrap());
}

#[test]
fn gap_and_explicit_minimum_props_override_layout_defaults() {
    let (_dir, solution) = element_solution();
    let text = error_page("row: {id: r, gap: 12, props: {flex-min-height: 123px}, children: [{label: {id: a, text: A}}, {label: {id: b, text: B}}]}", "");
    let built = compile(&solution, "Acme", &text).unwrap();
    assert_eq!(element_props(&built, "r")["flex-min-height"], "123px");
    assert_eq!(element_props(&built, "b")["Margin"], "0 0 0 12");
}

#[test]
fn combo_secondary_axis_uses_native_axis_count_and_series_title() {
    let (_dir, solution) = element_solution();
    let text = error_page("chart: {id: c, kind: combo, data: rows, x: Name, series: [{field: Count, kind: line, title: Throughput, axis: secondary}]}", "");
    let built = compile(&solution, "Acme", &text).unwrap();
    let props = element_props(&built, "c");
    assert_eq!(props["DisplaySeries1On"], "yaxis2");
    assert_eq!(props["NumberOfYaxes"], 2);
    assert_eq!(props["YAxis2Label"], "Throughput");
    assert_eq!(props["HideYAxis2"], false);
    assert_eq!(props["DisableLegendGrouping"], true);
    assert_eq!(props["LegendPosition"], "bottom");
    assert_eq!(props["HideLegend"], false);
    let built = compile(&solution, "Acme", &text.replace(", title: Throughput", "")).unwrap();
    assert_eq!(element_props(&built, "c")["YAxis2Label"], "Count");
    let built = compile(&solution, "Acme", &text.replace("secondary", "primary")).unwrap();
    assert_eq!(element_props(&built, "c")["DisplaySeries1On"], "yaxis1");
    assert_eq!(element_props(&built, "c")["NumberOfYaxes"], 1);
}

#[test]
fn sparkline_binds_service_rows_and_overrides_the_theme_height_floor() {
    let (_dir, solution) = element_solution();
    let text = error_page("trend_tile: {id: t, label: Total, value: {bind: rows.Count}, sparkline: {field: Count, x: Name}}", "");
    let built = compile(&solution, "Acme", &text).unwrap();
    let props = element_props(&built, "t--sparkline");
    assert_eq!(props["Type"], "ptcschartline");
    for key in [
        "HideXAxis",
        "HideYAxis",
        "HideLegend",
        "SparkView",
        "HideValues",
    ] {
        assert_eq!(props[key], true, "{key}");
    }
    for key in ["ShowXAxisRuler", "ShowYAxisRuler", "ShowZeroValueRuler"] {
        assert_eq!(props[key], false, "{key}");
    }
    let doc = mashup::edit::parse(&built.content).unwrap();
    let chart = doc.find_exact(&built.ids["t--sparkline"]).unwrap();
    let wrapper = &doc.at(&chart.path[..chart.path.len() - 1])["Properties"];
    assert_eq!(wrapper["flex-min-height"], "40px");
    assert_eq!(wrapper["flex-max-height"], "40px");
    assert_eq!(wrapper["flex-size"], "fixed");
    assert_eq!(props["Height"], 40);
    assert_eq!(props["DataField1"], "Count");
    assert_eq!(props["NumberOfSeries"], 1);
    assert_eq!(props["XAxisField"], "Name");
    assert_eq!(props["Label"], "");
    assert_eq!(props["LabelType"], "label");
    for key in [
        "HorizontalAxisMaxHeight",
        "VerticalAxisMaxWidth",
        "LegendMaxWidth",
    ] {
        assert_eq!(props[key], 0, "{key}");
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
        assert_eq!(props[key], false, "{key}");
    }
    assert!(props.get("YAxisMinimumValues").is_none());
    assert!(props.get("YAxisMaximumValues").is_none());
    assert_eq!(
        element_props(&built, "t--figure")["flex-min-height"],
        "56px"
    );
    assert_eq!(element_props(&built, "t--figure")["flex-shrink"], 0);
    assert!(!doc.widgets().iter().any(|w| w.type_id == "expression2"));
    let content: Value = serde_json::from_str(&built.content).unwrap();
    let parts = &content["UI"]["Properties"]["StyleProperties"]["data"][&built.ids["t--sparkline"]]
        ["parts"];
    assert_eq!(parts["$"]["$"]["min-height"], "0px");
    assert_eq!(parts["$"]["$"]["height"], "40px");
    assert_eq!(parts["$"]["$"]["max-height"], "40px");
    for side in ["top", "bottom", "left", "right"] {
        assert_eq!(parts["chart-layout"]["$"][format!("padding-{side}")], "0px");
    }
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
    let bindings: Vec<_> = content["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["TargetId"] == built.ids["t--sparkline"])
        .collect();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["SourceArea"], "Data");
    assert_eq!(bindings[0]["SourceDetails"], "AllData");
    assert_eq!(bindings[0]["PropertyMaps"][0]["SourceProperty"], "");
    assert_eq!(bindings[0]["PropertyMaps"][0]["TargetProperty"], "Data");
    assert_eq!(built, compile(&solution, "Acme", &text).unwrap());
    compile(
        &solution,
        "Acme",
        &text.replace("field: Count", "data: rows, field: Count"),
    )
    .unwrap();
}

#[test]
fn detail_card_row_fits_its_contents_without_a_fixed_height_or_inner_scroll() {
    let built = compiled_benchmark("a");
    let doc = mashup::edit::parse(&built.content).unwrap();
    let row = doc.find_exact(&built.ids["focus_row"]).unwrap();
    for widget in doc
        .widgets()
        .iter()
        .filter(|w| w.path.starts_with(&row.path) && w.type_id == "flexcontainer")
    {
        let props = &doc.at(&widget.path)["Properties"];
        if built.ids.values().any(|id| id == &widget.id) {
            assert_ne!(props["flex-size"], "fixed", "{}", widget.id);
            assert_eq!(props["flex-max-height"], "", "{}", widget.id);
        }
        assert_eq!(props["ShowVerticalScrollbar"], false, "{}", widget.id);
        assert_eq!(props["scrolling"], "no-scroll", "{}", widget.id);
    }
    assert_eq!(element_props(&built, "focus_row")["flex-basis"], "auto");
    assert_eq!(element_props(&built, "focus_row")["flex-shrink"], 0);
}

#[test]
fn detail_state_maps_bound_value_to_native_shape_formatter() {
    let built = compiled_benchmark("a");
    let extracted = mashup::extract(&built.entity).unwrap();
    let doc = mashup::edit::parse(&extracted.content).unwrap();
    for (key, id) in built
        .ids
        .iter()
        .filter(|(key, _)| key.starts_with("focus_") && key.ends_with("--dot"))
    {
        let dot = doc.find_exact(id).unwrap();
        let pair = doc
            .find_exact(&built.ids[key.strip_suffix("--dot").unwrap()])
            .unwrap();
        assert_eq!(dot.type_id, "shape");
        let caption = doc
            .find_exact(&built.ids[&key.replace("--dot", "--caption")])
            .unwrap();
        assert_eq!(
            &dot.path[..dot.path.len() - 1],
            &caption.path[..caption.path.len() - 1]
        );
        assert_eq!(&dot.path[..dot.path.len() - 2], pair.path);
        assert_eq!(
            doc.at(&dot.path[..dot.path.len() - 1])["Properties"]["flex-min-height"],
            "12px"
        );
        let props = &doc.at(&dot.path)["Properties"];
        assert_eq!(props["Width"], 12);
        assert_eq!(props["Height"], 12);
        assert_eq!(
            props["ShapeFormatter"]["StateDefinition"],
            "Acme.BenchKPI_State"
        );
        assert!(doc.value()["DataBindings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["TargetId"] == *id && b["PropertyMaps"][0]["TargetProperty"] == "Data"));
    }
    let dot = element_props(&built, "focus_1_objective--dot");
    assert_eq!(dot["Type"], "shape");
    assert_eq!(dot["ShapeType"], "circle");
    assert_eq!(
        dot["ShapeFormatter"]["StateDefinition"],
        "Acme.BenchKPI_State"
    );
    assert_eq!(dot["ShapeFormatter"]["FieldName"], "Objective");
    assert_eq!(dot["ShapeFormatter"]["StateDefinitionType"], "numeric");
    assert!(!dot["ShapeFormatter"]["StateFormats"]
        .as_array()
        .unwrap()
        .is_empty());
    let value: Value = serde_json::from_str(&built.content).unwrap();
    assert!(value["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["TargetId"] == built.ids["focus_1_objective--dot"]
            && b["PropertyMaps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["TargetProperty"] == "Data" && p["SourceProperty"] == "Objective")));
}

#[test]
fn small_element_errors_have_source_locations() {
    let (_dir, solution) = element_solution();
    for body in [
        "chart: {id: c, kind: combo, data: rows, x: Name, series: [{field: Count, kind: line, axis: third}]}",
        "trend_tile: {id: t, label: Total, value: {bind: rows.Count}, sparkline: {field: Missing, x: Name}}",
        "trend_tile: {id: t, label: Total, value: {bind: rows.Count}, sparkline: {field: Name, x: Name}}",
        "trend_tile: {id: t, label: Total, value: {bind: rows.Count}, sparkline: {field: Count, x: Missing}}",
        "trend_tile: {id: t, label: Total, value: {bind: rows.Count}, sparkline: {field: Count, x: Name, data: missing}}",
        "detail_pair: {id: d, label: State, value: {bind: rows.Name}, state: Acme.Unknown}",
        "detail_pair: {id: d, label: State, value: 1, state: Acme.Unknown}",
        "detail_pair: {id: d, label: State, value: {bind: rows.Name}, state: Acme.Unknown, status: ok}",
        "label: {id: l, text: Bad, auto_select: false}",
    ] {
        let error = compile(&solution, "Acme", &error_page(body, "")).unwrap_err();
        assert!(error.line >= 10 && error.column > 0, "{body}: {error}");
    }
}

#[test]
fn selection_bindings_default_to_first_row_and_allow_opt_out() {
    let built = compiled_benchmark("q4");
    assert_eq!(element_props(&built, "fleet")["DefaultSelectedRows"], "1");
    let (_dir, solution) = element_solution();
    let text = error_page(
        "label: {id: detail, props: {LabelText: {bind: g.selected.Name}}}",
        "      - grid: {id: g, data: rows, auto_select: false}\n",
    );
    let built = compile(&solution, "Acme", &text).unwrap();
    assert_eq!(element_props(&built, "g")["DefaultSelectedRows"], "");
    let built = compile(
        &solution,
        "Acme",
        &text.replace("auto_select: false", "props: {DefaultSelectedRows: '2'}"),
    )
    .unwrap();
    assert_eq!(element_props(&built, "g")["DefaultSelectedRows"], "2");
    let built = compile(
        &solution,
        "Acme",
        &error_page("grid: {id: g, data: rows, auto_select: true}", ""),
    )
    .unwrap();
    assert_eq!(element_props(&built, "g")["DefaultSelectedRows"], "1");
    assert_eq!(element_props(&built, "g")["RowSelection"], "single");
}

#[test]
fn fixed_detail_status_still_colours_its_dot() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "detail_pair: {id: d, label: State, value: {bind: rows.Name}, status: warning}",
            "",
        ),
    )
    .unwrap();
    assert_eq!(element_props(&built, "d--dot")["LabelText"], "\u{25CF}");
}

fn doc_of(built: &super::compile::Artifacts) -> mashup::edit::MashupDocument {
    mashup::edit::parse(&built.content).unwrap()
}

fn widget_style(built: &super::compile::Artifacts, id: &str) -> Value {
    doc_of(built).root_properties()["StyleProperties"]["data"][&built.ids[id]].clone()
}

fn binding_between(built: &super::compile::Artifacts, from: &str, to: &str) -> Option<Value> {
    doc_of(built).value()["DataBindings"]
        .as_array()?
        .iter()
        .find(|b| b["SourceId"] == from && b["TargetId"] == to)
        .cloned()
}

#[test]
fn side_panel_is_a_grey_276px_column_whose_tab_holds_the_children() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "row: {id: shell, children: [{side_panel: {id: side, title: Compare, help: Pick assets, tab: Filters, children: [{label: {id: note, text: Hello}}]}}]}",
            "",
        ),
    )
    .unwrap();
    let side = element_props(&built, "side");
    assert_eq!(side["flex-size"], "fixed");
    assert_eq!(side["flex-min-width"], "276px");
    assert_eq!(side["flex-max-width"], "276px");
    assert_eq!(
        widget_style(&built, "side")["parts"]["$"]["$"]["background"],
        "#F7F7F7"
    );
    let doc = doc_of(&built);
    let root = doc.find_exact(&built.ids["side"]).unwrap().path;
    let inside: Vec<_> = doc
        .widgets()
        .into_iter()
        .filter(|w| w.path.starts_with(&root))
        .collect();
    let title = inside
        .iter()
        .find(|w| doc.at(&w.path)["Properties"]["LabelText"] == "Compare")
        .expect("title label");
    assert_eq!(doc.at(&title.path)["Properties"]["LabelType"], "header");
    let help = inside
        .iter()
        .find(|w| doc.at(&w.path)["Properties"]["TooltipField"] == "Pick assets")
        .expect("help icon");
    assert_eq!(
        doc.at(&help.path)["Properties"]["SVGIcon"],
        "cds:icon_help_circle"
    );
    let tabs = inside.iter().find(|w| w.type_id == "ptcstabset").unwrap();
    let tabs_props = &doc.at(&tabs.path)["Properties"];
    assert_eq!(tabs_props["NumberOfTabs"], 1);
    assert_eq!(tabs_props["Tab1Name"], "Filters");
    let note = doc.find_exact(&built.ids["note"]).unwrap();
    assert!(
        note.path.starts_with(&tabs.path),
        "children live in the tab"
    );
    // No tab label or title is invented: the same spec without help has no icon button.
    let plain = compile(
        &solution,
        "Acme",
        &error_page("side_panel: {id: side, title: Compare}", ""),
    )
    .unwrap();
    assert!(!doc_of(&plain)
        .widgets()
        .iter()
        .any(|w| w.type_id == "ptcsbutton"));
    assert_eq!(
        compile(
            &solution,
            "Acme",
            &error_page("side_panel: {id: side, title: Compare}", "")
        )
        .unwrap(),
        plain
    );
}

#[test]
fn lists_feed_each_other_and_show_an_empty_notice() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "column: {id: c, children: [{list: {id: all, data: rows, display: Name, selection: multiple, empty: No rows}}, {list: {id: picked, data: all.selected, display: Name}}]}",
            "",
        ),
    )
    .unwrap();
    let all = element_props(&built, "all");
    assert_eq!(all["MultiSelect"], true);
    assert_eq!(all["DisplayField"], "Name");
    assert_eq!(all["Label"], "");
    assert_eq!(all["ClearSelectionItem"], false);
    let picked = element_props(&built, "picked");
    assert_eq!(picked["MultiSelect"], false);
    let binding =
        binding_between(&built, &built.ids["all"], &built.ids["picked"]).expect("selection source");
    assert_eq!(
        binding["PropertyMaps"][0]["SourceProperty"],
        "SelectedItems"
    );
    assert_eq!(binding["PropertyMaps"][0]["TargetProperty"], "Data");
    // The list has no empty-state property, so a notice shows while its rows are empty.
    let doc = doc_of(&built);
    let notice = doc
        .widgets()
        .into_iter()
        .find(|w| doc.at(&w.path)["Properties"]["Label"] == "No rows")
        .expect("empty notice");
    assert_eq!(
        doc.at(&notice.path)["Properties"]["SVGIcon"],
        "cds:icon_not_visible"
    );
    assert!(doc.value()["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["TargetId"]
            == doc.at(&notice.path[..notice.path.len() - 1])["Properties"]["Id"]
            && b["PropertyMaps"][0]["TargetProperty"] == "Visible"));
    let list = doc.find_exact(&built.ids["all"]).unwrap();
    let list_row = &doc.at(&list.path[..list.path.len() - 1])["Properties"];
    assert_eq!(list_row["Visible"], false);
    let region = doc.at(&list.path[..list.path.len() - 2]);
    assert_eq!(region["Properties"]["flex-direction"], "row");
    let branch_height = region["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(mashup::layout::height_floor)
        .max()
        .unwrap();
    assert_eq!(mashup::layout::height_floor(region), branch_height);
    let row = &doc.at(&notice.path[..notice.path.len() - 1])["Properties"];
    assert_eq!(row["flex-size"], "range");
    assert_eq!(row["flex-max-height"], "");
    assert_eq!(row["justify-content"], "center");
    assert_eq!(row["flex-min-height"], "100px");
    assert!(doc.value()["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["TargetId"] == list_row["Id"]
            && b["PropertyMaps"][0]["TargetProperty"] == "Visible"));
    assert!(built
        .content
        .contains("!!(rows && (rows.rows || rows).length)"));
    assert_eq!(
        compile(&solution, "Acme", &error_page("column: {id: c, children: [{list: {id: all, data: rows, display: Name, selection: multiple, empty: No rows}}, {list: {id: picked, data: all.selected, display: Name}}]}", "")).unwrap(),
        built
    );
}

#[test]
fn info_icons_follow_headings_labels_and_page_headers() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "column: {id: c, children: [{label: {id: l, text: Selected, info: Pick some}}, {heading: {id: h, text: Title, info: About}}, {page_header: {id: ph, title: Cards, info: Help text}}]}",
            "",
        ),
    )
    .unwrap();
    for (id, icon, tip) in [
        ("l--info", "cds:icon_info", "Pick some"),
        ("h--info", "cds:icon_info", "About"),
        ("ph--info", "cds:icon_help_circle", "Help text"),
    ] {
        let props = element_props(&built, id);
        assert_eq!(props["SVGIcon"], icon, "{id}");
        assert_eq!(props["TooltipField"], tip, "{id}");
        assert_eq!(props["Disabled"], true, "{id}");
    }
    assert_eq!(element_props(&built, "l--title")["LabelType"], "label");
    assert_eq!(element_props(&built, "h--title")["LabelType"], "header");
    assert_eq!(element_props(&built, "l--title")["LabelText"], "Selected");
    // The wrapper row keeps the authored id, so other references to it still hold.
    assert_eq!(element_props(&built, "l")["flex-direction"], "row");
}

#[test]
fn text_field_icons_and_filter_title_and_chevron() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "column: {id: c, children: [{text_field: {id: a, icon: search}}, {text_field: {id: b, icon: 'cds:icon_search'}}]}",
            "",
        ),
    )
    .unwrap();
    assert_eq!(element_props(&built, "a")["Icon"], "cds:icon_search");
    assert_eq!(element_props(&built, "b")["Icon"], "cds:icon_search");
    let error = compile(
        &solution,
        "Acme",
        &error_page("text_field: {id: a, icon: nonesuch}", ""),
    )
    .unwrap_err();
    assert!(error.line >= 10 && error.column > 0, "{error}");
    let panel = |extra: &str| {
        compile(
            &solution,
            "Acme",
            &error_page(
                &format!("filter_panel: {{id: f, title: Filter, {extra} fields: [{{text_field: {{id: q, hint: Find}}}}], footer: [apply]}}"),
                "",
            ),
        )
        .unwrap()
    };
    let chevron = panel("collapsible: true,");
    assert_eq!(element_props(&chevron, "f--title")["LabelType"], "title");
    assert_eq!(element_props(&chevron, "f--title")["LabelText"], "Filter");
    let doc = doc_of(&chevron);
    let column = doc
        .widgets()
        .into_iter()
        .find(|w| w.display_name == "filter-column")
        .unwrap();
    let props = &doc.at(&column.path)["Properties"];
    assert_eq!(props["EnableExpandCollapse"], true);
    assert_eq!(props["ShowExpandCollapseTab"], true);
    assert_eq!(props["ExpandCollapseTabLocation"], "inside");
    let removed = panel("collapsible: false,");
    let doc = doc_of(&removed);
    let column = doc
        .widgets()
        .into_iter()
        .find(|w| w.display_name == "filter-column")
        .unwrap();
    assert_eq!(
        doc.at(&column.path)["Properties"]["ShowExpandCollapseTab"],
        false
    );
}

fn cell_solution() -> (tempfile::TempDir, Solution) {
    let (dir, _) = element_solution();
    let mut cell = authoring::session::Session::new(
        "Acme.RowCell_MU",
        "Acme",
        "blank",
        "PTC Convergence Theme",
    )
    .unwrap();
    cell.add_parameter("Name:STRING:IN").unwrap();
    std::fs::write(
        dir.path().join("acme/Mashups/Acme.RowCell_MU.xml"),
        cell.entity,
    )
    .unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution)
}

#[test]
fn collection_maps_rows_into_an_existing_cell_mashup() {
    let (_dir, solution) = cell_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "collection: {id: c, data: rows, mashup: Acme.RowCell_MU, map: {Name: Name}, per_row: 2, across: 800, empty: Nothing here}",
            "",
        ),
    )
    .unwrap();
    assert!(built.cells.is_empty());
    let props = element_props(&built, "c");
    assert_eq!(props["Mashup"], "Acme.RowCell_MU");
    assert_eq!(
        props["MashupPropertyBinding"],
        serde_json::json!({"Name": "Name"})
    );
    assert_eq!(props["NoDataText"], "Nothing here");
    // Two cells fit 800px less the paddings, the scrollbar and the gaps (the collection's rule).
    assert_eq!(props["CellWidth"], 319);
    let doc = doc_of(&built);
    assert!(doc.value()["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |b| b["TargetId"] == built.ids["c"] && b["PropertyMaps"][0]["TargetProperty"] == "Data"
        ));
    for (body, why) in [
        ("collection: {id: c, data: rows, mashup: Acme.RowCell_MU, map: {Missing: Name}}", "no field"),
        ("collection: {id: c, data: rows, mashup: Acme.RowCell_MU, map: {Name: Nope}}", "no parameter"),
        ("collection: {id: c, data: rows}", "needs exactly one"),
        ("collection: {id: c, data: rows, mashup: Acme.RowCell_MU, cell: {title: Name}, map: {Name: Name}}", "needs exactly one"),
        ("collection: {id: c, data: rows, mashup: Acme.RowCell_MU}", "needs map"),
        ("collection: {id: c, data: rows, cell: {title: Name}, map: {Name: Name}}", "map belongs"),
        ("collection: {id: c, data: rows, cell: {title: Name}, per_row: 0}", "above zero"),
        ("collection: {id: c, data: rows, cell: {kpi_dial: Name}}", "numeric"),
        ("collection: {id: c, data: rows, cell: {}}", "cell needs"),
        ("collection: {id: c, mashup: Acme.RowCell_MU, map: {Name: Name}}", "needs data"),
    ] {
        let error = compile(&solution, "Acme", &error_page(body, "")).unwrap_err();
        assert!(error.line >= 10 && error.column > 0, "{body}: {error}");
        assert!(error.message.contains(why), "{body}: {error}");
    }
}

#[test]
fn collection_cell_is_generated_even_when_it_exists() {
    let (dir, solution) = element_solution();
    let text = error_page(
        "collection: {id: c, size: fill, data: rows, per_row: 2, cell: {title: Name, kpi_dial: Count}}",
        "",
    );
    let built = compile(&solution, "Acme", &text).unwrap();
    assert_eq!(built, compile(&solution, "Acme", &text).unwrap());
    assert_eq!(built.cells.len(), 1);
    let cell = &built.cells[0];
    assert_eq!(cell.name, "Acme.ErrorsCell_MU");
    let parsed = schema::parse(&cell.spec).expect("the generated spec is a valid page spec");
    assert_eq!(parsed.page.value.parameters.len(), 2);
    let params: std::collections::BTreeMap<_, _> =
        crate::core::mashup::describe::mashup_parameters(&cell.artifacts.entity)
            .unwrap()
            .into_iter()
            .map(|p| (p.name, p.base_type))
            .collect();
    assert_eq!(
        params,
        std::collections::BTreeMap::from([
            ("Count".to_string(), "NUMBER".to_string()),
            ("Name".to_string(), "STRING".to_string())
        ])
    );
    let cell_doc = doc_of(&cell.artifacts);
    let label = cell_doc
        .widgets()
        .into_iter()
        .find(|w| w.type_id == "ptcslabel")
        .unwrap();
    let dial = cell_doc
        .widgets()
        .into_iter()
        .find(|w| w.type_id == "ptcskpidial")
        .unwrap();
    let bound = |target: &str| {
        cell_doc.value()["DataBindings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["SourceArea"] == "Mashup" && b["TargetId"] == target)
    };
    assert!(bound(&label.id) && bound(&dial.id));
    assert_eq!(cell_doc.at(&dial.path)["Properties"]["Label"], "");
    let props = element_props(&built, "c");
    assert_eq!(props["Mashup"], "Acme.ErrorsCell_MU");
    assert_eq!(
        props["MashupPropertyBinding"],
        serde_json::json!({"Count": "Count", "Name": "Name"})
    );
    assert_eq!(props["CellHeight"], 270);
    assert_eq!(props["AutoSelectFirstRow"], false);
    assert_eq!(
        widget_style(&built, "c")["parts"]["cell"]["$"]["border-style"],
        "solid"
    );
    // Compilation stays independent of existing files; the builder checks ownership.
    std::fs::write(
        dir.path().join("acme/Mashups/Acme.ErrorsCell_MU.xml"),
        &cell.artifacts.entity,
    )
    .unwrap();
    let again = compile(
        &Solution::load(&dir.path().join("twaco.toml")).unwrap(),
        "Acme",
        &text,
    )
    .unwrap();
    assert_eq!(again.cells, built.cells);
    assert_eq!(again.content, built.content);
}

#[test]
fn nav_menu_lists_groups_and_selects_the_first_item() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "nav_menu: {id: m, search: Search Menu..., groups: [{title: One, items: [A]}, {title: Two, items: [B, C]}]}",
            "",
        ),
    )
    .unwrap();
    let search = element_props(&built, "m--search");
    assert_eq!(search["HintText"], "Search Menu...");
    assert_eq!(search["Icon"], "cds:icon_search");
    assert_eq!(element_props(&built, "m--group-2")["LabelText"], "Two");
    assert_eq!(element_props(&built, "m--item-2-2")["Label"], "C");
    for id in ["m--item-1-1", "m--item-2-1", "m--item-2-2"] {
        let props = element_props(&built, id);
        assert_eq!(props["ButtonType"], "transparent", "{id}");
        assert_eq!(props["Size"], "standard", "{id}");
        assert_eq!(props["LabelAlignment"], "left", "{id}");
    }
    assert_eq!(
        widget_style(&built, "m--item-1-1")["parts"]["$"]["$"]["background"],
        "#e4e7e9"
    );
    for id in ["m--item-1-1", "m--item-2-1", "m--item-2-2"] {
        let style = widget_style(&built, id);
        assert_eq!(style["parts"]["label"]["$"]["font-size"], "14px");
        assert_eq!(style["parts"]["label"]["$"]["font-weight"], "600");
        assert_eq!(style["parts"]["label"]["$"]["color"], "#080909");
        assert_eq!(style["parts"]["$"]["$"]["width"], "100%");
        assert_eq!(style["parts"]["$"]["$"]["padding-left"], "24px");
    }
    assert_eq!(
        widget_style(&built, "m--group-1")["parts"]["label"]["$"]["font-size"],
        "14px"
    );
    let error = compile(&solution, "Acme", &error_page("nav_menu: {id: m}", "")).unwrap_err();
    assert!(
        error.line >= 10 && error.message.contains("needs groups"),
        "{error}"
    );
}

#[test]
fn batch_2a_elements_report_located_errors() {
    let (_dir, solution) = element_solution();
    for (body, why) in [
        ("side_panel: {id: s}", "needs title"),
        (
            "side_panel: {id: s, title: A, selection: single}",
            "does not belong",
        ),
        ("list: {id: l, data: rows}", "needs data and display"),
        (
            "list: {id: l, data: rows, display: Name, selection: both}",
            "single or multiple",
        ),
        ("list: {id: l, data: rows, display: Missing}", "Missing"),
        (
            "list: {id: l, data: nope.selected, display: Name}",
            "names a list",
        ),
        ("grid: {id: g, data: rows.selected}", "feeds a list"),
        ("label: {id: l, text: x, empty: no}", "does not belong"),
        ("grid: {id: g, data: rows, info: x}", "does not belong"),
        (
            "button: {id: b, label: x, collapsible: true}",
            "does not belong",
        ),
        ("text_field: {id: t, per_row: 2}", "does not belong"),
    ] {
        let error = compile(&solution, "Acme", &error_page(body, "")).unwrap_err();
        assert!(error.line >= 10 && error.column > 0, "{body}: {error}");
        assert!(error.message.contains(why), "{body}: {error}");
    }
}

#[test]
fn page_parameters_bind_into_a_page() {
    let (_dir, solution) = element_solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Params_MU\n  template: blank\n  parameters: {Title: STRING}\n  data:\n    rows: {service: Acme.Data.GetRows, on: load}\n  slots:\n    center:\n      - label: {id: t, props: {LabelText: {bind: param.Title}}}\n";
    let built = compile(&solution, "Acme", text).unwrap();
    assert_eq!(
        crate::core::mashup::describe::mashup_parameters(&built.entity).unwrap()[0].name,
        "Title"
    );
    assert!(doc_of(&built).value()["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["SourceArea"] == "Mashup" && b["TargetId"] == built.ids["t"]));
    let unknown = text.replace("param.Title", "param.Nope");
    let error = compile(&solution, "Acme", &unknown).unwrap_err();
    assert!(error.line >= 11, "{error}");
}

#[test]
fn list_and_side_panel_match_equivalent_primitive_sequences() {
    use authoring::{session::Session, AddRequest, Recipe, SourceAddRequest};
    let (_dir, solution) = element_solution();
    for (body, kind, props) in [
        (
            "list: {id: target, data: rows, display: Name, selection: multiple}",
            "ptcslist",
            vec![],
        ),
        (
            "side_panel: {id: target, title: Compare, help: Tip, tab: Filters}",
            "flexcontainer",
            vec![("flex-direction".to_string(), "column".to_string())],
        ),
    ] {
        let text=format!("version: 1\nrules: v1\npage:\n  name: Acme.Elements_MU\n  template: blank\n  data:\n    rows: {{service: Acme.Data.GetRows, on: load}}\n  slots:\n    center:\n      - {body}\n");
        let built = compile(&solution, "Acme", &text).unwrap();
        let mut primitive =
            Session::new("Acme.Elements_MU", "Acme", "blank", "PTC Convergence Theme").unwrap();
        let target = primitive
            .add(
                &solution,
                &AddRequest {
                    mashup: "Acme.Elements_MU".into(),
                    widget: kind.into(),
                    inputs: vec![],
                    placement: Some(mashup::edit::Placement::In("center".into())),
                    label: None,
                    properties: props,
                },
            )
            .unwrap();
        primitive
            .source(
                &solution,
                &SourceAddRequest {
                    mashup: "Acme.Elements_MU".into(),
                    entity: "Acme.Data".into(),
                    service: "GetRows".into(),
                    dynamic: false,
                    on_load: true,
                    alias: Some("rows".into()),
                    entity_from: None,
                    refresh: None,
                    cache: false,
                    cache_keys: vec![],
                    cache_strategy: None,
                    max_result_sets: None,
                    param_values: vec![],
                },
            )
            .unwrap();
        let target_id = target.clone();
        let recipe = if kind == "ptcslist" {
            Recipe::Picker {
                widget: target,
                data: "data:Acme.Data.GetRows[as rows]".into(),
                display: "Name".into(),
                value: None,
                selected: None,
                no_none_row: true,
                auto_select_first: false,
                multiple: true,
            }
        } else {
            Recipe::SidePanel {
                container: target,
                title: "Compare".into(),
                info: Some("Tip".into()),
                tab: "Filters".into(),
            }
        };
        primitive.recipe(&solution, recipe).unwrap();
        let parsed = schema::parse(&text).unwrap();
        let ids = std::collections::BTreeMap::from([("target".to_string(), target_id)]);
        super::elements::finish_layout(&mut primitive, &parsed.page.value, &ids).unwrap();
        assert_eq!(
            canonical(&built.content),
            canonical(&primitive.document.to_content()),
            "{kind}"
        );
    }
}

#[test]
fn benchmark_l_golden() {
    benchmark("l");
}
#[test]
fn benchmark_m_golden() {
    benchmark("m");
}

#[test]
fn narrow_columns_bound_search_widths_and_titled_collapsible_filter_rows() {
    let (_dir, solution) = element_solution();
    let built = compile(&solution, "Acme", &error_page(
        "row: {id: r, children: [{side_panel: {id: s, title: Compare, children: [{text_field: {id: q}}, {text_field: {id: small, props: {Width: 120}}}, {nav_menu: {id: menu, search: Find, groups: [{title: Pages, items: [One]}]}}]}}, {filter_panel: {id: f, title: Filter, collapsible: true, fields: [{text_field: {id: fq, props: {Label: Version}}}, {dropdown: {id: version, options: rows, display: Name}}], footer: [apply, reset]}}]}", "")).unwrap();
    assert_eq!(element_props(&built, "q")["Width"], 240);
    assert_eq!(element_props(&built, "small")["Width"], 120);
    assert_eq!(element_props(&built, "menu--search")["Width"], 214);
    assert_eq!(element_props(&built, "fq")["Width"], 240);
    let doc = doc_of(&built);
    let fields = doc
        .widgets()
        .into_iter()
        .find(|w| w.display_name == "filters")
        .unwrap();
    for node in ["fq", "version"] {
        let field = doc.find_exact(&built.ids[node]).unwrap();
        let row = &field.path[..field.path.len() - 1];
        for depth in fields.path.len()..row.len() {
            let parent = &doc.at(&row[..depth])["Properties"];
            assert_eq!(parent["flex-direction"], "column");
            assert_eq!(parent["justify-content"], "flex-start");
        }
    }
    let controls = doc.at(&fields.path)["Widgets"].as_array().unwrap();
    assert_eq!(controls.len(), 3);
    assert_eq!(controls[0]["Properties"]["LabelText"], "Filter");
    assert_eq!(controls[0]["Properties"]["Margin"], "16");
    assert!(controls
        .iter()
        .all(|c| c["Properties"]["Type"] != "flexcontainer"));
}

#[test]
fn generated_cells_refresh_until_any_generated_file_is_edited() {
    for edited in [
        None,
        Some("xml"),
        Some("content"),
        Some("css"),
        Some("spec"),
        Some("baseline"),
    ] {
        let (_dir, solution) = element_solution();
        let spec = solution.root.join("collection.yaml");
        let original = error_page(
            "collection: {id: c, data: rows, cell: {title: Name, kpi_dial: Count}}",
            "",
        );
        std::fs::write(&spec, &original).unwrap();
        let guard = lock::acquire(&solution.root, "cell test", &[]).unwrap();
        let mut plan = super::build::plan(&solution, "Acme.Errors_MU", None, Some(&spec)).unwrap();
        plan.apply(&solution, &guard).unwrap();
        let cell_dir = solution.src_root().join("Acme.ErrorsCell_MU");
        let paths = [
            solution.root.join("acme/Mashups/Acme.ErrorsCell_MU.xml"),
            cell_dir.join("mashup/content.json"),
            cell_dir.join("mashup/custom.css"),
            cell_dir.join("page.yaml"),
            cell_dir.join(".page-baseline.json"),
        ];
        let unchanged = super::build::plan(&solution, "Acme.Errors_MU", None, None).unwrap();
        assert_eq!(unchanged.report["files"], serde_json::json!([]));
        if let Some(kind) = edited {
            let index = ["xml", "content", "css", "spec", "baseline"]
                .iter()
                .position(|k| *k == kind)
                .unwrap();
            let mut bytes = std::fs::read(&paths[index]).unwrap();
            bytes.extend_from_slice(b"\n ");
            std::fs::write(&paths[index], bytes).unwrap();
        }
        let before: Vec<_> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();
        std::fs::write(
            solution.src_root().join("Acme.Errors_MU/page.yaml"),
            original.replace("title: Name, ", ""),
        )
        .unwrap();
        let mut plan = super::build::plan(&solution, "Acme.Errors_MU", None, None).unwrap();
        assert!(!plan.conflicts(), "{}", plan.report);
        if edited.is_some() {
            assert_eq!(
                plan.report["cell_notices"],
                serde_json::json!(["Acme.ErrorsCell_MU: cell edited, not regenerated"])
            );
        } else {
            assert_eq!(plan.report["cell_notices"], serde_json::json!([]));
            assert_eq!(
                plan.report["cells"],
                serde_json::json!(["Acme.ErrorsCell_MU"])
            );
        }
        plan.apply(&solution, &guard).unwrap();
        let after: Vec<_> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();
        if edited.is_some() {
            assert_eq!(before, after);
            let again = super::build::plan(&solution, "Acme.Errors_MU", None, None).unwrap();
            assert_eq!(again.report["cell_notices"], plan.report["cell_notices"]);
        } else {
            assert_ne!(before, after);
            let content = std::str::from_utf8(&after[1]).unwrap();
            let doc = mashup::edit::parse(content).unwrap();
            assert!(!doc.widgets().iter().any(|w| w.type_id == "ptcslabel"));
            let dial = doc
                .widgets()
                .into_iter()
                .find(|w| w.type_id == "ptcskpidial")
                .unwrap();
            assert_eq!(doc.at(&dial.path)["Properties"]["Label"], "");
            assert_eq!(
                super::build::plan(&solution, "Acme.Errors_MU", None, None)
                    .unwrap()
                    .report["files"],
                serde_json::json!([])
            );
        }
    }
}

#[test]
fn older_generated_cell_refreshes_a_placeholder_from_its_baseline() {
    let (_dir, solution) = element_solution();
    let spec = solution.root.join("collection.yaml");
    std::fs::write(
        &spec,
        error_page(
            "collection: {id: c, data: rows, cell: {title: Name, kpi_dial: Count}}",
            "",
        ),
    )
    .unwrap();
    let guard = lock::acquire(&solution.root, "cell migration test", &[]).unwrap();
    let mut plan = super::build::plan(&solution, "Acme.Errors_MU", None, Some(&spec)).unwrap();
    plan.apply(&solution, &guard).unwrap();
    let parent_baseline = solution
        .src_root()
        .join("Acme.Errors_MU/.page-baseline.json");
    let mut parent: Value =
        serde_json::from_slice(&std::fs::read(&parent_baseline).unwrap()).unwrap();
    parent.as_object_mut().unwrap().remove("generated_cells");
    std::fs::write(
        &parent_baseline,
        serde_json::to_vec_pretty(&parent).unwrap(),
    )
    .unwrap();
    let dir = solution.src_root().join("Acme.ErrorsCell_MU");
    let baseline_path = dir.join(".page-baseline.json");
    let mut baseline =
        super::baseline::Baseline::read(&std::fs::read(&baseline_path).unwrap()).unwrap();
    let mut doc = mashup::edit::MashupDocument::new(baseline.content).unwrap();
    let dial = doc
        .widgets()
        .into_iter()
        .find(|w| w.type_id == "ptcskpidial")
        .unwrap();
    doc.set_property(&dial.path, "Label", serde_json::json!("Label"));
    baseline.content = doc.value().clone();
    let assets = mashup::Assets {
        content: mashup::to_sidecar(&baseline.content),
        css: baseline.css.clone(),
    };
    let entity_path = solution.root.join("acme/Mashups/Acme.ErrorsCell_MU.xml");
    let xml = mashup::sync(&std::fs::read(&entity_path).unwrap(), &assets)
        .unwrap()
        .0;
    std::fs::write(&entity_path, xml).unwrap();
    std::fs::write(dir.join("mashup/content.json"), &assets.content).unwrap();
    std::fs::write(&baseline_path, baseline.bytes()).unwrap();
    let mut plan = super::build::plan(&solution, "Acme.Errors_MU", None, None).unwrap();
    assert_eq!(plan.report["cell_notices"], serde_json::json!([]));
    assert_eq!(
        plan.report["cells"],
        serde_json::json!(["Acme.ErrorsCell_MU"])
    );
    plan.apply(&solution, &guard).unwrap();
    let content = std::fs::read_to_string(dir.join("mashup/content.json")).unwrap();
    let doc = mashup::edit::parse(&content).unwrap();
    assert_eq!(
        doc.at(&doc.find_exact(&dial.id).unwrap().path)["Properties"]["Label"],
        ""
    );
    assert!(super::build::plan(&solution, "Acme.Errors_MU", None, None)
        .unwrap()
        .report["files"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn benchmark_p_golden() {
    benchmark("p");
}
#[test]
fn benchmark_s_golden() {
    benchmark("s");
}

fn widget_entry(built: &super::compile::Artifacts, id: &str) -> Value {
    widget_style(built, id)
}

#[test]
fn card_grid_builds_equal_rows_and_cells_and_pads_the_last_row() {
    let (_dir, solution) = element_solution();
    let cards = (1..=5)
        .map(|i| format!("{{card: {{id: c{i}, title: Card {i}, align: center}}}}"))
        .collect::<Vec<_>>()
        .join(", ");
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            &format!("card_grid: {{id: gallery, size: fill, per_row: 3, children: [{cards}]}}"),
            "",
        ),
    )
    .unwrap();
    let doc = doc_of(&built);
    let gallery = doc.find_exact(&built.ids["gallery"]).unwrap();
    let rows = doc.at(&gallery.path)["Widgets"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 2);
    for row in &rows {
        let p = &row["Properties"];
        assert_eq!(p["flex-direction"], "row");
        assert_eq!(p["flex-size"], "default");
        assert_eq!(p["flex-grow"], 1);
        assert_eq!(p["flex-basis"], "0px");
        let cells = row["Widgets"].as_array().unwrap();
        assert_eq!(cells.len(), 3);
        for cell in cells {
            let p = &cell["Properties"];
            assert_eq!(p["flex-grow"], 1);
            assert_eq!(p["flex-shrink"], 1);
            assert_eq!(p["flex-basis"], "0px");
            assert_eq!(p["Margin"], "10");
        }
    }
    // The sixth cell is an empty grower, so the fifth card is as wide as the first.
    assert_eq!(
        rows[1]["Widgets"][2]["Widgets"].as_array().unwrap().len(),
        0
    );
    // A card is a kit section: a fixed 40px title row over a LastContainer body, content centred.
    let card = &rows[0]["Widgets"][0];
    let [bar, body] = card["Widgets"].as_array().unwrap().as_slice() else {
        panic!("section has a title row and a body row: {card}");
    };
    assert_eq!(bar["Properties"]["flex-min-height"], "40px");
    assert_eq!(bar["Properties"]["flex-max-height"], "40px");
    assert_eq!(bar["Widgets"][0]["Properties"]["LabelType"], "sub-header");
    assert_eq!(body["Properties"]["LastContainer"], true);
    assert_eq!(body["Properties"]["justify-content"], "center");
    assert_eq!(body["Properties"]["align-items"], "center");
    let frame = &widget_style(&built, "c1")["parts"]["$"]["$"];
    assert_eq!(frame["border-style"], "solid");
}

#[test]
fn a_card_with_align_matches_the_section_recipe_on_an_equivalent_primitive_column() {
    use authoring::{session::Session, AddRequest, Recipe};
    let (_dir, solution) = element_solution();
    let text = error_page("card: {id: target, title: Compare, align: center}", "");
    let built = compile(&solution, "Acme", &text).unwrap();
    let mut primitive =
        Session::new("Acme.Errors_MU", "Acme", "blank", "PTC Convergence Theme").unwrap();
    let target = primitive
        .add(
            &solution,
            &AddRequest {
                mashup: "Acme.Errors_MU".into(),
                widget: "flexcontainer".into(),
                inputs: vec![],
                placement: Some(mashup::edit::Placement::In("center".into())),
                label: None,
                properties: vec![("flex-direction".to_string(), "column".to_string())],
            },
        )
        .unwrap();
    primitive
        .recipe(
            &solution,
            Recipe::Section {
                container: target.clone(),
                title: "Compare".into(),
                center: true,
            },
        )
        .unwrap();
    let section = |document: &mashup::edit::MashupDocument, id: &str| {
        let path = document.find_exact(id).unwrap().path;
        let mut node = document.at(&path).clone();
        fn strip(value: &mut Value) {
            match value {
                Value::Object(map) => {
                    for key in ["Id", "DisplayName", "iconClass", "scrolling"] {
                        map.remove(key);
                    }
                    map.values_mut().for_each(strip);
                }
                Value::Array(items) => items.iter_mut().for_each(strip),
                _ => {}
            }
        }
        strip(&mut node);
        node["Widgets"].take()
    };
    assert_eq!(
        section(&doc_of(&built), &built.ids["target"]),
        section(&primitive.document, &target)
    );
}

#[test]
fn text_field_variants_match_the_input_variant_recipe() {
    use authoring::{session::Session, AddRequest, Recipe};
    let (_dir, solution) = element_solution();
    for variant in [
        "rounded",
        "focus-background",
        "focus-glow",
        "focus-underline",
    ] {
        let built = compile(
            &solution,
            "Acme",
            &error_page(&format!("text_field: {{id: t, variant: {variant}}}"), ""),
        )
        .unwrap();
        let mut primitive =
            Session::new("Acme.Errors_MU", "Acme", "blank", "PTC Convergence Theme").unwrap();
        let field = primitive
            .add(
                &solution,
                &AddRequest {
                    mashup: "Acme.Errors_MU".into(),
                    widget: "ptcstextfield".into(),
                    inputs: vec![],
                    placement: Some(mashup::edit::Placement::In("center".into())),
                    label: None,
                    properties: vec![],
                },
            )
            .unwrap();
        primitive
            .recipe(
                &solution,
                Recipe::InputVariant {
                    widget: field.clone(),
                    variant: variant.into(),
                },
            )
            .unwrap();
        assert_eq!(
            widget_entry(&built, "t"),
            primitive.document.root_properties()["StyleProperties"]["data"][&field],
            "{variant}"
        );
    }
    let error = compile(
        &solution,
        "Acme",
        &error_page("text_field: {id: t, variant: glossy}", ""),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("rounded, focus-background"),
        "{error}"
    );
}

#[test]
fn headings_labels_and_text_take_sizes_highlights_and_style_maps() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "column: {id: col, children: [{page_header: {id: head, title: Inputs, color: primary}}, {heading: {id: big, text: Big, size: display, highlight: lime}}, {heading: {id: small, text: Small, size: sm}}, {text: {id: lead, text: Lead}}, {label: {id: tag, text: Tag, style: {background: ink, color: lime, font: mono, border: pink}}}]}",
            "",
        ),
    )
    .unwrap();
    assert_eq!(element_props(&built, "big")["LabelType"], "large-header");
    assert_eq!(element_props(&built, "small")["LabelType"], "title");
    assert_eq!(element_props(&built, "lead")["LabelType"], "body");
    let big = widget_style(&built, "big");
    assert_eq!(big["parts"]["$"]["$"]["background"], "#CCFF00");
    assert_eq!(big["parts"]["label"]["$"]["font-size"], "72px");
    let tag = widget_style(&built, "tag");
    assert_eq!(tag["parts"]["$"]["$"]["background"], "#0B0B0B");
    assert_eq!(tag["parts"]["$"]["$"]["border-color"], "#FF2E93");
    assert_eq!(tag["parts"]["$"]["$"]["border-width"], "2px");
    assert_eq!(tag["parts"]["label"]["$"]["color"], "#CCFF00");
    assert!(tag["parts"]["label"]["$"]["font-family"]
        .as_str()
        .unwrap()
        .contains("monospace"));
    assert_eq!(
        widget_style(&built, "head--title")["parts"]["label"]["$"]["color"],
        "#0088CE"
    );
}

#[test]
fn a_block_is_a_coloured_card_with_a_caption_and_a_bound_value() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "block: {id: kpi, color: pink, caption: OEE, value: {bind: rows.Count}}",
            "",
        ),
    )
    .unwrap();
    assert_eq!(
        widget_style(&built, "kpi")["parts"]["$"]["$"]["background"],
        "#FF2E93"
    );
    let doc = doc_of(&built);
    let card = doc.find_exact(&built.ids["kpi"]).unwrap();
    let labels: Vec<_> = doc
        .widgets()
        .into_iter()
        .filter(|w| w.path.starts_with(&card.path) && w.type_id == "ptcslabel")
        .collect();
    assert_eq!(doc.at(&labels[0].path)["Properties"]["LabelText"], "OEE");
    assert!(doc.value()["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["TargetId"] == labels[1].id));
}

#[test]
fn new_element_errors_are_located() {
    let (_dir, solution) = element_solution();
    for (body, expect) in [
        (
            "card_grid: {id: g, children: [{card: {id: c, title: A}}]}",
            "needs per_row",
        ),
        (
            "card_grid: {id: g, per_row: 0, children: [{card: {id: c, title: A}}]}",
            "per_row is 1 to 12",
        ),
        ("card_grid: {id: g, per_row: 2}", "needs children"),
        (
            "card_grid: {id: g, per_row: 2, children: [{label: {id: l, text: A}}]}",
            "card_grid cells are",
        ),
        (
            "card: {id: c, title: A, align: middle}",
            "align is start or center",
        ),
        (
            "card: {id: c, title: A, align: center, value: 3}",
            "holds children",
        ),
        (
            "block: {id: b, caption: A, value: 1}",
            "block needs color, caption and value",
        ),
        (
            "block: {id: b, color: teal, caption: A, value: 1}",
            "unknown colour",
        ),
        (
            "heading: {id: h, text: A, highlight: teal}",
            "unknown colour",
        ),
        (
            "page_header: {id: h, title: A, color: teal}",
            "unknown colour",
        ),
        (
            "label: {id: h, text: A, style: {texture: ink}}",
            "style texture is not supported",
        ),
        (
            "chart: {id: c, kind: bar, data: rows, x: Name, style: {color: ink}}",
            "belongs to heading, label and text",
        ),
        (
            "grid: {id: g, data: rows, size: lg}",
            "size sm, md, lg or display",
        ),
        (
            "grid: {id: g, data: rows, style: {background: ink}}",
            "style map",
        ),
        (
            "button: {id: b, label: A, highlight: lime}",
            "highlight does not belong",
        ),
        (
            "heading: {id: h, text: A, align: center}",
            "align does not belong",
        ),
        ("text_field: {id: t, caption: A}", "caption does not belong"),
    ] {
        let error = compile(&solution, "Acme", &error_page(body, "")).unwrap_err();
        assert!(error.line >= 8, "{body}: {error}");
        assert!(error.to_string().contains(expect), "{body}: {error}");
    }
}

#[test]
fn style_lists_and_headings_without_a_size_keep_their_result() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &error_page(
            "heading: {id: h, text: Plain, style: [\"label:color=#112233\"]}",
            "",
        ),
    )
    .unwrap();
    assert_eq!(element_props(&built, "h")["LabelType"], "header");
    assert_eq!(
        widget_style(&built, "h")["parts"]["label"]["$"]["color"],
        "#112233"
    );
}

#[test]
fn components_expand_through_existing_elements_and_validate_props() {
    let (_dir, solution) = element_solution();
    for (component, element, props) in [
        ("page_header", "page_header", ", title: Alerts"),
        ("section_card", "card", ", title: Inputs, align: start"),
        ("side_panel", "side_panel", ", title: Compare, tab: Filters"),
        ("toolbar_band", "toolbar", ", children: []"),
        (
            "filter_panel",
            "filter_panel",
            ", fields: [{text_field: {id: query}}], footer: [apply, reset], title: Filter",
        ),
        (
            "trend_tile",
            "trend_tile",
            ", label: Count, value: {bind: rows.Count}, trend: flat, unit: '', delta: ''",
        ),
    ] {
        let component_spec = error_page(
            &format!("component: {{use: {component}, id: target{props}}}"),
            "",
        );
        let element_spec = error_page(&format!("{element}: {{id: target{props}}}"), "");
        let a = compile(&solution, "Acme", &component_spec)
            .unwrap_or_else(|e| panic!("{component}: {e}"));
        let b = compile(&solution, "Acme", &element_spec).unwrap();
        assert!(a == b, "component differs from its element: {component}");
    }
    let card = error_page(
        "component: {use: status_card, id: health, title: Health, status: Healthy, detail: Online}",
        "",
    );
    let built = compile(&solution, "Acme", &card).unwrap();
    assert_eq!(
        element_props(&built, "health--status--value")["LabelText"],
        "Healthy"
    );
    assert_eq!(
        element_props(&built, "health--detail")["LabelText"],
        "Online"
    );
    let invalid = error_page("component: {use: trend_tile, id: bad_spark, label: Total, value: {bind: rows.Count}, sparkline: {field: Missing, x: Name}}","");
    let error = compile(&solution, "Acme", &invalid).unwrap_err();
    assert!(error.line > 1, "{error}");
    let nested = error_page("component: {use: section_card, id: outer, title: Outer, children: [{component: {use: section_card, id: inner, title: Inner}}]}","");
    compile(&solution, "Acme", &nested).unwrap();
    for (props, message) in [
        ("use: missing", "unknown component"),
        ("use: page_header", "missing required prop title"),
        (
            "use: page_header, title: Test, typo: x",
            "unknown prop typo",
        ),
        ("use: page_header, title: 3", "must be string"),
    ] {
        let error = compile(
            &solution,
            "Acme",
            &error_page(&format!("component: {{id: target, {props}}}"), ""),
        )
        .unwrap_err();
        assert!(error.message.contains(message), "{error}");
        assert!(error.line > 1, "{error}");
    }
}

#[test]
fn solution_components_nest_override_and_check_data_contracts() {
    let (dir, solution) = element_solution();
    let components = dir.path().join("twaco/components");
    std::fs::create_dir_all(&components).unwrap();
    std::fs::write(
        components.join("header.yaml"),
        r#"name: page_header
source: twaco
props:
  title: {type: string, required: true}
tree:
  heading: {id: '$id', text: '$title'}
"#,
    )
    .unwrap();
    std::fs::write(
        components.join("metric.yaml"),
        r#"name: metric
source: twaco
props:
  data: {type: string, required: true}
contract:
  data: data
  fields: {Count: NUMBER}
tree:
  component: {use: page_header, id: '$id', title: Metric}
"#,
    )
    .unwrap();
    let text = error_page("component: {use: metric, id: metric, data: rows}", "");
    let built = compile(&solution, "Acme", &text).unwrap();
    assert_eq!(element_props(&built, "metric")["LabelText"], "Metric");
    let def = super::components::inspect(&solution, Some("page_header")).unwrap();
    assert_eq!(def["source"], "twaco");
    for (from, to, message) in [
        ("Count: NUMBER", "Missing: NUMBER", "Missing"),
        ("Count: NUMBER", "Count: STRING", "expected STRING"),
    ] {
        let path = components.join("metric.yaml");
        let original = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, original.replace(from, to)).unwrap();
        let e = compile(&solution, "Acme", &text).unwrap_err();
        assert!(e.message.contains(message), "{e}");
        assert!(e.line > 1, "{e}");
        std::fs::write(path, original).unwrap();
    }
    std::fs::write(
        components.join("cycle.yaml"),
        "name: loop\nsource: twaco\ntree: {component: {use: loop, id: '$id'}}\n",
    )
    .unwrap();
    let e = compile(
        &solution,
        "Acme",
        &error_page("component: {use: loop, id: cycle}", ""),
    )
    .unwrap_err();
    assert!(e.message.contains("cycle"), "{e}");
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &path);
        } else {
            std::fs::copy(entry.path(), path).unwrap();
        }
    }
}

fn fixtures() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mashup/pages")
}

/// The offline benchmark solution, in a scratch directory.
fn bench_workspace() -> (tempfile::TempDir, Solution) {
    let dir = tempfile::tempdir().unwrap();
    copy_tree(&fixtures().join("bench_solution"), dir.path());
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution)
}

/// What a rebuilt page keeps although the compiler adds layout wrappers: the widgets that are
/// not containers in document order with the non-container widgets around them, the services,
/// and the bindings and events by widget type.
#[derive(Debug, PartialEq)]
struct Shape {
    widgets: Vec<(String, Vec<String>)>,
    services: Vec<String>,
    bindings: Vec<String>,
    events: Vec<String>,
}

fn shape(content: &str) -> Shape {
    let value: Value = serde_json::from_str(content).unwrap();
    let mut types = BTreeMap::new();
    let mut widgets = vec![];
    fn walk(
        widget: &Value,
        around: &mut Vec<String>,
        types: &mut BTreeMap<String, String>,
        widgets: &mut Vec<(String, Vec<String>)>,
    ) {
        for child in widget["Widgets"].as_array().into_iter().flatten() {
            let kind = child["Properties"]["Type"].as_str().unwrap().to_string();
            types.insert(
                child["Properties"]["Id"].as_str().unwrap().to_string(),
                kind.clone(),
            );
            let container = kind == "flexcontainer";
            if !container {
                widgets.push((kind.clone(), around.clone()));
                around.push(kind);
            }
            walk(child, around, types, widgets);
            if !container {
                around.pop();
            }
        }
    }
    walk(&value["UI"], &mut vec![], &mut types, &mut widgets);
    let name = |id: &Value| {
        let id = id.as_str().unwrap_or("");
        types.get(id).cloned().unwrap_or_else(|| id.to_string())
    };
    let mut services = vec![];
    for entry in value["Data"].as_object().unwrap().values() {
        if entry["EntityType"] == "Things" {
            for service in entry["Services"].as_array().unwrap() {
                services.push(format!(
                    "{}.{}",
                    entry["EntityName"].as_str().unwrap(),
                    service["Target"].as_str().unwrap()
                ));
            }
        }
    }
    let mut bindings = vec![];
    for binding in value["DataBindings"].as_array().unwrap() {
        for map in binding["PropertyMaps"].as_array().unwrap() {
            bindings.push(format!(
                "{}:{}.{} -> {}:{}.{}",
                binding["SourceArea"].as_str().unwrap(),
                name(&binding["SourceId"]),
                map["SourceProperty"].as_str().unwrap(),
                binding["TargetArea"].as_str().unwrap(),
                name(&binding["TargetId"]),
                map["TargetProperty"].as_str().unwrap(),
            ));
        }
    }
    let mut events = vec![];
    for event in value["Events"].as_array().unwrap() {
        events.push(format!(
            "{}:{}.{} -> {}:{}.{}",
            event["EventTriggerArea"].as_str().unwrap(),
            name(&event["EventTriggerId"]),
            event["EventTriggerEvent"].as_str().unwrap(),
            event["EventHandlerArea"].as_str().unwrap(),
            name(&event["EventHandlerId"]),
            event["EventHandlerService"].as_str().unwrap(),
        ));
    }
    for list in [&mut services, &mut bindings, &mut events] {
        list.sort();
    }
    Shape {
        widgets,
        services,
        bindings,
        events,
    }
}

#[test]
fn kit_structure_fixtures_derive_readable_specs_that_rebuild_the_same_shape() {
    let (dir, solution) = bench_workspace();
    let cell = compile(
        &solution,
        "Acme.Bench",
        "version: 1\nrules: v1\npage:\n  name: Acme.Kit.Cell_MU\n  template: kpi-tile\n  parameters: {cardTitle: STRING, cardKpiDial: NUMBER}\n  slots:\n    title: [{label: {id: title, props: {LabelText: {bind: param.cardTitle}}}}]\n    value: [{kpi_dial: {id: dial, value: {bind: param.cardKpiDial}}}]\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("Mashups")).unwrap();
    std::fs::write(
        dir.path().join("Mashups/Acme.Kit.Cell_MU.xml"),
        &cell.entity,
    )
    .unwrap();
    let folder = fixtures().join("kit_structures");
    let mut names: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(names.len(), 5);
    for file in names {
        let content = std::fs::read_to_string(folder.join(&file)).unwrap();
        let stem = file.strip_suffix(".content.json").unwrap();
        let name = format!("Acme.Kit.{stem}_MU");
        let derived = super::extract::derive_content(&name, &content).unwrap();
        assert!(
            !derived.spec.contains("imported") && !derived.spec.trim_start().starts_with('{'),
            "{stem} is YAML elements, not an envelope"
        );
        let rebuilt = compile(&solution, "Acme.Bench", &derived.spec)
            .unwrap_or_else(|e| panic!("{stem}: {e}\n{}", derived.spec));
        assert_eq!(
            shape(&content),
            shape(&rebuilt.content),
            "{stem}\n{}",
            derived.spec
        );
        assert!(
            derived.spec.len() < 4000,
            "{stem} spec is {} bytes",
            derived.spec.len()
        );
        println!(
            "{stem}: {} widgets, {} elements/components, {} raw {:?}, {} bytes",
            derived.widgets,
            derived.elements,
            derived.raw,
            derived.raw_types,
            derived.spec.len()
        );
        let expected: &[&str] = match stem {
            "filter_column" => &[
                "filter_panel:",
                "title: Filter",
                "page_header:",
                "toolbar:",
                "grid:",
                "dropdown:",
                "date_range:",
                "service: Acme.Cards.SearchAssets",
            ],
            "side_column" => &[
                "side_panel:",
                "list:",
                "collection:",
                "mashup: Acme.Kit.Cell_MU",
            ],
            "section_gallery" => &["card:", "align: center", "text_field:"],
            "admin_shell" => &["toolbar:", "spacer:", "chart:", "kpi_row:", "weight: 2"],
            "tabs_page" => &["tabs:", "type: expression2", "bind:"],
            _ => panic!("uncovered fixture {stem}"),
        };
        for needle in expected {
            assert!(
                derived.spec.contains(needle),
                "{stem} lacks {needle}\n{}",
                derived.spec
            );
        }
        let raw: &[(&str, usize)] = match stem {
            "tabs_page" => &[("expression2", 1)],
            "filter_column" => &[("ptcspagination", 1)],
            "section_gallery" => &[("ptcsdatepicker", 1)],
            _ => &[],
        };
        let raw: BTreeMap<String, usize> = raw.iter().map(|(k, n)| (k.to_string(), *n)).collect();
        assert_eq!(derived.raw_types, raw, "{stem}");
    }
}

#[test]
fn all_acme_bench_sources_extract_and_rebuild_exactly() {
    let folder = fixtures();
    let (dir, solution) = bench_workspace();
    let mut count = 0;
    let mut rebuilt_count = 0;
    for entry in std::fs::read_dir(&folder).unwrap() {
        let path = entry.unwrap().path();
        if !path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("bench_")
            || !path.join("page.yaml").exists()
        {
            continue;
        }
        let text = std::fs::read_to_string(path.join("page.yaml")).unwrap();
        let spec = schema::parse(&text).unwrap();
        let name = &spec.page.value.name.value;
        let built = compile(&solution, "Acme.Bench", &text).unwrap();
        let xml = dir.path().join("Mashups").join(format!("{name}.xml"));
        std::fs::create_dir_all(xml.parent().unwrap()).unwrap();
        std::fs::write(&xml, &built.entity).unwrap();
        let source = solution.src_root().join(name).join("page.yaml");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, &text).unwrap();
        let derived = super::extract::extract(&solution, name).unwrap();
        assert!(derived.exact, "{name}");
        assert_eq!(derived.spec, text, "{name}");
        let baseline_path = source.with_file_name(".page-baseline.json");
        std::fs::write(
            &baseline_path,
            super::baseline::Baseline::generated(&built, name, "Acme.Bench").bytes(),
        )
        .unwrap();
        let derived = super::extract::extract(&solution, name).unwrap();
        assert!(derived.exact && derived.hand_edited.is_empty(), "{name}");
        assert_eq!(derived.spec, text, "{name}");
        assert_eq!(derived.raw_types.is_empty(), derived.raw == 0);
        let generic = super::extract::derive_content(name, &built.content).unwrap();
        schema::parse_solution(&generic.spec, &solution).unwrap_or_else(|e| {
            panic!(
                "{name}: {e}
{}",
                generic.spec
            )
        });
        assert!(
            generic.spec.len() * 3 < built.content.len() * 2,
            "{name}: spec {} bytes, content {}",
            generic.spec.len(),
            built.content.len()
        );
        // Building a derived spec takes a while for the large pages; the small ones prove the shape.
        if generic.widgets <= 40 {
            for cell in &built.cells {
                std::fs::write(
                    dir.path()
                        .join("Mashups")
                        .join(format!("{}.xml", cell.name)),
                    &cell.artifacts.entity,
                )
                .unwrap();
            }
            let rebuilt = compile(&solution, "Acme.Bench", &generic.spec).unwrap_or_else(|e| {
                panic!(
                    "{name}: {e}
{}",
                    generic.spec
                )
            });
            assert_eq!(
                shape(&built.content),
                shape(&rebuilt.content),
                "{name}
{}",
                generic.spec
            );
            rebuilt_count += 1;
        }
        println!(
            "{name}: {} widgets, {} elements/components, {} raw {:?}, {} bytes",
            generic.widgets,
            generic.elements,
            generic.raw,
            generic.raw_types,
            generic.spec.len()
        );
        for cell in &built.cells {
            let _ = std::fs::remove_file(
                dir.path()
                    .join("Mashups")
                    .join(format!("{}.xml", cell.name)),
            );
        }
        std::fs::remove_file(&xml).unwrap();
        std::fs::remove_file(&baseline_path).unwrap();
        count += 1;
    }
    assert_eq!(count, 14);
    assert_eq!(rebuilt_count, 6);
}

#[test]
fn a_hand_edited_page_returns_its_spec_and_names_the_edited_widgets() {
    let (dir, solution) = bench_workspace();
    let text = std::fs::read_to_string(fixtures().join("bench_h/page.yaml")).unwrap();
    let name = "Acme.BenchSpecH_MU";
    let built = compile(&solution, "Acme.Bench", &text).unwrap();
    std::fs::create_dir_all(dir.path().join("Mashups")).unwrap();
    std::fs::write(
        dir.path().join(format!("Mashups/{name}.xml")),
        &built.entity,
    )
    .unwrap();
    let source = solution.src_root().join(name);
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("page.yaml"), &text).unwrap();
    std::fs::write(
        source.join(".page-baseline.json"),
        super::baseline::Baseline::generated(&built, name, "Acme.Bench").bytes(),
    )
    .unwrap();
    let id = &built.ids["search"];
    let mut value: Value = serde_json::from_str(&built.content).unwrap();
    fn edit(widget: &mut Value, id: &str) -> bool {
        if widget["Properties"]["Id"] == id {
            widget["Properties"]["Margin"] = serde_json::json!("9");
            return true;
        }
        widget["Widgets"]
            .as_array_mut()
            .is_some_and(|kids| kids.iter_mut().any(|k| edit(k, id)))
    }
    assert!(edit(&mut value["UI"], id));
    let edited = mashup::to_sidecar(&value);
    let assets = mashup::Assets {
        content: edited,
        css: built.css.clone(),
    };
    let entity = mashup::sync(&built.entity, &assets).unwrap().0;
    std::fs::write(dir.path().join(format!("Mashups/{name}.xml")), entity).unwrap();
    let found = super::extract::extract(&solution, name).unwrap();
    assert!(!found.exact);
    assert_eq!(found.spec, text);
    assert_eq!(found.hand_edited, vec![format!("search ({id})")]);
    assert!(
        found.notes[0].contains("edited by hand"),
        "{:?}",
        found.notes
    );
}

#[test]
fn component_plan_refuses_changed_added_and_removed_definitions() {
    let (dir, solution) = solution();
    let spec = dir.path().join("input.yaml");
    let text = "version: 1\nrules: v1\npage: {name: Acme.Component_MU, template: blank, slots: {center: [{component: {use: page_header, id: h, title: Test}}]}}\n";
    std::fs::write(&spec, text).unwrap();
    let folder = dir.path().join("twaco/components");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("extra.yaml");
    let definition = "name: extra\nsource: twaco\ntree: {label: {id: '$id'}}\n";
    for action in ["add", "change", "remove"] {
        if action == "add" {
            if file.exists() {
                std::fs::remove_file(&file).unwrap();
            }
        } else {
            std::fs::write(&file, definition).unwrap();
        }
        let mut plan =
            super::build::plan(&solution, "Acme.Component_MU", Some("Acme"), Some(&spec)).unwrap();
        match action {
            "remove" => std::fs::remove_file(&file).unwrap(),
            "change" => std::fs::write(&file, definition.replace("label:", "heading:")).unwrap(),
            _ => std::fs::write(&file, definition).unwrap(),
        }
        let lock = lock::acquire(dir.path(), "test", &[]).unwrap();
        assert!(plan
            .apply(&solution, &lock)
            .unwrap_err()
            .to_string()
            .contains("twaco/components changed"));
        assert!(!dir
            .path()
            .join("acme/Mashups/Acme.Component_MU.xml")
            .exists());
    }
}

#[test]
fn command_components_bind_existing_calls_through_the_spec_compiler() {
    let (_dir, solution) = element_solution();
    let mut session = authoring::session::Session::new(
        "Acme.Errors_MU",
        "Acme",
        "blank",
        "PTC Convergence Theme",
    )
    .unwrap();
    session
        .source(
            &solution,
            &authoring::SourceAddRequest {
                mashup: "Acme.Errors_MU".into(),
                entity: "Acme.Data".into(),
                service: "GetRows".into(),
                alias: Some("rows".into()),
                dynamic: false,
                on_load: true,
                entity_from: None,
                refresh: None,
                cache: false,
                cache_keys: vec![],
                cache_strategy: None,
                max_result_sets: None,
                param_values: vec![],
            },
        )
        .unwrap();
    session.sync().unwrap();
    std::fs::write(
        solution.root.join("acme/Mashups/Acme.Errors_MU.xml"),
        &session.entity,
    )
    .unwrap();
    let plan = authoring::plan_recipe(
        &solution,
        &authoring::RecipeRequest {
            mashup: "Acme.Errors_MU".into(),
            recipe: authoring::Recipe::Component {
                container: "center".into(),
                name: "trend_tile".into(),
                settings: vec![
                    "id=tile".into(),
                    "label=Total".into(),
                    "value={\"bind\":\"rows.Count\"}".into(),
                ],
            },
        },
    )
    .unwrap();
    let content = mashup::extract(&plan.edit.entity_after).unwrap().content;
    let text = error_page(
        "component: {use: trend_tile, id: tile, label: Total, value: {bind: rows.Count}}",
        "",
    );
    let built = compile(&solution, "Acme", &text).unwrap();
    assert!(
        canonical(&content) == canonical(&built.content),
        "command component differs from the spec component"
    );
}

#[test]
fn spec_output_locks_before_reading_and_writes_nothing_when_held() {
    let (dir, solution) = solution();
    let output = dir.path().join("derived.yaml");
    let guard = lock::acquire_for(&solution, "other writer").unwrap();
    let error = super::extract::export(&solution, "Acme.Missing_MU", Some(&output)).unwrap_err();
    assert!(
        error.to_string().contains("another twaco command"),
        "{error}"
    );
    assert!(!output.exists());
    drop(guard);
}

#[test]
fn structural_command_components_allow_unused_duplicate_service_aliases() {
    let (_dir, solution) = element_solution();
    let mut session = authoring::session::Session::new(
        "Acme.Components_MU",
        "Acme",
        "blank",
        "PTC Convergence Theme",
    )
    .unwrap();
    session
        .source(
            &solution,
            &authoring::SourceAddRequest {
                mashup: "Acme.Components_MU".into(),
                entity: "Acme.Data".into(),
                service: "GetRows".into(),
                alias: Some("rows".into()),
                dynamic: false,
                on_load: true,
                entity_from: None,
                refresh: None,
                cache: false,
                cache_keys: vec![],
                cache_strategy: None,
                max_result_sets: None,
                param_values: vec![],
            },
        )
        .unwrap();
    let sections = crate::core::bindings::data_sections(session.document.value());
    let (section, call) = sections
        .iter()
        .find_map(|(key, section)| {
            section
                .services
                .iter()
                .find(|call| call.target == "GetRows")
                .map(|call| (key.clone(), call.instance.clone()))
        })
        .unwrap();
    let data = session.document.root_mut()["Data"].as_object_mut().unwrap();
    let duplicate = data[&section].clone();
    data.insert("Acme.Other".into(), duplicate);
    assert_eq!(
        crate::core::bindings::data_sections(session.document.value())
            .values()
            .flat_map(|s| &s.services)
            .filter(|c| c.instance == call)
            .count(),
        2
    );
    session.sync().unwrap();
    std::fs::write(
        solution.root.join("acme/Mashups/Acme.Components_MU.xml"),
        &session.entity,
    )
    .unwrap();
    let request = |name: &str, settings: Vec<String>| authoring::RecipeRequest {
        mashup: "Acme.Components_MU".into(),
        recipe: authoring::Recipe::Component {
            container: "center".into(),
            name: name.into(),
            settings,
        },
    };
    authoring::plan_recipe(
        &solution,
        &request("page_header", vec!["id=h".into(), "title=Title".into()]),
    )
    .unwrap();
    let error = authoring::plan_recipe(
        &solution,
        &request(
            "trend_tile",
            vec![
                "id=k".into(),
                format!(
                    "value={}",
                    serde_json::json!({"bind":format!("{call}.Count")})
                ),
            ],
        ),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("data alias {call} names several calls")),
        "{error}"
    );
}

#[test]
fn command_components_generate_collection_cells_in_the_parent_transaction() {
    let (dir, solution) = element_solution();
    let folder = dir.path().join("twaco/components");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("cards.yaml"),
        r#"name: metric_cards
source: twaco
props:
  data: {type: string, required: true}
tree:
  column: {id: '$id', children: [{collection: {id: '${id}--list', data: '$data', per_row: 2, cell: {title: Name, kpi_dial: Count}}}]}
"#,
    )
    .unwrap();
    let themes = solution.root.join("acme/StyleThemes");
    std::fs::create_dir_all(&themes).unwrap();
    std::fs::write(themes.join("Acme.Theme.xml"),r#"<Entities><StyleThemes><StyleTheme name="Acme.Theme" projectName="Acme"/></StyleThemes></Entities>"#).unwrap();
    let mut session =
        authoring::session::Session::new("Acme.Components_MU", "Acme", "blank", "Acme.Theme")
            .unwrap();
    session
        .source(
            &solution,
            &authoring::SourceAddRequest {
                mashup: "Acme.Components_MU".into(),
                entity: "Acme.Data".into(),
                service: "GetRows".into(),
                alias: Some("rows".into()),
                dynamic: false,
                on_load: true,
                entity_from: None,
                refresh: None,
                cache: false,
                cache_keys: vec![],
                cache_strategy: None,
                max_result_sets: None,
                param_values: vec![],
            },
        )
        .unwrap();
    session.sync().unwrap();
    let parent = solution.root.join("acme/Mashups/Acme.Components_MU.xml");
    std::fs::write(&parent, &session.entity).unwrap();
    let plan = authoring::plan_recipe(
        &solution,
        &authoring::RecipeRequest {
            mashup: "Acme.Components_MU".into(),
            recipe: authoring::Recipe::Component {
                container: "center".into(),
                name: "metric_cards".into(),
                settings: vec!["id=cards".into(), "data=rows".into()],
            },
        },
    )
    .unwrap();
    assert_eq!(plan.generated.len(), 1);
    let files: Vec<_> = plan.generated[0].report["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| solution.root.join(p.as_str().unwrap()))
        .collect();
    assert_eq!(files.len(), 5);
    let guard = lock::acquire_for(&solution, "test component").unwrap();
    for fail in 0..6 {
        let tx = authoring::recipe_transaction(&solution, &plan).unwrap();
        let error = tx
            .apply_with(&guard, &mut |step, _| {
                if step == fail {
                    Err(std::io::Error::other("injected component failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(error.changed_nothing(), "{error}");
        assert_eq!(std::fs::read(&parent).unwrap(), session.entity);
        assert!(files.iter().all(|p| !p.exists()));
    }
    authoring::apply_recipe(&solution, &plan, &guard).unwrap();
    assert!(files.iter().all(|p| p.exists()));
    let cell = solution.src_root().join("Acme.ComponentsCell_MU/page.yaml");
    assert!(std::fs::read_to_string(cell).unwrap().contains("Count"));
    let content: Value = serde_json::from_str(
        &std::fs::read_to_string(
            solution
                .src_root()
                .join("Acme.ComponentsCell_MU/mashup/content.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(content["UI"]["Properties"]["StyleTheme"], "Acme.Theme");
    assert_ne!(std::fs::read(&parent).unwrap(), session.entity);
    let cell_path = solution.src_root().join("Acme.ComponentsCell_MU/page.yaml");
    let original_spec = std::fs::read_to_string(&cell_path).unwrap();
    let comment_spec = format!("# keep this comment\n{original_spec}");
    std::fs::write(&cell_path, &comment_spec).unwrap();
    let request = |id: &str| authoring::RecipeRequest {
        mashup: "Acme.Components_MU".into(),
        recipe: authoring::Recipe::Component {
            container: "center".into(),
            name: "metric_cards".into(),
            settings: vec![format!("id={id}"), "data=rows".into()],
        },
    };
    let second = authoring::plan_recipe(&solution, &request("more_cards")).unwrap();
    authoring::apply_recipe(&solution, &second, &guard).unwrap();
    assert_eq!(std::fs::read_to_string(&cell_path).unwrap(), comment_spec);
    let mut edited: Value = serde_json::from_str(&original_spec).unwrap();
    edited["page"]["slots"]["title"][0]["label"]["props"]["LabelText"] =
        serde_json::json!("Handwritten");
    let edited = edited.to_string();
    std::fs::write(&cell_path, &edited).unwrap();
    let before = std::fs::read(&parent).unwrap();
    let error = match authoring::plan_recipe(&solution, &request("third_cards")) {
        Err(error) => error,
        Ok(_) => panic!("a different existing cell spec was accepted"),
    };
    assert!(
        error.to_string().contains("existing cell spec differs"),
        "{error}"
    );
    assert_eq!(std::fs::read(&parent).unwrap(), before);
    assert_eq!(std::fs::read_to_string(&cell_path).unwrap(), edited);
}

#[test]
fn command_components_bind_existing_mashup_parameters() {
    let (_dir, solution) = element_solution();
    let mut session = authoring::session::Session::new(
        "Acme.Parameters_MU",
        "Acme",
        "blank",
        "PTC Convergence Theme",
    )
    .unwrap();
    session.add_parameter("Count:NUMBER:IN").unwrap();
    session.sync().unwrap();
    std::fs::write(
        solution.root.join("acme/Mashups/Acme.Parameters_MU.xml"),
        &session.entity,
    )
    .unwrap();
    let plan = authoring::plan_recipe(
        &solution,
        &authoring::RecipeRequest {
            mashup: "Acme.Parameters_MU".into(),
            recipe: authoring::Recipe::Component {
                container: "center".into(),
                name: "kpi_tile".into(),
                settings: vec![
                    "id=kpi".into(),
                    "label=Count".into(),
                    "value={\"bind\":\"param.Count\"}".into(),
                ],
            },
        },
    )
    .unwrap();
    let parameters = mashup::describe::mashup_parameters(&plan.edit.entity_after).unwrap();
    assert_eq!(parameters.len(), 1);
    assert_eq!(parameters[0].name, "Count");
    let content = mashup::extract(&plan.edit.entity_after).unwrap().content;
    assert!(content.contains("Count"));
}

#[test]
fn tokens_spec_styles_blocks_and_bands_use_native_style_properties() {
    let (_dir, mut solution) = element_solution();
    solution.mashup.tokens = Some("missing-tokens.yaml".into());
    let yaml = error_page("column: {id: c, children: [{block: {id: b, color: pink, caption: OEE, value: 84, size: xl, style: {shadow: hard, radius: none, border: ink}}}, {label: {id: ticker, text: SHIFT B, band: true, style: {font: mono}}}]}", "").replace("rules: v1", "rules: v1\ntokens: brutalist");
    let built = compile(&solution, "Acme", &yaml).unwrap();
    let style = widget_style(&built, "b");
    assert_eq!(
        style,
        serde_json::json!({"widget":"flex-container", "parts":{"$":{"$":{
            "background":"#FF2E93", "border-color":"#0B0B0B", "border-radius":"0px", "border-style":"solid", "border-width":"2px",
            "box-shadow":"4px 4px 0 #0B0B0B", "padding-bottom":"12px", "padding-left":"24px", "padding-right":"24px", "padding-top":"12px"
        }}}})
    );
    assert_eq!(style["parts"]["$"]["$"]["box-shadow"], "4px 4px 0 #0B0B0B");
    assert_eq!(style["parts"]["$"]["$"]["border-radius"], "0px");
    assert_eq!(style["parts"]["$"]["$"]["border-color"], "#0B0B0B");
    assert_eq!(style["parts"]["$"]["$"]["border-style"], "solid");
    assert_eq!(style["parts"]["$"]["$"]["border-width"], "2px");
    let doc = doc_of(&built);
    let card = doc.find_exact(&built.ids["b"]).unwrap();
    let value = doc
        .widgets()
        .into_iter()
        .filter(|w| w.path.starts_with(&card.path) && w.type_id == "ptcslabel")
        .nth(1)
        .unwrap();
    assert_eq!(
        doc.root_properties()["StyleProperties"]["data"][&value.id]["parts"]["label"]["$"]
            ["font-size"],
        "48px"
    );
    assert_eq!(
        widget_style(&built, "ticker")["parts"]["$"]["$"]["width"],
        "100%"
    );
    assert_eq!(
        widget_style(&built, "ticker")["parts"]["$"]["$"]["flex-grow"],
        "1"
    );
    assert_eq!(
        widget_style(&built, "ticker")["parts"]["label"]["$"]["font-family"],
        "IBM Plex Mono, ui-monospace, Consolas, monospace"
    );
    let defaults = compile(
        &solution,
        "Acme",
        &error_page("block: {id: b, color: pink, caption: OEE, value: 84}", "")
            .replace("rules: v1", "rules: v1\ntokens: brutalist"),
    )
    .unwrap();
    assert_eq!(
        widget_style(&defaults, "b")["parts"]["$"]["$"]["box-shadow"],
        "4px 4px 0 #0B0B0B"
    );
}

#[test]
fn tokens_spec_paths_are_relative_and_theme_reminder_is_in_plan() {
    let (dir, solution) = element_solution();
    let folder = dir.path().join("specs");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("tokens.yaml"),
        "version: 1\nname: Acme\nextends: brutalist\ncolors: {pink: '#123456'}\n",
    )
    .unwrap();
    let yaml = error_page("block: {id: b, color: pink, caption: OEE, value: 84}", "")
        .replace("rules: v1", "rules: v1\ntokens: ./tokens.yaml");
    let built = super::compile::compile_at(&solution, "Acme", &yaml, &folder).unwrap();
    assert_eq!(
        widget_style(&built, "b")["parts"]["$"]["$"]["background"],
        "#123456"
    );
    let path = folder.join("page.yaml");
    std::fs::write(&path, &yaml).unwrap();
    let spec = schema::parse_at(&yaml, &solution, &folder).unwrap();
    let mut plan = super::build::plan(
        &solution,
        &spec.page.value.name.value,
        Some("Acme"),
        Some(&path),
    )
    .unwrap();
    assert!(plan.report["cell_notices"]
        .to_string()
        .contains("new theme Acme.Errors_TH --tokens specs/tokens.yaml --apply"));
    plan.apply(&solution, &lock::acquire_for(&solution, "test").unwrap())
        .unwrap();
    let name = &spec.page.value.name.value;
    let stored = std::fs::read_to_string(solution.src_root().join(name).join("page.yaml")).unwrap();
    assert!(
        stored.contains("tokens: \"../../specs/tokens.yaml\""),
        "{stored}"
    );
    let again = super::build::plan(&solution, name, None, None).unwrap();
    assert!(
        again.report["files"].as_array().unwrap().is_empty(),
        "{}",
        again.report
    );
    let extracted = super::extract::extract(&solution, name).unwrap();
    assert!(extracted.spec.contains("tokens:"));
    std::fs::write(
        folder.join("tokens.yaml"),
        "version: 1\nname: Changed\nextends: clean\n",
    )
    .unwrap();
    assert!(again
        .transaction(&solution)
        .err()
        .unwrap()
        .to_string()
        .contains("changed since planning"));
    std::fs::remove_file(solution.src_root().join(name).join(".page-baseline.json")).unwrap();
    let derived = super::extract::extract(&solution, name).unwrap();
    assert!(!derived.exact);
    assert!(derived.spec.contains("tokens:"));
    assert!(derived.spec.contains("#123456"));
}

#[test]
fn tokens_s_fixture_compiles_with_native_theme_and_finished_blocks() {
    let (_dir, solution) = element_solution();
    let fields = [
        "OEE", "Output", "Scrap", "Downtime", "Cutting", "Press", "Assembly", "Packing", "Hour",
        "Time", "Severity", "Machine", "Message",
    ];
    let definitions = fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            format!(
                "<FieldDefinition name=\"{field}\" baseType=\"{}\" ordinal=\"{i}\"/>",
                if i < 4 { "NUMBER" } else { "STRING" }
            )
        })
        .collect::<String>();
    std::fs::write(solution.root.join("acme/DataShapes/Acme.StyleRows_DS.xml"), format!("<Entities><DataShapes><DataShape name=\"Acme.StyleRows_DS\" projectName=\"Acme\"><FieldDefinitions>{definitions}</FieldDefinitions></DataShape></DataShapes></Entities>")).unwrap();
    let services = ["GetKPIs", "GetMachines", "GetHourlyOutput", "GetAlerts"].iter().map(|service| format!("<ServiceDefinition name=\"{service}\"><ResultType baseType=\"INFOTABLE\" aspect.dataShape=\"Acme.StyleRows_DS\"/></ServiceDefinition>")).collect::<String>();
    std::fs::write(solution.root.join("acme/Things/Acme.StyleData.xml"), format!("<Entities><Things><Thing name=\"Acme.StyleData\" projectName=\"Acme\"><ThingShape><ServiceDefinitions>{services}</ServiceDefinitions></ThingShape></Thing></Things></Entities>")).unwrap();
    let request = crate::core::entity::new::NewRequest {
        tokens: Some("brutalist".into()),
        ..crate::core::entity::new::NewRequest::blank(
            crate::core::entity::new::Kind::Theme,
            "Acme.StyleS_TH",
        )
    };
    let plan = crate::core::entity::new::plan(&solution, &request).unwrap();
    crate::core::entity::new::apply(
        &solution,
        &plan,
        &lock::acquire_for(&solution, "test").unwrap(),
    )
    .unwrap();
    let yaml = include_str!("../../../../tests/fixtures/mashup/page/S.page.yaml");
    let built = compile(&solution, "Acme", yaml).unwrap();
    for id in ["k1", "k2", "k3", "k4"] {
        assert_eq!(
            widget_style(&built, id)["parts"]["$"]["$"]["box-shadow"],
            "4px 4px 0 #0B0B0B"
        );
        assert_eq!(
            widget_style(&built, id)["parts"]["$"]["$"]["border-width"],
            "2px"
        );
    }
    assert_eq!(
        widget_style(&built, "ticker")["parts"]["$"]["$"]["width"],
        "100%"
    );
    let doc = doc_of(&built);
    for id in ["k1", "k2", "k3", "k4"] {
        let widget = doc.find_exact(&built.ids[id]).unwrap();
        let props = &doc.at(&widget.path)["Properties"];
        assert_eq!(props["flex-size"], "range");
        assert_eq!(props["flex-min-height"], "160px");
        assert_eq!(props["flex-max-height"], "");
        assert_eq!(props["flex-basis"], "0px");
        assert_eq!(props["flex-grow"], 1);
    }
    for id in ["m1", "m2", "m3", "m4"] {
        assert_eq!(
            widget_style(&built, id),
            serde_json::json!({
                "widget": "flex-container", "parts": {"$": {"$": {
                    "background": "#F3F0E6", "border-color": "#0B0B0B", "border-radius": "initial",
                    "border-style": "solid", "border-width": "2px", "padding-bottom": "12px",
                    "padding-left": "24px", "padding-right": "24px", "padding-top": "12px"
                }}}
            })
        );
    }
    assert_eq!(
        widget_style(&built, "topbar"),
        serde_json::json!({
            "widget": "flex-container", "parts": {"$": {"$": {
                "background": "#F3F0E6", "border-color": "#0B0B0B", "border-radius": "initial",
                "border-style": "solid", "border-width": "0 0 2px 0", "padding-bottom": "",
                "padding-left": "", "padding-right": "", "padding-top": ""
            }}}
        })
    );
    assert_eq!(
        widget_style(&built, "ticker--band"),
        serde_json::json!({
            "widget": "flex-container", "parts": {"$": {"$": {
                "background": "#CCFF00", "border-style": "solid", "border-width": "2px", "border-color": "#0B0B0B"
            }}}
        })
    );
    let ticker = doc.find_exact(&built.ids["ticker"]).unwrap();
    let band = doc.find_exact(&built.ids["ticker--band"]).unwrap();
    assert_eq!(&ticker.path[..ticker.path.len() - 1], band.path);
    assert_eq!(doc.at(&band.path)["Properties"]["flex-direction"], "row");
    assert_eq!(doc.at(&band.path)["Properties"]["align-items"], "stretch");
    assert_eq!(doc.at(&band.path)["Properties"]["flex-max-height"], "");
    assert_eq!(
        doc.at(&ticker.path)["Properties"]["LabelText"],
        "NO UNPLANNED STOPS  *  SCRAP UNDER 2%  *  SHIFT B ON TARGET"
    );
    assert_eq!(
        widget_style(&built, "h1")["parts"]["label"]["$"]["font-family"],
        "Archivo Black, Archivo, Arial Black, sans-serif"
    );
}

#[test]
fn tokens_reach_sections_panels_surfaces_navigation_and_focus() {
    let (_dir, mut solution) = element_solution();
    solution.mashup.tokens = Some("brutalist".into());
    let yaml = error_page("column: {id: c, children: [
            {card: {id: section, title: Details, align: start, children: [{label: {id: caption, text: Acme}}]}},
            {panel: {id: panel, title: Metrics, children: [{label: {id: metric, text: Acme}}]}},
            {widget: {id: surface, type: flexcontainer, surface: true}},
            {nav_menu: {id: nav, groups: [{title: Acme, items: [Overview]}]}},
            {text_field: {id: input, variant: focus-background}},
            {text_field: {id: glow, variant: focus-glow}},
            {label: {id: ticker, band: true, props: {LabelText: {bind: input.Text}}, style: {background: lime, border: ink}}}
          ]}", "");
    let built = compile(&solution, "Acme", &yaml).unwrap();
    let section = widget_style(&built, "section");
    assert_eq!(section["parts"]["$"]["$"]["border-color"], "#0B0B0B");
    assert_eq!(section["parts"]["$"]["$"]["border-width"], "2px");
    let panel = widget_style(&built, "panel");
    assert_eq!(panel["parts"]["$"]["$"]["background"], "#F3F0E6");
    assert_eq!(panel["parts"]["$"]["$"]["border-color"], "#0B0B0B");
    assert_eq!(panel["parts"]["$"]["$"]["border-width"], "2px");
    assert_eq!(
        widget_style(&built, "surface")["parts"]["$"]["$"]["background"],
        "#F3F0E6"
    );
    assert_eq!(
        widget_style(&built, "nav--group-1")["parts"]["label"]["$"]["color"],
        "#0B0B0B"
    );
    assert_eq!(
        widget_style(&built, "nav--item-1-1")["parts"]["$"]["$"]["background"],
        "#CCFF00"
    );
    assert_eq!(
        widget_style(&built, "input")["parts"]["text-box"]["focus"]["background"],
        "#F3F0E6"
    );
    assert_eq!(
        widget_style(&built, "glow")["parts"]["text-box"]["focus"]["border-color"],
        "#CCFF00"
    );
    assert_eq!(
        widget_style(&built, "glow")["parts"]["text-box"]["focus"]["box-shadow"],
        "0 0 6px 1px #CCFF00"
    );
    let content: Value = serde_json::from_str(&built.content).unwrap();
    assert!(content["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|binding| {
            binding["SourceId"] == built.ids["input"] && binding["TargetId"] == built.ids["ticker"]
        }));
}

#[test]
fn sized_blocks_keep_an_uncapped_height_after_final_layout() {
    let (_dir, solution) = element_solution();
    let children = ["sm", "md", "lg", "xl", "display"].into_iter().map(|size| {
        format!("{{block: {{id: b_{size}, color: lime, caption: Acme, value: '84%', size: {size}}}}}")
    }).collect::<Vec<_>>().join(", ");
    let built = compile(
        &solution,
        "Acme",
        &error_page(&format!("column: {{id: c, children: [{children}]}}"), ""),
    )
    .unwrap();
    let mut problems = vec![];
    mashup::validate::check_content(&built.content, &mut problems);
    for size in ["sm", "md", "lg", "xl", "display"] {
        let id = &built.ids[&format!("b_{size}")];
        let props = element_props(&built, &format!("b_{size}"));
        assert_eq!(props["flex-size"], "range");
        assert_eq!(props["flex-max-height"], "");
        assert!(
            props["flex-min-height"]
                .as_str()
                .unwrap()
                .trim_end_matches("px")
                .parse::<u32>()
                .unwrap()
                >= 100
        );
        assert!(
            !problems.iter().any(|p| p.rule == "wrapped-content-clipping"
                && p.message.starts_with(&format!("{id}:"))),
            "{problems:?}"
        );
    }
}

/// The page spec examples in README.md and the page-specs guide topic compile as written.
#[test]
fn documented_page_spec_examples_compile() {
    let (dir, _) = solution();
    let folder = dir.path().join("acme");
    std::fs::write(folder.join("Things/Acme.Manager.xml"), r#"<Entities><Things><Thing name="Acme.Manager" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders_DS"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#).unwrap();
    std::fs::create_dir_all(folder.join("DataShapes")).unwrap();
    std::fs::write(folder.join("DataShapes/Acme.Orders_DS.xml"), r#"<Entities><DataShapes><DataShape name="Acme.Orders_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="Name" baseType="STRING" ordinal="0"/></FieldDefinitions></DataShape></DataShapes></Entities>"#).unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for doc in ["README.md", "knowledge/page-specs.md"] {
        let text = std::fs::read_to_string(root.join(doc)).unwrap();
        let spec = text
            .split("```yaml\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .unwrap_or_else(|| panic!("{doc} has no yaml example"));
        compile(&solution, "Acme", spec).unwrap_or_else(|e| panic!("{doc}: {e}"));
    }
}

/// A generated collection cell keeps the page's token selection, with a file path rebased onto
/// the folder the cell spec is stored in, so a broken twaco.toml default never takes over.
#[test]
fn generated_cells_inherit_the_page_tokens() {
    let (dir, _) = element_solution();
    std::fs::write(
        dir.path().join("twaco.toml"),
        "[mashup]\ntokens = 'missing.yaml'\n\n[[project]]\nname = 'Acme'\nroot = 'acme'\n",
    )
    .unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    let folder = dir.path().join("specs");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("tokens.yaml"),
        "version: 1\nname: Acme\nextends: brutalist\n",
    )
    .unwrap();
    let cell = "collection: {id: c, size: fill, data: rows, cell: {title: Name}}";
    for (tokens, expected) in [
        ("brutalist", "brutalist".to_string()),
        ("./tokens.yaml", "../../specs/tokens.yaml".to_string()),
    ] {
        let yaml =
            error_page(cell, "").replace("rules: v1", &format!("rules: v1\ntokens: {tokens}"));
        let built = super::compile::compile_at(&solution, "Acme", &yaml, &folder).unwrap();
        let spec: Value = serde_saphyr::from_str(&built.cells[0].spec).unwrap();
        assert_eq!(spec["tokens"], expected.as_str());
        let stored = solution.src_root().join(&built.cells[0].name);
        schema::parse_at(&built.cells[0].spec, &solution, &stored)
            .expect("the stored cell spec resolves its tokens from its own folder");
    }
    drop(solution);
}

/// Without tokens, CSS colour keywords and block layout sizes behave as before batch 4.
#[test]
fn default_tokens_keep_css_keywords_and_block_layout_sizes() {
    let tokens = crate::core::mashup::tokens::Tokens::convergence();
    for keyword in ["grey", "white", "red", "black"] {
        assert_eq!(tokens.style("color", keyword).unwrap(), keyword);
        assert_eq!(tokens.style("background", keyword).unwrap(), keyword);
    }
    let (_dir, solution) = element_solution();
    for size in ["fit", "fill", "{fixed: 120}", "xl"] {
        let block = format!("block: {{id: b, color: pink, caption: OEE, value: 84, size: {size}}}");
        compile(&solution, "Acme", &error_page(&block, ""))
            .unwrap_or_else(|e| panic!("size {size}: {e}"));
    }
}
