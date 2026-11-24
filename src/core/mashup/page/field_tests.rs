//! Adopting what a real solution has: older binding files, sparse function widgets, sibling
//! calls of one ThingShape, wiring to widgets the catalogue does not know.

use super::super::extract as extraction;
use super::adopt::{apply, current, handmade, write_spec, HAND_SPEC};
use super::*;
use crate::core::mashup::Assets;

/// The hand-made page with a text field feeding a label, so a widget-to-widget binding exists.
fn spec_with_ui_binding() -> String {
    HAND_SPEC.replace(
        "            - embed:",
        "            - text_field: {id: filter}
            - label: {id: out2, props: {LabelText: {bind: filter.Text}}}
            - embed:",
    )
}

/// Change a hand-made mashup's content in place, as an older Composer would have written it.
fn rewrite(solution: &Solution, name: &str, change: impl Fn(&mut Value)) {
    let path = solution.root.join(format!("acme/Mashups/{name}.xml"));
    let loaded = authoring::load_existing(solution, name).unwrap();
    let mut value: Value = serde_json::from_str(&loaded.assets.content).unwrap();
    change(&mut value);
    let assets = Assets {
        content: mashup::to_sidecar(&value),
        css: String::new(),
    };
    let entity = mashup::sync(&loaded.entity_before, &assets).unwrap().0;
    std::fs::write(path, entity).unwrap();
}

/// Derive a spec from the mashup and build it: the plan has no conflict and writes nothing.
fn adopts_cleanly(solution: &Solution, name: &str) -> String {
    let derived = extraction::extract(solution, name).unwrap();
    write_spec(solution, name, &derived.spec);
    let before = current(solution, name);
    let report = apply(solution, name);
    assert_eq!(report["conflicts"], serde_json::json!([]), "{report}");
    assert_eq!(current(solution, name), before);
    derived.spec
}

#[test]
fn bindings_in_the_older_format_are_claimed_and_their_bytes_kept() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, &spec_with_ui_binding(), "Acme.Hand_MU");
    rewrite(&solution, &name, |value| {
        for binding in value["DataBindings"].as_array_mut().unwrap() {
            if binding["SourceArea"] != "UI" {
                continue;
            }
            binding.as_object_mut().unwrap().remove("SourceDetails");
            for map in binding["PropertyMaps"].as_array_mut().unwrap() {
                map["SourcePropertyType"] = "property".into();
                map["TargetPropertyType"] = "Property".into();
            }
        }
    });
    let old: Vec<_> = current(&solution, &name)["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["SourceArea"] == "UI")
        .cloned()
        .collect();
    assert_eq!(old.len(), 1);
    assert!(old[0].get("SourceDetails").is_none());
    adopts_cleanly(&solution, &name);
}

#[test]
fn defaults_the_source_lacks_are_not_added_on_an_as_built_page() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, HAND_SPEC, "Acme.Hand_MU");
    rewrite(&solution, &name, |value| {
        fn strip(widget: &mut Value) {
            if let Some(properties) = widget["Properties"].as_object_mut() {
                for key in ["CustomClass", "TooltipField"] {
                    if properties.get(key).is_some_and(|v| v == "") {
                        properties.remove(key);
                    }
                }
            }
            for child in widget["Widgets"].as_array_mut().into_iter().flatten() {
                strip(child);
            }
        }
        strip(&mut value["UI"]);
    });
    adopts_cleanly(&solution, &name);
}

#[test]
fn an_events_router_with_empty_inputs_derives_what_build_accepts() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, HAND_SPEC, "Acme.Hand_MU");
    rewrite(&solution, &name, |value| {
        value["UI"]["Widgets"].as_array_mut().unwrap().push(serde_json::json!({
            "Properties": {"Id": "eventsrouter-90", "Type": "eventsrouter", "DisplayName": "eventsrouter-90",
                "NumberOfInputs": 2, "DataType": "STRING", "Input1": {}, "Input2": {}},
            "Widgets": []
        }));
    });
    let derived = extraction::extract(&solution, &name).unwrap();
    assert!(derived.spec.contains("events_router"), "{}", derived.spec);
    assert!(!derived.spec.contains("{}"), "{}", derived.spec);
    adopts_cleanly(&solution, &name);
}

#[test]
fn calls_of_one_shape_get_the_entity_even_when_the_call_without_one_sorts_first() {
    let (_dir, solution) = element_solution();
    std::fs::create_dir_all(solution.root.join("acme/ThingShapes")).unwrap();
    std::fs::write(solution.root.join("acme/ThingShapes/Acme.Manager_TS.xml"), r#"<Entities><ThingShapes><ThingShape name="Acme.Manager_TS" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetMenu"><ResultType baseType="INFOTABLE"/></ServiceDefinition><ServiceDefinition name="GetTotals"><ResultType baseType="INFOTABLE"/></ServiceDefinition></ServiceDefinitions></ThingShape></ThingShape></ThingShapes></Entities>"#).unwrap();
    let solution = Solution::load(&solution.root.join("twaco.toml")).unwrap();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Shared_MU\n  template: blank\n  parameters: {Manager: THINGNAME}\n  data:\n    first: {service: Acme.Manager_TS.GetTotals, on: load}\n    second: {service: Acme.Manager_TS.GetMenu, entity: {bind: param.Manager}, on: load}\n  slots:\n    center:\n      - list: {id: items, data: second, display: Name}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let content: Value = serde_json::from_str(&built.content).unwrap();
    assert_eq!(
        content["Data"]["DynamicThingShapes_Acme.Manager_TS"]["Services"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn wiring_to_a_widget_the_catalogue_does_not_know_is_carried_on_an_as_built_page() {
    let (_dir, solution) = element_solution();
    let spec = HAND_SPEC
        .replace(
            "            - embed:",
            "            - widget: {id: gauge-90, type: gauge, props: {Data: 0}}
            - embed:",
        )
        .replace(
            "  events:",
            "  bindings:
    - {from: rows.Count, to: gauge-90.Data}
  events:",
        );
    let name = handmade(&solution, &spec, "Acme.Hand_MU");
    assert!(current(&solution, &name)["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["TargetId"] == "gauge-90"));
    adopts_cleanly(&solution, &name);
}

#[test]
fn a_binding_stated_twice_and_a_formatter_threshold_adopt_as_found() {
    let (_dir, solution) = element_solution();
    let spec = HAND_SPEC
        .replace(
            "            - embed:",
            "            - widget: {id: ptcskpidial-91, type: ptcskpidial, props: {TargetFormat_thresholdMin: 50}}\n            - embed:",
        )
        .replace(
            "  events:",
            "  bindings:\n    - {from: rows.Count, to: ptcskpidial-91.TargetFormat_thresholdMin}\n    - {from: rows.Count, to: ptcskpidial-91.TargetFormat_thresholdMin}\n  events:",
        );
    let name = handmade(&solution, &spec, "Acme.Hand_MU");
    let bound = current(&solution, &name)["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["TargetId"] == "ptcskpidial-91")
        .count();
    assert_eq!(bound, 1);
    adopts_cleanly(&solution, &name);
}

#[test]
fn a_root_container_with_another_id_than_the_templates_is_claimed_without_a_wrapper() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, HAND_SPEC, "Acme.Hand_MU");
    rewrite(&solution, &name, |value| {
        value["UI"]["Widgets"][0]["Properties"]["Id"] = "flexcontainer-1".into();
    });
    let spec = adopts_cleanly(&solution, &name);
    assert!(
        spec.contains("root:\n    id: flexcontainer-1") || spec.contains("id: flexcontainer-1"),
        "{spec}"
    );
    let content = current(&solution, &name);
    assert_eq!(
        content["UI"]["Widgets"][0]["Properties"]["Id"],
        "flexcontainer-1"
    );
    assert_eq!(content["UI"]["Widgets"].as_array().unwrap().len(), 1);
}
