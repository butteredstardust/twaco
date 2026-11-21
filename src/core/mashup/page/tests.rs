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
    let mut expected = normalized(&expected.content);
    // The unversioned primitive retains its legacy gutter; specs use the v1 page gutter.
    expected["UI"]["Widgets"][0]["Properties"]["Margin"] = serde_json::json!("24");
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
    assert_eq!(paths.len(), 19);
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
        let recipe = match kind {
            "flexcontainer" => Recipe::Card {
                container: target,
                caption: "Status".into(),
                value: Some("42".into()),
                status: Some("Healthy".into()),
                kind: "ok".into(),
            },
            "ptcschartline" => Recipe::Chart {
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
            },
            _ => Recipe::Pagination {
                widget: target,
                data,
            },
        };
        primitive.recipe(&solution, recipe).unwrap();
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
    let actual = serde_json::to_string_pretty(&serde_json::json!({
        "entity": String::from_utf8(built.entity).unwrap(),
        "content": serde_json::from_str::<Value>(&built.content).unwrap(),
        "css": built.css, "ids": built.ids
    }))
    .unwrap()
        + "\n";
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
fn benchmark_n_golden() {
    benchmark("n");
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
        ("distribution", "distribution_panel", "distribution_tools"),
        ("trend", "trend_panel", "trend_tools"),
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
    assert_eq!(props["Margin"], "20");
    let mut widths = vec![];
    for name in ["filters--apply", "filters--reset"] {
        let button = doc.find_exact(&built.ids[name]).unwrap();
        assert_eq!(&button.path[..button.path.len() - 1], footer.path);
        let props = &doc.at(&button.path)["Properties"];
        assert_eq!(props["Margin"], "0");
        widths.push(props["Width"].as_u64().unwrap());
    }
    assert_eq!(widths, [120, 120]);
    assert_eq!(widths.iter().sum::<u64>() + 2 * 20, 280);
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
    assert!(!built.ids.contains_key("second--dot"));
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
    assert_eq!(element_props(&built, "details")["ButtonType"], "tertiary");
}
