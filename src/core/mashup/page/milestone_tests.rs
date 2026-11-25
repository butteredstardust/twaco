use super::super::derive;
use super::*;
use serde_json::json;

fn source(solution: &Solution, body: &str) -> Value {
    let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Source_MU\n  template: blank\n  layout: as-built\n{body}");
    let built = compile(solution, "Acme", &text).unwrap();
    serde_json::from_str(&built.content).unwrap()
}

fn roundtrip(solution: &Solution, before: &Value) -> Vec<Value> {
    [true, false]
        .into_iter()
        .map(|exact| {
            let derived = if exact {
                derive::derive_exact("Acme.Copy_MU", &before.to_string())
            } else {
                derive::derive("Acme.Copy_MU", &before.to_string())
            }
            .unwrap();
            assert!(
                !derived
                    .notes
                    .iter()
                    .any(|n| n.contains("left out") || n.contains("binding lost")),
                "{:?}",
                derived.notes
            );
            let built = compile(solution, "Acme", &derived.spec)
                .unwrap_or_else(|e| panic!("{e}\n{}", derived.spec));
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
            after
        })
        .collect()
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

#[test]
fn raw_container_subtrees_and_chart_bindings_survive_both_derives() {
    let (_dir, solution) = element_solution();
    for (kind, child, props) in [
        (
            "ptcsdynamicpanel",
            "ptcschartline",
            "XAxisField: Name, DataField1: Count",
        ),
        (
            "ptcsdynamicpanel",
            "ptcschartpie",
            "DataField: Name, ValueField: Count",
        ),
        ("fieldset", "ptcsgrid", ""),
    ] {
        let mut before = source(&solution, &format!("  data:\n    rows: {{service: Acme.Data.GetRows}}\n  slots:\n    center:\n      - widget:\n          id: {kind}-20\n          type: {kind}\n          children:\n            - column: {{id: flexcontainer-21, children: [{{widget: {{id: {child}-3, type: {child}, props: {{Data: {{bind: rows}}, {props}}}}}}}]}}\n            - column: {{id: flexcontainer-22, children: [{{label: {{id: ptcslabel-23, text: Acme}}}}]}}\n"));
        if kind == "fieldset" {
            let fieldset = &mut before["UI"]["Widgets"][0]["Widgets"][0];
            assert_eq!(fieldset["Properties"]["Type"], "fieldset");
            fieldset["Widgets"] = json!([fieldset["Widgets"][0]["Widgets"][0].clone()]);
        }
        let derived = derive::derive("Acme.Copy_MU", &before.to_string()).unwrap();
        assert!(
            derived.spec.contains(if child == "ptcsgrid" {
                "grid:"
            } else {
                "chart:"
            }),
            "{}",
            derived.spec
        );
        for after in roundtrip(&solution, &before) {
            let children = widget(&after["UI"], &format!("{kind}-20"))["Widgets"]
                .as_array()
                .unwrap();
            if kind == "fieldset" {
                assert_eq!(children.len(), 1);
                assert_eq!(children[0]["Properties"]["Id"], "ptcsgrid-3");
            } else {
                assert_eq!(children.len(), 2);
                assert_eq!(children[0]["Properties"]["Id"], "flexcontainer-21");
                assert_eq!(children[1]["Properties"]["Id"], "flexcontainer-22");
                assert!(!widget(&children[0], &format!("{child}-3")).is_null());
                assert!(!widget(&children[1], "ptcslabel-23").is_null());
            }
        }
    }
}

#[test]
fn static_template_and_shape_sources_keep_their_sections_and_bindings() {
    let (dir, solution) = element_solution();
    for (collection, kind) in [("ThingTemplates", "template"), ("ThingShapes", "shape")] {
        let folder = dir.path().join(format!("acme/{collection}"));
        std::fs::create_dir_all(&folder).unwrap();
        let tag = if kind == "template" {
            "ThingTemplate"
        } else {
            "ThingShape"
        };
        std::fs::write(folder.join("Acme.Provider.xml"), format!(r#"<Entities><{collection}><{tag} name="Acme.Provider" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetImplementingThingsWithData"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Rows_DS"/></ServiceDefinition></ServiceDefinitions></ThingShape></{tag}></{collection}></Entities>"#)).unwrap();
        let mut before = source(&solution, &format!("  data:\n    rows: {{service: Acme.Provider.GetImplementingThingsWithData, entity_type: {kind}, on: load}}\n  slots:\n    center:\n      - label: {{id: ptcslabel-5, props: {{LabelText: {{bind: rows.Name}}}}}}\n"));
        before["Data"][format!("{collection}_Acme.Provider")]
            .as_object_mut()
            .unwrap()
            .remove("RefreshInterval");
        for binding in before["DataBindings"].as_array_mut().into_iter().flatten() {
            binding["PropertyMaps"][0]["SourcePropertyType"] = json!("field");
        }
        for after in roundtrip(&solution, &before) {
            assert_eq!(
                after["Data"][format!("{collection}_Acme.Provider")]["EntityType"],
                collection
            );
        }
        let derived = derive::derive_exact("Acme.Copy_MU", &before.to_string()).unwrap();
        let built = compile(&solution, "Acme", &derived.spec).unwrap();
        let xml = solution.root.join("acme/Mashups/Acme.Copy_MU.xml");
        std::fs::write(&xml, built.entity).unwrap();
        let sources = authoring::list_sources(&solution, "Acme.Copy_MU").unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].kind, kind);
        std::fs::remove_file(xml).unwrap();
        std::fs::remove_file(folder.join("Acme.Provider.xml")).unwrap();
    }
}

#[test]
fn legacy_gauge_keeps_numeric_binding_metadata_in_both_derives() {
    let (_dir, solution) = element_solution();
    let mut before = source(&solution, "  parameters: {Max: STRING}\n  slots:\n    center:\n      - widget: {id: gauge-13, type: gauge, props: {MaxValue: 100}}\n      - label: {id: ptcslabel-14, text: Acme}\n  bindings:\n    - {from: param.Max, to: gauge-13.MaxValue}\n");
    // Composer records the legacy target's numeric type even when a string parameter feeds it.
    before["DataBindings"][0]["PropertyMaps"][0]["TargetPropertyBaseType"] = json!("NUMBER");
    before["DataBindings"][0]
        .as_object_mut()
        .unwrap()
        .remove("SourceDetails");
    before["DataBindings"][0]["PropertyMaps"][0]["SourcePropertyType"] = json!("property");
    for after in roundtrip(&solution, &before) {
        assert_eq!(
            after["DataBindings"][0]["PropertyMaps"][0]["TargetPropertyBaseType"],
            "NUMBER"
        );
    }
}

#[test]
fn reset_events_survive_both_derives() {
    let (_dir, solution) = element_solution();
    let before = source(&solution, "  slots:\n    center:\n      - column: {id: flexcontainer-30, children: [{button: {id: ptcsbutton-10, text: Reset}}, {text_field: {id: ptcstextfield-13}}, {date_picker: {id: ptcsdatepicker-14}}, {widget: {id: ptcsdropdown-15, type: ptcsdropdown}}]}\n  events:\n    - {on: ptcsbutton-10.Clicked, invoke: mashup.ResetInputsToDefaultValue}\n    - {on: ptcsbutton-10.Clicked, invoke: ptcstextfield-13.ResetToDefaultValue}\n    - {on: ptcsbutton-10.Clicked, invoke: ptcsdatepicker-14.ResetToDefaultValue}\n    - {on: ptcsbutton-10.Clicked, invoke: ptcsdropdown-15.ResetToDefaultValue}\n");
    for after in roundtrip(&solution, &before) {
        assert_eq!(after["Events"].as_array().unwrap().len(), 4);
    }
}

#[test]
fn tabs_use_item_labels_instead_of_repeated_placeholder_roles() {
    let (_dir, solution) = element_solution();
    let mut before = source(&solution, "  slots:\n    center:\n      - tabs: {id: ptcstabset-37, tabs: [{label: First, content: [{label: {id: ptcslabel-40, text: Acme}}]}, {label: Second, content: [{label: {id: ptcslabel-41, text: Acme}}]}, {label: Third, content: [{label: {id: ptcslabel-42, text: Acme}}]}, {label: Fourth, content: [{label: {id: ptcslabel-43, text: Acme}}]}]}\n");
    fn placeholders(w: &mut Value) {
        if w["Properties"]["Type"] == "ptcstabset" {
            w["Properties"]["Items"] = json!(["First", "Second", "Third", "Fourth"].into_iter().enumerate().map(|(n, name)| json!({"name": name, "value": format!("TabValue{}", n + 1), "visible": true, "disabled": false})).collect::<Vec<_>>());
            for n in 1..=4 {
                w["Properties"][format!("Tab{n}Name")] = json!("[[TabName]]");
            }
        }
        for child in w["Widgets"].as_array_mut().into_iter().flatten() {
            placeholders(child);
        }
    }
    placeholders(&mut before["UI"]);
    let tab_ids = |value: &Value| {
        widget(&value["UI"], "ptcstabset-37")["Widgets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tab| tab["Properties"]["Id"].clone())
            .collect::<Vec<_>>()
    };
    let expected = tab_ids(&before);
    for after in roundtrip(&solution, &before) {
        assert_eq!(tab_ids(&after), expected);
    }
}

#[test]
fn distinct_explicit_ids_can_share_a_numeric_suffix() {
    let (_dir, solution) = element_solution();
    let before = source(&solution, "  slots:\n    center:\n      - column: {id: flexcontainer-48, children: [{checkbox: {id: ptcscheckbox-48}}]}\n");
    for after in roundtrip(&solution, &before) {
        assert!(!widget(&after["UI"], "flexcontainer-48").is_null());
        assert!(!widget(&after["UI"], "ptcscheckbox-48").is_null());
        assert_ne!(
            widget(&after["UI"], "flexcontainer-48")["Properties"]["DisplayName"],
            widget(&after["UI"], "ptcscheckbox-48")["Properties"]["DisplayName"]
        );
    }
}

#[test]
fn unsupported_source_warnings_name_each_lost_binding() {
    let (_dir, solution) = element_solution();
    let mut before = source(
        &solution,
        "  slots:\n    center:\n      - label: {id: ptcslabel-5, text: Acme}\n",
    );
    before["Data"]["Resources_Acme.Provider"] = json!({"EntityType": "Resources", "EntityName": "Acme.Provider", "Services": [{"Name": "GetRows", "Target": "GetRows"}]});
    before["DataBindings"] = json!([{"SourceArea": "Data", "SourceSection": "Resources_Acme.Provider", "SourceId": "GetRows", "TargetArea": "UI", "TargetId": "ptcslabel-5", "PropertyMaps": [{"SourceProperty": "Name", "TargetProperty": "LabelText"}, {"SourceProperty": "Enabled", "TargetProperty": "Visible"}]}]);
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
                .any(|n| n.contains("GetRows.Name -> ptcslabel-5.LabelText")),
            "{:?}",
            derived.notes
        );
        assert!(
            derived
                .notes
                .iter()
                .any(|n| n.contains("GetRows.Enabled -> ptcslabel-5.Visible")),
            "{:?}",
            derived.notes
        );
    }
}

#[test]
fn source_kinds_and_uncatalogued_binding_types_are_checked() {
    let (_dir, solution) = element_solution();
    for (body, expected) in [
        ("  slots: {center: [{label: {id: title, binding_types: {LabelText: NUMBER}}}]}\n", "binding_types is only for widgets without a catalogue contract"),
        ("  slots: {center: [{widget: {id: gauge-13, type: gauge, binding_types: {MaxValue: UNKNOWN}}}]}\n", "known base type"),
        ("  data: {rows: {service: Acme.Data.GetRows, entity_type: template}}\n", "does not match its collection"),
        ("  data: {user: {session: Session, entity_type: template}}\n", "entity_type needs a service"),
    ] {
        let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Invalid_MU\n  template: blank\n{body}");
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }
}
