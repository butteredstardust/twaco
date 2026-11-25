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
