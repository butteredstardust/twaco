use super::super::{build, derive, extract};
use super::adopt::{apply, current, handmade, write_spec, HAND_SPEC};
use super::*;
use serde_json::json;

fn widget<'a>(value: &'a Value, id: &str) -> &'a Value {
    if value["Properties"]["Id"] == id {
        return value;
    }
    for child in value["Widgets"].as_array().into_iter().flatten() {
        let found = widget(child, id);
        if !found.is_null() {
            return found;
        }
    }
    &Value::Null
}

#[test]
fn fresh_copy_preserves_defaults_root_styles_custom_ids_and_empty_bound_text() {
    let (_dir, solution) = element_solution();
    let spec = r#"version: 1
rules: v1
page:
  name: Acme.Fresh_MU
  template: blank
  layout: as-built
  parameters:
    Manager: {type: THINGNAME, direction: BOTH, default: Acme.Manager}
    Count: {type: NUMBER, default: 12}
    Enabled: {type: BOOLEAN, default: false}
    Empty: {type: STRING, default: ''}
    Absent: {type: JSON, default: null}
  props: {suppressWidgetPositioning: false, suppressWidgetResize: true}
  styles: {root: {base: {background: '#123456'}}}
  root: {props: {Left: 0, Top: 0}}
  slots:
    center:
      - label: {id: label-title, widget_id: label-title, text: '', props: {Margin: '0'}}
      - button: {id: button-refresh, widget_id: button-refresh, text: Refresh}
  bindings:
    - {from: param.Manager, to: label-title.LabelText}
  events:
    - {on: button-refresh.Clicked, invoke: mashup.CloseIfPopup}
"#;
    let first = compile(&solution, "Acme", spec).unwrap();
    let mut before: Value = serde_json::from_str(&first.content).unwrap();
    before.as_object_mut().unwrap().remove("twacoPageRules");
    let derived = derive::derive_exact("Acme.Copy_MU", &before.to_string()).unwrap();
    let fresh = compile(&solution, "Acme", &derived.spec).unwrap();
    let after: Value = serde_json::from_str(&fresh.content).unwrap();
    for key in [
        "Manager",
        "Count",
        "Enabled",
        "Empty",
        "Absent",
        "StyleProperties",
        "suppressWidgetPositioning",
        "suppressWidgetResize",
    ] {
        assert_eq!(
            before["UI"]["Properties"][key], after["UI"]["Properties"][key],
            "{key}: {}",
            derived.spec
        );
    }
    assert_eq!(
        widget(&after["UI"], "label-title")["Properties"]["LabelText"],
        ""
    );
    assert!(!widget(&after["UI"], "button-refresh").is_null());
    assert_eq!(
        before["UI"]["Widgets"][0]["Properties"]["Left"],
        after["UI"]["Widgets"][0]["Properties"]["Left"]
    );
    assert!(super::super::align::lost_wiring(&before, &after).is_empty());
    for tier in [mashup::diff::Tier::Executable, mashup::diff::Tier::Layout] {
        let diff = mashup::diff::compare(
            "Acme.Fresh_MU",
            "Acme.Copy_MU",
            &before,
            &after,
            None,
            None,
            tier,
        );
        assert!(diff.equal, "{diff:?}");
    }
    let mut changed = after.clone();
    changed["UI"]["Properties"]["Manager"] = json!("Acme.Other");
    let diff = mashup::diff::compare(
        "a",
        "b",
        &after,
        &changed,
        None,
        None,
        mashup::diff::Tier::Executable,
    );
    assert!(
        !diff.equal
            && diff
                .differences
                .iter()
                .any(|d| d.path == "UI.Properties.Manager")
    );
    changed = after.clone();
    changed["UI"]["Properties"]["Count"] = json!(13);
    assert!(
        mashup::diff::compare(
            "a",
            "b",
            &after,
            &changed,
            None,
            None,
            mashup::diff::Tier::Executable
        )
        .equal
    );
}

#[test]
fn dynamic_service_to_service_bindings_and_parameter_events_survive_a_fresh_copy() {
    let (_dir, solution) = element_solution();
    let folder = solution.root.join("acme/ThingShapes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Acme.Manager_TS.xml"), r#"<Entities><ThingShapes><ThingShape name="Acme.Manager_TS" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="Resolve"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Rows_DS"/></ServiceDefinition><ServiceDefinition name="Create"><ParameterDefinitions><FieldDefinition name="amount" baseType="NUMBER"/></ParameterDefinitions><ResultType baseType="NOTHING"/></ServiceDefinition></ServiceDefinitions></ThingShape></ThingShape></ThingShapes></Entities>"#).unwrap();
    let spec = r#"version: 1
rules: v1
page:
  name: Acme.Dynamic_MU
  template: blank
  layout: as-built
  parameters: {Manager: {type: THINGNAME, direction: BOTH, default: Acme.Manager}}
  data:
    resolve: {service: Acme.Manager_TS.Resolve, entity: {bind: param.Manager}}
    create: {service: Acme.Manager_TS.Create, params: {amount: {bind: resolve.Count}}}
  events:
    - {on: mashup.ManagerChanged, invoke: resolve}
"#;
    let first = compile(&solution, "Acme", spec).unwrap();
    let mut before: Value = serde_json::from_str(&first.content).unwrap();
    let duplicate = before["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["SourceArea"] == "Data")
        .unwrap()
        .clone();
    before["DataBindings"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    let derived = derive::derive_exact("Acme.Copy_MU", &before.to_string()).unwrap();
    let fresh = compile(&solution, "Acme", &derived.spec).unwrap();
    let after: Value = serde_json::from_str(&fresh.content).unwrap();
    assert!(
        super::super::align::lost_wiring(&before, &after).is_empty(),
        "{}",
        derived.spec
    );
    let diff = mashup::diff::compare(
        "a",
        "b",
        &before,
        &after,
        None,
        None,
        mashup::diff::Tier::Executable,
    );
    assert!(diff.equal, "{diff:?}");
    let mut changed = after.clone();
    let binding = changed["DataBindings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|b| b["SourceArea"] == "Data")
        .unwrap();
    binding["SourceDetails"] = json!("SelectedRows");
    assert!(
        !mashup::diff::compare(
            "a",
            "b",
            &after,
            &changed,
            None,
            None,
            mashup::diff::Tier::Executable
        )
        .equal
    );
    assert!(!super::super::align::lost_wiring(&after, &changed).is_empty());
}

#[test]
fn a_text_conflict_points_to_the_text_line() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Conflict_MU\n  template: blank\n  slots:\n    center:\n      - label:\n          id: ptcslabel-12\n          text: Original\n";
    let source = solution.root.join("source.yaml");
    std::fs::write(&source, spec).unwrap();
    let mut plan = build::plan(&solution, "Acme.Conflict_MU", None, Some(&source)).unwrap();
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(&solution, &guard).unwrap();
    drop(guard);
    let folder = solution.src_root().join("Acme.Conflict_MU");
    let content = std::fs::read_to_string(folder.join("mashup/content.json"))
        .unwrap()
        .replace("Original", "Hand");
    let loaded = authoring::load_existing(&solution, "Acme.Conflict_MU").unwrap();
    let assets = mashup::Assets {
        content: content.clone(),
        css: loaded.assets.css,
    };
    std::fs::write(folder.join("mashup/content.json"), content).unwrap();
    std::fs::write(
        solution.root.join("acme/Mashups/Acme.Conflict_MU.xml"),
        mashup::sync(&loaded.entity_before, &assets).unwrap().0,
    )
    .unwrap();
    std::fs::write(folder.join("page.yaml"), spec.replace("Original", "Spec")).unwrap();
    let plan = build::plan(&solution, "Acme.Conflict_MU", None, None).unwrap();
    assert_eq!(
        plan.report["conflicts"].as_array().unwrap().len(),
        1,
        "{}",
        plan.report
    );
    assert_eq!(plan.report["conflicts"][0]["line"], 10, "{}", plan.report);
    assert!(plan.report["conflicts"][0]["message"]
        .as_str()
        .unwrap()
        .contains("(text)"));
}

#[test]
fn session_sources_derive_and_build_native_interfaces_and_typed_bindings() {
    let (_dir, solution) = element_solution();
    let spec = r#"version: 1
rules: v1
page:
  name: Acme.Session_MU
  template: blank
  layout: as-built
  data:
    user: {session: UserExtensions, fields: {name: STRING}}
    session: {session: Session, fields: {enabled: BOOLEAN}}
  slots:
    center:
      - label: {id: label-user, props: {LabelText: {bind: user.name}, Visible: {bind: session.enabled}}}
"#;
    let first = compile(&solution, "Acme", spec).unwrap();
    let before: Value = serde_json::from_str(&first.content).unwrap();
    assert_eq!(before["Data"]["UserExtensions"]["Id"], "UserExtensions");
    assert_eq!(before["Data"]["Session"]["Id"], "session");
    assert_eq!(before["DataBindings"][0]["SourceArea"], "UserExtensions");
    assert_eq!(
        before["DataBindings"][1]["PropertyMaps"][0]["SourcePropertyBaseType"],
        "BOOLEAN"
    );
    let derived = derive::derive_exact("Acme.Copy_MU", &first.content).unwrap();
    let fresh = compile(&solution, "Acme", &derived.spec).unwrap();
    let after: Value = serde_json::from_str(&fresh.content).unwrap();
    assert!(
        super::super::align::lost_wiring(&before, &after).is_empty(),
        "{}",
        derived.spec
    );
    let diff = mashup::diff::compare(
        "Acme.Session_MU",
        "Acme.Copy_MU",
        &before,
        &after,
        None,
        None,
        mashup::diff::Tier::Executable,
    );
    assert!(diff.equal, "{diff:?}");
}

#[test]
fn adopted_page_and_build_agree_about_hand_edits_and_report_the_property() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, HAND_SPEC, "Acme.Hand_MU");
    let loaded = authoring::load_existing(&solution, &name).unwrap();
    let mut content: Value = serde_json::from_str(&loaded.assets.content).unwrap();
    content["CustomMashupCss"] = json!(".acme { color: red; }");
    let assets = mashup::Assets {
        content: mashup::to_sidecar(&content),
        css: ".acme { color: red; }".into(),
    };
    std::fs::write(
        solution.root.join(format!("acme/Mashups/{name}.xml")),
        mashup::sync(&loaded.entity_before, &assets).unwrap().0,
    )
    .unwrap();
    let derived = extract::without_written_css(extract::extract(&solution, &name).unwrap());
    write_spec(&solution, &name, &derived.spec);
    apply(&solution, &name);
    let found = extract::extract(&solution, &name).unwrap();
    assert!(found.exact && found.hand_edited.is_empty(), "{found:?}");
    let no_op = build::plan(&solution, &name, None, None).unwrap();
    assert_eq!(
        no_op.report["files_state"], "nothing to write",
        "{}",
        no_op.report
    );
    let mut value = current(&solution, &name);
    fn edit(w: &mut Value) -> Option<String> {
        if w["Properties"]["Type"] == "ptcslabel" {
            w["Properties"]["LabelText"] = json!("Hand text");
            return w["Properties"]["Id"].as_str().map(str::to_string);
        }
        w["Widgets"].as_array_mut()?.iter_mut().find_map(edit)
    }
    let id = edit(&mut value["UI"]).unwrap();
    let loaded = authoring::load_existing(&solution, &name).unwrap();
    let assets = mashup::Assets {
        content: mashup::to_sidecar(&value),
        css: loaded.assets.css,
    };
    let xml = solution.root.join(format!("acme/Mashups/{name}.xml"));
    std::fs::write(
        solution.src_root().join(&name).join("mashup/content.json"),
        &assets.content,
    )
    .unwrap();
    std::fs::write(xml, mashup::sync(&loaded.entity_before, &assets).unwrap().0).unwrap();
    let plan = build::plan(&solution, &name, None, None).unwrap();
    let edits = plan.report["hand_edits_kept"].as_array().unwrap();
    assert_eq!(edits.len(), 1, "{}", plan.report);
    assert_eq!(edits[0], format!("{id}.LabelText"));
    assert_eq!(
        extract::extract(&solution, &name)
            .unwrap()
            .hand_edited
            .len(),
        1
    );
    write_spec(
        &solution,
        &name,
        &derived.spec.replace("text: Acme", "text: Spec"),
    );
    let conflict = build::plan(&solution, &name, None, None).unwrap();
    assert_eq!(
        conflict.report["conflicts"].as_array().unwrap().len(),
        1,
        "{}",
        conflict.report
    );
    assert_eq!(
        conflict.report["hand_edits_kept"],
        json!([]),
        "{}",
        conflict.report
    );
}

#[test]
fn new_name_overrides_the_spec_and_export_creates_missing_parent() {
    let (_dir, solution) = element_solution();
    let source = solution.root.join("source.yaml");
    std::fs::write(solution.root.join("source.css"), ".acme { color: red; }\n").unwrap();
    std::fs::write(
        &source,
        "version: 1\nrules: v1\npage:\n  name: Acme.Source_MU\n  template: blank\n  css: source.css\n",
    )
    .unwrap();
    let mut plan = build::plan(&solution, "Acme.Copy_MU", None, Some(&source)).unwrap();
    assert!(plan.report["cell_notices"]
        .to_string()
        .contains("overridden"));
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(&solution, &guard).unwrap();
    drop(guard);
    let stored =
        std::fs::read_to_string(solution.src_root().join("Acme.Copy_MU/page.yaml")).unwrap();
    assert!(stored.contains("Acme.Copy_MU") && !stored.contains("Acme.Source_MU"));
    let out = solution.root.join("missing/nested/page.yaml");
    extract::export(&solution, "Acme.Copy_MU", Some(&out)).unwrap();
    assert!(out.is_file());
    assert!(out.parent().unwrap().join("page.css").is_file());
}

#[test]
fn navigation_return_bindings_bound_geometry_and_native_reset_are_carried() {
    let (_dir, solution) = element_solution();
    let child = compile(&solution, "Acme", "version: 1\nrules: v1\npage:\n  name: Acme.Child_MU\n  template: blank\n  parameters: {returned: STRING:BOTH}\n").unwrap();
    std::fs::write(
        solution.root.join("acme/Mashups/Acme.Child_MU.xml"),
        child.entity,
    )
    .unwrap();
    let spec = r#"version: 1
rules: v1
page:
  name: Acme.Navigation_MU
  template: blank
  layout: as-built
  data:
    height: {expression: '500', output: NUMBER}
  slots:
    center:
      - button: {id: ptcsbutton-12, text: Open}
      - widget: {id: navigationfunction-13, type: navigationfunction, props: {TargetMashup: Acme.Child_MU, TargetWindowType: popup, PopupHeight: {bind: height}}}
      - label: {id: ptcslabel-14, props: {LabelText: {bind: navigationfunction-13.returned}}}
      - widget: {id: ptcstogglebutton-15, type: ptcstogglebutton}
  events:
    - {on: ptcsbutton-12.Clicked, invoke: navigationfunction-13.Navigate}
    - {on: ptcsbutton-12.Clicked, invoke: ptcstogglebutton-15.ResetToDefaultValue}
"#;
    let first = compile(&solution, "Acme", spec).unwrap();
    let before: Value = serde_json::from_str(&first.content).unwrap();
    let derived = derive::derive_exact("Acme.Copy_MU", &first.content).unwrap();
    assert!(
        derived.spec.contains("type: navigationfunction"),
        "{}",
        derived.spec
    );
    let fresh = compile(&solution, "Acme", &derived.spec).unwrap();
    let after: Value = serde_json::from_str(&fresh.content).unwrap();
    assert!(
        super::super::align::lost_wiring(&before, &after).is_empty(),
        "{}",
        derived.spec
    );
    assert!(
        mashup::diff::compare(
            "a",
            "b",
            &before,
            &after,
            None,
            None,
            mashup::diff::Tier::Executable
        )
        .equal
    );
}

#[test]
fn unknown_widget_geometry_and_reordered_expression_identity_survive() {
    let (_dir, solution) = element_solution();
    let spec = r#"version: 1
rules: v1
page:
  name: Acme.Functions_MU
  template: blank
  layout: as-built
  data:
    first: {expression: '1', output: NUMBER}
    second: {expression: '2', output: NUMBER}
  slots:
    center:
      - widget: {id: gauge-90, type: gauge, props: {ResponsiveLayout: true, Z-index: 10}}
      - label: {id: ptcslabel-91, props: {LabelText: {bind: first}}}
"#;
    let built = compile(&solution, "Acme", spec).unwrap();
    let mut before: Value = serde_json::from_str(&built.content).unwrap();
    before["UI"]["Widgets"].as_array_mut().unwrap().reverse();
    let derived = derive::derive_exact("Acme.Copy_MU", &before.to_string()).unwrap();
    let fresh = compile(&solution, "Acme", &derived.spec).unwrap();
    let after: Value = serde_json::from_str(&fresh.content).unwrap();
    for tier in [mashup::diff::Tier::Executable, mashup::diff::Tier::Layout] {
        let diff = mashup::diff::compare("a", "b", &before, &after, None, None, tier);
        assert!(diff.equal, "{diff:?}\n{}", derived.spec);
    }
    for expression in before["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["Properties"]["Type"] == "expression2")
    {
        let id = expression["Properties"]["Id"].as_str().unwrap();
        assert_eq!(
            widget(&after["UI"], id)["Properties"]["Expression"],
            expression["Properties"]["Expression"]
        );
    }
}

fn brief81_copy(solution: &Solution, before: &Value) -> (Value, String) {
    let derived = derive::derive_exact("Acme.Copy_MU", &before.to_string()).unwrap();
    let made = compile(solution, "Acme", &derived.spec)
        .unwrap_or_else(|e| panic!("{e}\n{}", derived.spec));
    let after: Value = serde_json::from_str(&made.content).unwrap();
    let mut problems = vec![];
    mashup::validate::check_content(&made.content, &mut problems);
    assert!(
        !problems
            .iter()
            .any(|p| p.severity == mashup::validate::Severity::Error),
        "{problems:?}\n{}",
        derived.spec
    );
    assert!(
        super::super::align::lost_wiring(before, &after).is_empty(),
        "{}",
        derived.spec
    );
    let diff = mashup::diff::compare(
        "a",
        "b",
        before,
        &after,
        None,
        None,
        mashup::diff::Tier::Executable,
    );
    assert!(diff.equal, "{diff:?}\n{}", derived.spec);
    (after, derived.spec)
}

const BRIEF81: &str =
    "version: 1\nrules: v1\npage:\n  name: Acme.Source_MU\n  template: blank\n  layout: as-built\n";

#[test]
fn brief81_image_positions() {
    let (_dir, solution) = element_solution();
    for position in ["left", "right", "top", "bottom", "center"] {
        let spec = format!("{BRIEF81}  slots:\n    center:\n      - widget: {{id: ptcsimage-3, type: ptcsimage, props: {{Position: {position}}}}}\n");
        let built = compile(&solution, "Acme", &spec).unwrap();
        let before = serde_json::from_str(&built.content).unwrap();
        let (after, _) = brief81_copy(&solution, &before);
        assert_eq!(
            widget(&after["UI"], "ptcsimage-3")["Properties"]["Position"],
            position
        );
    }
}

#[test]
fn brief81_collection_case_and_read_only() {
    let (_dir, solution) = element_solution();
    let spec = format!("{BRIEF81}  slots:\n    center:\n      - widget: {{id: ptcscollection-3, type: ptcscollection, props: {{CellPointer: Auto}}}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    let mut before: Value = serde_json::from_str(&built.content).unwrap();
    before["UI"]["Widgets"][0]["Widgets"][0]["Properties"]["HasSelectedCells"] = json!(false);
    let (_, spec) = brief81_copy(&solution, &before);
    assert!(!spec.contains("HasSelectedCells"), "{spec}");
    brief81_extract_copy(&solution, &built.entity, &before);
}

#[test]
fn brief81_chart_numbered_sources() {
    let (_dir, solution) = element_solution();
    let spec = format!("{BRIEF81}  data:\n    rows: {{service: Acme.Data.GetRows}}\n  slots:\n    center:\n      - widget: {{id: ptcschartline-3, type: ptcschartline, props: {{MultipleDataSources: true, NumberOfSeries: 2}}}}\n  bindings:\n    - {{from: rows, to: ptcschartline-3.DataSource1}}\n    - {{from: rows, to: ptcschartline-3.DataSource2}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    brief81_copy(&solution, &serde_json::from_str(&built.content).unwrap());
    let error = compile(
        &solution,
        "Acme",
        &spec.replace("DataSource2", "DataSource3"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("DataSource3"), "{error}");
}

#[test]
fn brief81_master_host_through_legacy_container() {
    let (_dir, solution) = element_solution();
    let spec = format!("{BRIEF81}  kind: master\n  root: {{id: flexcontainer-1}}\n  slots:\n    center:\n      - row: {{id: flexcontainer-2, children: [{{label: {{id: ptcslabel-9}}}}]}}\n      - row: {{id: flexcontainer-7, children: [{{widget: {{id: container-4, type: container, children: [{{page_host: {{id: pagemashupcontainer-5}}}}]}}}}]}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    let mut before: Value = serde_json::from_str(&built.content).unwrap();
    before["UI"]["Widgets"][0]["Widgets"][1]["Widgets"][0]["Properties"]["Type"] =
        json!("container");
    let (after, derived) = brief81_copy(&solution, &before);
    assert!(derived.contains("page_host"), "{derived}");
    assert!(!widget(&after["UI"], "pagemashupcontainer-5").is_null());
    brief81_extract_copy(&solution, &built.entity, &before);
}

#[test]
fn brief81_unknown_widget_events_and_bindings() {
    let (_dir, solution) = element_solution();
    let spec = format!("{BRIEF81}  slots:\n    center:\n      - widget: {{id: AcmeViewer-7, type: AcmeViewer, props: {{Value: hello}}}}\n      - label: {{id: ptcslabel-8}}\n  bindings:\n    - {{from: AcmeViewer-7.Value, to: ptcslabel-8.LabelText}}\n  events:\n    - {{on: AcmeViewer-7.Clicked, invoke: AcmeViewer-7.Reset}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    assert!(
        built
            .warnings
            .iter()
            .any(|w| w.contains("AcmeViewer") && w.contains("cannot be checked")),
        "{:?}",
        built.warnings
    );
    brief81_copy(&solution, &serde_json::from_str(&built.content).unwrap());
    let navigation = spec.replace(
        "invoke: AcmeViewer-7.Reset",
        "navigate: {mashup: Acme.External_MU}",
    );
    let built = compile(&solution, "Acme", &navigation).unwrap();
    brief81_copy(&solution, &serde_json::from_str(&built.content).unwrap());
    brief81_extract_copy(
        &solution,
        &built.entity,
        &serde_json::from_str(&built.content).unwrap(),
    );
}

#[test]
fn brief81_expression_extra_boolean_input() {
    let (_dir, solution) = element_solution();
    let spec = format!("{BRIEF81}  parameters: {{selected: BOOLEAN}}\n  data:\n    expression2_12:\n      widget_id: expression2-12\n      expression: 'input2 ? input : input;'\n      output: STRING\n      inputs:\n        input: {{type: STRING}}\n        input2: {{type: BOOLEAN, value: {{bind: param.selected}}}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    let (after, _) = brief81_copy(&solution, &serde_json::from_str(&built.content).unwrap());
    assert!(
        widget(&after["UI"], "expression2-12")["Properties"]["_currentParameterDefs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["ParameterName"] == "input2" && d["BaseType"] == "BOOLEAN")
    );
    brief81_extract_copy(
        &solution,
        &built.entity,
        &serde_json::from_str(&built.content).unwrap(),
    );
}

#[test]
fn brief81_static_dynamic_service_parameters_survive_extraction_and_build() {
    let (_dir, solution) = element_solution();
    let folder = solution.root.join("acme/ThingShapes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Acme.Management_TS.xml"), r#"<Entities><ThingShapes><ThingShape name="Acme.Management_TS" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetRows"><ParameterDefinitions><FieldDefinition name="RobotName" baseType="THINGNAME"/></ParameterDefinitions><ResultType baseType="INFOTABLE"/></ServiceDefinition><ServiceDefinition name="GetOrders"><ParameterDefinitions><FieldDefinition name="RobotName" baseType="THINGNAME"/></ParameterDefinitions><ResultType baseType="INFOTABLE"/></ServiceDefinition></ServiceDefinitions></ThingShape></ThingShape></ThingShapes></Entities>"#).unwrap();
    let spec = format!("{BRIEF81}  parameters: {{Manager: {{type: THINGNAME, default: Acme.Manager}}}}\n  data:\n    rows: {{service: Acme.Management_TS.GetRows, entity: {{bind: param.Manager}}, params: {{RobotName: Acme.Robot01}}}}\n    orders: {{service: Acme.Management_TS.GetOrders, params: {{RobotName: Acme.Robot02}}}}\n    bound: {{service: Acme.Management_TS.GetOrders, params: {{RobotName: {{bind: param.Manager}}}}, fixed: {{RobotName: Acme.Robot03}}}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    let before: Value = serde_json::from_str(&built.content).unwrap();
    let (after, _) = brief81_copy(&solution, &before);
    for call in after["Data"]["DynamicThingShapes_Acme.Management_TS"]["Services"]
        .as_array()
        .unwrap()
    {
        let expected = match call["Name"].as_str().unwrap() {
            "rows" => "Acme.Robot01",
            "orders" => "Acme.Robot02",
            _ => "Acme.Robot03",
        };
        assert_eq!(call["Parameters"]["RobotName"], expected);
    }
    brief81_extract_copy(&solution, &built.entity, &before);
}

fn brief81_extract_copy(solution: &Solution, entity: &[u8], before: &Value) -> Value {
    let folder = solution.root.join("acme/Mashups");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("Acme.Source_MU.xml");
    let entity = mashup::sync(
        entity,
        &mashup::Assets {
            content: before.to_string(),
            css: String::new(),
        },
    )
    .unwrap()
    .0;
    std::fs::write(&path, &entity).unwrap();
    let extracted = extract::extract(solution, "Acme.Source_MU").unwrap();
    let spec = solution.root.join("brief81.page.yaml");
    std::fs::write(&spec, &extracted.spec).unwrap();
    let plan = build::plan(solution, "Acme.Copy_MU", Some("Acme"), Some(&spec)).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let made = compile(solution, "Acme", &extracted.spec).unwrap();
    let after = serde_json::from_str(&made.content).unwrap();
    let diff = mashup::diff::compare(
        "a",
        "b",
        before,
        &after,
        None,
        None,
        mashup::diff::Tier::Executable,
    );
    assert!(diff.equal, "{diff:?}\n{}", extracted.spec);
    write_spec(solution, "Acme.Source_MU", &extracted.spec);
    let plan = build::plan(solution, "Acme.Source_MU", None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    assert_eq!(std::fs::read(path).unwrap(), entity);
    after
}

#[test]
fn brief81_numeric_refresh_representation() {
    let (_dir, solution) = element_solution();
    let spec = format!("{BRIEF81}  slots:\n    center:\n      - widget: {{id: autorefreshfunction-9, type: autorefreshfunction, props: {{RefreshInterval: '600'}}}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    let before = serde_json::from_str(&built.content).unwrap();
    brief81_copy(&solution, &before);
    brief81_extract_copy(&solution, &built.entity, &before);
}

#[test]
fn brief81_external_navigation_warns_and_builds() {
    let (_dir, solution) = element_solution();
    let spec = format!("{BRIEF81}  slots:\n    center:\n      - button: {{id: ptcsbutton-3, text: Open}}\n  events:\n    - {{on: ptcsbutton-3.Clicked, navigate: {{mashup: Acme.External_MU, mode: popup}}}}\n");
    let spec = spec
        .replace("  slots:", "  parameters: {selected: BOOLEAN}\n  slots:")
        .replace(
            "mode: popup",
            "mode: popup, params: {caption: Acme, selected: {bind: param.selected}}",
        );
    let built = compile(&solution, "Acme", &spec).unwrap();
    assert!(
        built
            .warnings
            .iter()
            .any(|w| w.contains("Acme.External_MU") && w.contains("cannot be checked")),
        "{:?}",
        built.warnings
    );
    brief81_copy(&solution, &serde_json::from_str(&built.content).unwrap());
}

#[test]
fn brief81_navigation_cache_order_and_false_defaults() {
    let (_dir, solution) = element_solution();
    let spec = format!("{BRIEF81}  slots:\n    center:\n      - widget: {{id: navigationfunction-9, type: navigationfunction, props: {{TargetMashup: Acme.External_MU, MashupParameters: [{{ParameterName: zeta, BaseType: STRING}}, {{ParameterName: selected, BaseType: BOOLEAN}}], _currentParameterDefs: [{{ParameterName: zeta, BaseType: STRING}}, {{ParameterName: selected, BaseType: BOOLEAN, DefaultValue: false}}]}}}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    let before: Value = serde_json::from_str(&built.content).unwrap();
    let (after, _) = brief81_copy(&solution, &before);
    let source = &widget(&before["UI"], "navigationfunction-9")["Properties"];
    let fresh = &widget(&after["UI"], "navigationfunction-9")["Properties"];
    for key in ["MashupParameters", "_currentParameterDefs"] {
        assert_eq!(source[key], fresh[key], "{key}");
    }
    brief81_extract_copy(&solution, &built.entity, &before);
    let mut reversed = after.clone();
    reversed["UI"]["Widgets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|w| w["Properties"]["Type"] == "navigationfunction")
        .unwrap()["Properties"]["MashupParameters"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(
        mashup::diff::compare(
            "a",
            "b",
            &after,
            &reversed,
            None,
            None,
            mashup::diff::Tier::Executable
        )
        .equal
    );
}

#[test]
fn brief81_popup_close_and_user_extensions_fresh_copy() {
    let (_dir, solution) = element_solution();
    for body in ["  slots:\n    center:\n      - button: {id: ptcsbutton-7, text: Close}\n  events:\n    - {on: ptcsbutton-7.Clicked, invoke: mashup.CloseIfPopup}\n", "  data:\n    user: {session: UserExtensions, fields: {name: STRING}}\n  slots:\n    center:\n      - label: {id: ptcslabel-8, props: {LabelText: {bind: user.name}}}\n"] {
        let built = compile(&solution, "Acme", &format!("{BRIEF81}{body}")).unwrap();
        let before = serde_json::from_str(&built.content).unwrap();
        brief81_copy(&solution, &before);
        brief81_extract_copy(&solution, &built.entity, &before);
        let _ = std::fs::remove_dir_all(solution.src_root().join("Acme.Source_MU"));
    }
}

#[test]
fn brief81_expression_bound_to_the_root_widget() {
    let (_dir, solution) = element_solution();
    let spec = format!("{BRIEF81}  data:\n    expression2_5:\n      widget_id: expression2-5\n      expression: '\"acme-busy\"'\n      output: STRING\n  slots:\n    center:\n      - label: {{id: ptcslabel-6, text: Hi}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    let mut before: Value = serde_json::from_str(&built.content).unwrap();
    let root = before["UI"]["Properties"]["Id"]
        .as_str()
        .unwrap()
        .to_string();
    before["DataBindings"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "Id": "b-root", "SourceArea": "UI", "SourceId": "expression2-5", "SourceSection": "",
            "SourceDetails": "", "TargetArea": "UI", "TargetId": root, "TargetSection": "",
            "PropertyMaps": [{"SourceProperty": "Output", "SourcePropertyBaseType": "STRING",
                "SourcePropertyType": "property", "TargetProperty": "CustomClass",
                "TargetPropertyBaseType": "STRING", "TargetPropertyType": "property"}]
        }));
    brief81_copy(&solution, &before);
}

#[test]
fn fresh_copy_keeps_the_stored_styles_of_the_root_container() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1
rules: v1
page:
  name: Acme.RootStyle_MU
  template: blank
  layout: as-built
  root: {props: {DisplayName: page}, styles: {$: {$: {background: '#ffffff'}}}}
  slots:
    center:
      - label: {id: out, text: Hello}
";
    let first = compile(&solution, "Acme", spec).unwrap();
    let mut before: Value = serde_json::from_str(&first.content).unwrap();
    before.as_object_mut().unwrap().remove("twacoPageRules");
    let root_id = before["UI"]["Widgets"][0]["Properties"]["Id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        before["UI"]["Properties"]["StyleProperties"]["data"][&root_id]["parts"]["$"]["$"]
            ["background"],
        "#ffffff"
    );
    for exact in [true, false] {
        let derived = if exact {
            derive::derive_exact("Acme.Copy_MU", &before.to_string())
        } else {
            derive::derive_with_styles("Acme.Copy_MU", &before.to_string(), true)
        }
        .unwrap();
        assert!(
            derived.notes.iter().all(|n| !n.contains("not carried")),
            "{:?}",
            derived.notes
        );
        let fresh = compile(&solution, "Acme", &derived.spec).unwrap();
        let after: Value = serde_json::from_str(&fresh.content).unwrap();
        let copy_id = after["UI"]["Widgets"][0]["Properties"]["Id"]
            .as_str()
            .unwrap();
        assert_eq!(
            after["UI"]["Properties"]["StyleProperties"]["data"][copy_id]["parts"]["$"]["$"]
                ["background"],
            "#ffffff",
            "exact={exact}: {}",
            derived.spec
        );
    }
}

#[test]
fn a_binding_into_the_root_containers_props_is_written() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1
rules: v1
page:
  name: Acme.RootBind_MU
  template: blank
  layout: as-built
  data:
    cls: {expression: \"'acme-wide'\", output: STRING}
  root: {props: {DisplayName: page, CustomClass: {bind: cls}}}
  slots:
    center:
      - label: {id: out, text: Hello}
";
    let built = compile(&solution, "Acme", spec).unwrap();
    let content: Value = serde_json::from_str(&built.content).unwrap();
    let root = content["UI"]["Widgets"][0]["Properties"]["Id"]
        .as_str()
        .unwrap();
    assert!(
        content["DataBindings"].as_array().unwrap().iter().any(|b| {
            b["TargetId"] == root
                && b["PropertyMaps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["TargetProperty"] == "CustomClass")
        }),
        "{}",
        content["DataBindings"]
    );
    assert!(content["UI"]["Widgets"][0]["Properties"]["CustomClass"].is_null());
    let derived = derive::derive_exact("Acme.Copy_MU", &built.content).unwrap();
    let fresh = compile(&solution, "Acme", &derived.spec).unwrap();
    let after: Value = serde_json::from_str(&fresh.content).unwrap();
    assert!(
        super::super::align::lost_wiring(&content, &after).is_empty(),
        "{}",
        derived.spec
    );
}

#[test]
fn an_extension_widgets_binding_keeps_the_target_base_type_through_derive() {
    let (_dir, solution) = element_solution();
    std::fs::write(solution.root.join("acme/Things/Acme.Logo.xml"), r#"<Entities><Things><Thing name="Acme.Logo" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetLogo"><ParameterDefinitions/><ResultType baseType="STRING"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#).unwrap();
    let spec = "version: 1
rules: v1
page:
  name: Acme.ExtBind_MU
  template: blank
  layout: as-built
  parameters: {logo: STRING}
  data:
    link: {expression: \"'a.svg'\", output: STRING}
    logo: {service: Acme.Logo.GetLogo, on: load}
  slots:
    center:
      - widget: {id: viewer, widget_id: acmeviewer-7, type: acmeviewer, binding_types: {SVGFileUrl: IMAGELINK, Alt: IMAGELINK, Src: IMAGELINK}}
  bindings:
    - {from: param.logo, to: viewer.SVGFileUrl}
    - {from: link, to: viewer.Alt}
    - {from: logo.result, to: viewer.Src}
";
    let built = compile(&solution, "Acme", spec).unwrap();
    let before: Value = serde_json::from_str(&built.content).unwrap();
    let base = |content: &Value| {
        let types: Vec<_> = content["DataBindings"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|b| b["PropertyMaps"].as_array().unwrap().clone())
            .filter(|m| {
                ["SVGFileUrl", "Alt", "Src"].contains(&m["TargetProperty"].as_str().unwrap())
            })
            .map(|m| m["TargetPropertyBaseType"].clone())
            .collect();
        assert_eq!(types.len(), 3, "{}", content["DataBindings"]);
        json!(types)
    };
    assert_eq!(
        base(&before),
        json!(["IMAGELINK", "IMAGELINK", "IMAGELINK"]),
        "{}",
        before["DataBindings"]
    );
    for exact in [true, false] {
        let derived = if exact {
            derive::derive_exact("Acme.Copy_MU", &built.content)
        } else {
            derive::derive_with_styles("Acme.Copy_MU", &built.content, true)
        }
        .unwrap();
        assert!(derived.spec.contains("IMAGELINK"), "{}", derived.spec);
        let fresh = compile(&solution, "Acme", &derived.spec).unwrap();
        let after: Value = serde_json::from_str(&fresh.content).unwrap();
        assert_eq!(
            base(&after),
            json!(["IMAGELINK", "IMAGELINK", "IMAGELINK"]),
            "exact={exact}: {}",
            derived.spec
        );
    }
}
