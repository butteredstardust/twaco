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
