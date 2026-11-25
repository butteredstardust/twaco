//! Contracts that tell the truth about the platform: built-in services resolve, legacy names the
//! Composer migrates are carried, and what only `check` can know does not stop a build.
use super::super::derive;
use super::*;
use serde_json::json;

fn source(solution: &Solution, body: &str) -> Value {
    let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Source_MU\n  template: blank\n  layout: as-built\n{body}");
    let built = compile(solution, "Acme", &text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    serde_json::from_str(&built.content).unwrap()
}

/// Derive through both paths, build a fresh copy and require no executable difference.
fn roundtrip(solution: &Solution, before: &Value) -> Vec<(String, Value)> {
    [true, false]
        .into_iter()
        .map(|exact| {
            let derived = if exact {
                derive::derive_exact("Acme.Copy_MU", &before.to_string())
            } else {
                derive::derive("Acme.Copy_MU", &before.to_string())
            }
            .unwrap();
            let built = compile(solution, "Acme", &derived.spec)
                .unwrap_or_else(|e| panic!("exact={exact}: {e}\n{}", derived.spec));
            let after: Value = serde_json::from_str(&built.content).unwrap();
            let diff = mashup::diff::compare(
                "Acme.Source_MU",
                "Acme.Copy_MU",
                before,
                &after,
                None,
                None,
                mashup::diff::Tier::Executable,
            );
            assert!(diff.equal, "exact={exact}: {diff:?}\n{}", derived.spec);
            (derived.spec, after)
        })
        .collect()
}

fn solution_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Solution) {
    let (dir, solution) = element_solution();
    for (path, text) in files {
        let path = dir.path().join("acme").join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    (dir, solution)
}

const TEMPLATE: (&str, &str) = (
    "ThingTemplates/Acme.Tpl.xml",
    r#"<Entities><ThingTemplates><ThingTemplate name="Acme.Tpl" projectName="Acme" baseThingTemplate="GenericThing"><ThingShape><PropertyDefinitions><PropertyDefinition name="Level" baseType="NUMBER" ordinal="0"/></PropertyDefinitions></ThingShape></ThingTemplate></ThingTemplates></Entities>"#,
);

#[test]
fn platform_services_resolve_without_a_cache_on_dynamic_and_static_template_sources() {
    let (_dir, solution) = solution_with(&[TEMPLATE]);
    // A dynamic source calls the Things' services (GetProperties); a static
    // template source calls the template's own (GetImplementingThingsWithData).
    let dynamic = source(
        &solution,
        "  parameters: {Machine: THINGNAME}
  data:
    Props: {service: Acme.Tpl.GetProperties, on: load, entity: {bind: param.Machine}}
  slots:
    center:
      - label: {id: ptcslabel-5, props: {LabelText: {bind: Props.Level}}}
",
    );
    assert!(dynamic["Data"]["DynamicThingTemplates_Acme.Tpl"].is_object());
    roundtrip(&solution, &dynamic);
    let mut fixed = source(
        &solution,
        "  data:
    Rows: {service: Acme.Tpl.GetImplementingThingsWithData, entity_type: template, on: load}
    Query: {service: Acme.Tpl.QueryImplementingThingsWithData, entity_type: template, on: load, params: {maxItems: 5}}
  slots:
    center:
      - label: {id: ptcslabel-5, props: {LabelText: {bind: Rows.name}}}
",
    );
    fixed["Data"]["ThingTemplates_Acme.Tpl"]
        .as_object_mut()
        .unwrap()
        .remove("RefreshInterval");
    roundtrip(&solution, &fixed);
}

#[test]
fn a_misspelled_platform_service_still_fails_and_the_hint_names_the_missing_entity() {
    let (_dir, solution) = solution_with(&[TEMPLATE]);
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Source_MU\n  template: blank\n  layout: as-built\n  parameters: {Machine: THINGNAME}\n  data:\n    Props: {service: Acme.Tpl.GetPropertiez, on: load, entity: {bind: param.Machine}}\n  slots:\n    center:\n      - label: {id: ptcslabel-5, text: x}\n";
    let error = compile(&solution, "Acme", text).unwrap_err().to_string();
    assert!(error.contains("GetPropertiez"), "{error}");
    assert!(error.contains("ThingTemplates/GenericThing"), "{error}");
    assert!(
        !error.contains("to fetch the platform's templates and shapes, then"),
        "{error}"
    );
}

#[test]
fn a_shape_without_the_service_resolves_through_its_implementers_for_a_dynamic_source() {
    let (_dir, solution) = solution_with(&[
        (
            "ThingShapes/Acme.Manager_TS.xml",
            r#"<Entities><ThingShapes><ThingShape name="Acme.Manager_TS" projectName="Acme"><ThingShape/></ThingShape></ThingShapes></Entities>"#,
        ),
        (
            "Things/Acme.Manager.xml",
            r#"<Entities><Things><Thing name="Acme.Manager" projectName="Acme" thingTemplate="GenericThing"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetExport"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Rows_DS"/></ServiceDefinition></ServiceDefinitions></ThingShape><ImplementedShapes><ImplementedShape name="Acme.Manager_TS"/></ImplementedShapes></Thing></Things></Entities>"#,
        ),
    ]);
    let before = source(
        &solution,
        "  parameters: {Manager: THINGNAME}\n  data:\n    GetExport: {service: Acme.Manager_TS.GetExport, entity: {bind: param.Manager}, on: load}\n  slots:\n    center:\n      - label: {id: ptcslabel-5, props: {LabelText: {bind: GetExport.Name}}}\n",
    );
    assert!(before["Data"]["DynamicThingShapes_Acme.Manager_TS"].is_object());
    roundtrip(&solution, &before);
    let _ = json!(null);
}

#[test]
fn a_dropdown_display_field_the_service_does_not_declare_warns_instead_of_refusing() {
    let (_dir, solution) = element_solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Source_MU\n  template: blank\n  layout: as-built\n  data:\n    rows: {service: Acme.Data.GetRows, on: load}\n  slots:\n    center:\n      - dropdown: {id: ptcsdropdown-19, options: rows, display: nameValue, hint: Pick, props: {ValueField: nameValue}}\n";
    let built = compile(&solution, "Acme", text).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        built
            .warnings
            .iter()
            .any(|w| w.contains("no field nameValue") && w.contains("check")),
        "{:?}",
        built.warnings
    );
    let content: Value = serde_json::from_str(&built.content).unwrap();
    let dropdown = &content["UI"]["Widgets"][0]["Widgets"][0];
    assert_eq!(dropdown["Properties"]["Id"], "ptcsdropdown-19", "{content}");
    assert_eq!(dropdown["Properties"]["ValueField"], "nameValue");
}

#[test]
fn grid_events_and_a_collection_cell_event_survive_both_derives() {
    let (_dir, solution) = element_solution();
    let before = source(
        &solution,
        "  data:
    rows: {service: Acme.Data.GetRows, on: load}
    sel: {expression: \"result = input;\", output: STRING, inputs: {input: {type: STRING, value: {bind: rows.SelectedRows.Name}}}, auto_evaluate: true, fire_on_first_value: false, data_change: VALUE}
  slots:
    center:
      - widget:
          id: ptcsdynamicpanel-20
          type: ptcsdynamicpanel
          children:
            - column: {id: flexcontainer-21, children: [{widget: {id: ptcsgrid-46, type: ptcsgrid, props: {Data: {bind: rows}}}}]}
            - column: {id: flexcontainer-22, children: [{label: {id: ptcslabel-23, text: Acme}}]}
  events:
    - {on: ptcsgrid-46.SelectedRowsChanged, invoke: sel.Evaluate}
",
    );
    let derived = roundtrip(&solution, &before);
    for (spec, _) in derived {
        assert!(spec.contains("ptcsgrid-46.SelectedRowsChanged"), "{spec}");
    }
}

#[test]
fn a_collection_surfaces_event_fields_as_sources() {
    let (_dir, solution) = element_solution();
    let before = source(
        &solution,
        "  data:\n    rows: {service: Acme.Data.GetRows, on: load}\n  slots:\n    center:\n      - widget: {id: ptcscollection-7, type: ptcscollection, props: {Data: {bind: rows}}}\n      - label: {id: ptcslabel-9, text: x}\n  bindings:\n    - {from: ptcscollection-7.Event:Name, to: ptcslabel-9.LabelText}\n",
    );
    let maps = before["DataBindings"].to_string();
    assert!(maps.contains("Event:Name"), "{maps}");
    roundtrip(&solution, &before);
}

#[test]
fn a_contained_mashup_chosen_at_run_time_warns_instead_of_refusing_unknown_parameters() {
    let (_dir, solution) = element_solution();
    let text = "version: 1\nrules: v1\npage:\n  name: Acme.Source_MU\n  template: blank\n  layout: as-built\n  parameters: {which: MASHUPNAME, menu: STRING}\n  slots:\n    center:\n      - widget: {id: mashupcontainer-10, type: mashupcontainer, props: {Name: Acme.Child_MU}}\n  bindings:\n    - {from: param.which, to: mashupcontainer-10.Name}\n    - {from: param.menu, to: mashupcontainer-10.selectedMenuItem}\n";
    let built = compile(&solution, "Acme", text).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        built
            .warnings
            .iter()
            .any(|w| w.contains("mashupcontainer-10") && w.contains("run time")),
        "{:?}",
        built.warnings
    );
    // The same binding to a container with a fixed mashup is still refused.
    let fixed = text.replace(
        "    - {from: param.which, to: mashupcontainer-10.Name}\n",
        "",
    );
    let error = compile(&solution, "Acme", &fixed).unwrap_err().to_string();
    assert!(
        error.contains("no bindable parameter selectedMenuItem"),
        "{error}"
    );
    let before: Value = serde_json::from_str(&built.content).unwrap();
    roundtrip(&solution, &before);
}

#[test]
fn legacy_property_names_composer_migrates_are_renamed_or_kept_never_dropped() {
    let (_dir, solution) = element_solution();
    let mut before = source(
        &solution,
        "  parameters: {open: BOOLEAN}\n  slots:\n    center:\n      - widget:\n          id: ptcsdynamicpanel-20\n          type: ptcsdynamicpanel\n          props: {Collapsed: true}\n          children:\n            - column: {id: flexcontainer-21, children: [{label: {id: ptcslabel-23, text: Acme}}]}\n            - column: {id: flexcontainer-22, children: [{label: {id: ptcslabel-24, text: More}}]}\n      - widget: {id: navigationfunction-5, type: navigationfunction, props: {TargetMashup: Acme.Child_MU, TargetWindowType: modal-popup}}\n  bindings:\n    - {from: param.open, to: ptcsdynamicpanel-20.Collapsed}\n",
    );
    // Rewrite the page as an older Composer stored it.
    fn stale(widget: &mut Value) {
        let props = widget["Properties"].as_object_mut().unwrap();
        if props["Type"] == "ptcsdynamicpanel" {
            let value = props.remove("Collapsed").unwrap();
            props.insert("CollapseByDefault".into(), value);
        }
        if props["Type"] == "navigationfunction" {
            props.insert("TargetWindow".into(), json!("new"));
        }
        for child in widget["Widgets"].as_array_mut().into_iter().flatten() {
            stale(child);
        }
    }
    stale(&mut before["UI"]);
    for binding in before["DataBindings"].as_array_mut().unwrap() {
        for map in binding["PropertyMaps"].as_array_mut().unwrap() {
            if map["TargetProperty"] == "Collapsed" {
                map["TargetProperty"] = json!("CollapseByDefault");
            }
        }
    }
    for exact in [true, false] {
        let derived = if exact {
            derive::derive_exact("Acme.Copy_MU", &before.to_string())
        } else {
            derive::derive("Acme.Copy_MU", &before.to_string())
        }
        .unwrap();
        assert!(
            derived
                .notes
                .iter()
                .any(|n| n.contains("renamed property CollapseByDefault to Collapsed")),
            "{:?}",
            derived.notes
        );
        let built = compile(&solution, "Acme", &derived.spec)
            .unwrap_or_else(|e| panic!("exact={exact}: {e}\n{}", derived.spec));
        let after: Value = serde_json::from_str(&built.content).unwrap();
        let panel = widget(&after["UI"], "ptcsdynamicpanel-20");
        assert_eq!(panel["Properties"]["Collapsed"], true);
        assert!(panel["Properties"].get("CollapseByDefault").is_none());
        assert!(after["DataBindings"].to_string().contains("\"Collapsed\""));
        assert!(!after["DataBindings"]
            .to_string()
            .contains("CollapseByDefault"));
        // Both names set and the values differ in kind: the legacy one is kept as written.
        let navigation = widget(&after["UI"], "navigationfunction-5");
        assert_eq!(navigation["Properties"]["TargetWindow"], "new");
        assert_eq!(navigation["Properties"]["TargetWindowType"], "modal-popup");
    }
}

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
