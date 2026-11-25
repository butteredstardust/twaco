use super::super::derive;
use super::*;

fn manager_solution() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = element_solution();
    std::fs::create_dir_all(solution.root.join("acme/ThingShapes")).unwrap();
    std::fs::write(solution.root.join("acme/ThingShapes/Acme.Manager_TS.xml"), r#"<Entities><ThingShapes><ThingShape name="Acme.Manager_TS" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetSearch"><ParameterDefinitions><FieldDefinition name="flag" baseType="BOOLEAN"/></ParameterDefinitions><ResultType baseType="INFOTABLE"/></ServiceDefinition></ServiceDefinitions></ThingShape></ThingShape></ThingShapes></Entities>"#).unwrap();
    let solution = Solution::load(&solution.root.join("twaco.toml")).unwrap();
    (dir, solution)
}

fn first<'a>(widget: &'a Value, kind: &str) -> Option<&'a Value> {
    if widget["Properties"]["Type"] == kind {
        return Some(widget);
    }
    widget["Widgets"]
        .as_array()?
        .iter()
        .find_map(|w| first(w, kind))
}

#[test]
fn a_gadget_page_builds_a_gadget_with_its_widgets_in_the_root() {
    let (_dir, solution) = manager_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Gauge_MU\n  kind: gadget\n  gadget_size: 2x1\n  template: blank\n  layout: as-built\n  slots:\n    center:\n      - label: {id: out, text: Hello}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let entity = String::from_utf8(built.entity).unwrap();
    assert!(entity.contains("aspect.mashupType=\"gadget\""), "{entity}");
    assert!(entity.contains("columns=\"2.0\""), "{entity}");
    let content: Value = serde_json::from_str(&built.content).unwrap();
    assert_eq!(
        content["UI"]["Widgets"][0]["Properties"]["Type"],
        "ptcslabel"
    );
    assert!(
        first(&content["UI"], "flexcontainer").is_none(),
        "{content}"
    );
}

#[test]
fn a_call_to_a_service_its_entity_dropped_is_kept_as_found_on_an_as_built_page() {
    let (_dir, solution) = manager_solution();
    let spec = |layout: &str| {
        format!("version: 1\nrules: v1\npage:\n  name: Acme.Old_MU\n  template: blank\n{layout}  parameters: {{Manager: THINGNAME, name: STRING}}\n  data:\n    export:\n      service: Acme.Manager_TS.GetExport\n      entity: {{bind: param.Manager}}\n      params:\n        name: {{bind: param.name}}\n      unknown_service: {{name: STRING}}\n  slots:\n    center:\n      - label: {{id: out, text: x}}\n")
    };
    let built = compile(&solution, "Acme", &spec("  layout: as-built\n")).unwrap();
    let content: Value = serde_json::from_str(&built.content).unwrap();
    let section = &content["Data"]["DynamicThingShapes_Acme.Manager_TS"];
    assert_eq!(section["Services"][0]["Target"], "GetExport", "{content}");
    let binding = content["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["TargetId"] == "export" && b["PropertyMaps"][0]["TargetProperty"] == "name")
        .unwrap_or_else(|| panic!("{content}"));
    assert_eq!(
        binding["PropertyMaps"][0]["TargetPropertyBaseType"],
        "STRING"
    );
    // An authored page names a service its entity declares.
    let error = compile(&solution, "Acme", &spec(""))
        .unwrap_err()
        .to_string();
    assert!(error.contains("layout: as-built"), "{error}");
}

#[test]
fn a_binding_keeps_the_type_it_stored_when_the_service_declares_another() {
    let (_dir, solution) = manager_solution();
    let spec = |types: &str| {
        format!("version: 1\nrules: v1\npage:\n  name: Acme.Typed_MU\n  template: blank\n  parameters: {{Manager: THINGNAME, on: BOOLEAN}}\n  data:\n    search:\n      service: Acme.Manager_TS.GetSearch\n      entity: {{bind: param.Manager}}\n      params:\n        flag: {{bind: param.on}}\n{types}  slots:\n    center:\n      - label: {{id: out, text: x}}\n")
    };
    let stored = |text: &str| {
        let built = compile(&solution, "Acme", text).unwrap();
        let content: Value = serde_json::from_str(&built.content).unwrap();
        let binding = content["DataBindings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["TargetId"] == "search" && b["PropertyMaps"][0]["TargetProperty"] == "flag")
            .cloned()
            .unwrap_or_else(|| panic!("{content}"));
        binding["PropertyMaps"][0]["TargetPropertyBaseType"].clone()
    };
    assert_eq!(stored(&spec("")), "BOOLEAN");
    assert_eq!(
        stored(&spec("      param_types: {flag: STRING}\n")),
        "STRING"
    );
}

#[test]
fn a_shape_element_writes_its_properties_and_derives_back() {
    let (_dir, solution) = manager_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Dots_MU\n  template: blank\n  slots:\n    center:\n      - shape: {id: dot, form: roundrect, fill: Acme_Blue, unfilled: Acme_Grey, radius: 12, dynamic: true, min: 0, max: 50, orientation: horizontal, props: {Width: 40, Height: 8}}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let content: Value = serde_json::from_str(&built.content).unwrap();
    let shape = first(&content["UI"], "shape").unwrap_or_else(|| panic!("{content}"));
    let p = &shape["Properties"];
    assert_eq!(p["ShapeType"], "roundrect");
    assert_eq!(p["ShapeStyle"], "Acme_Blue");
    assert_eq!(p["UnfilledBackgroundStyle"], "Acme_Grey");
    assert_eq!(p["RectangleCornerRadius"], 12);
    assert_eq!(p["EnableDynamicFill"], true);
    assert_eq!(p["DynamicRangeMaximum"], 50);
    assert_eq!(p["DynamicFillOrientation"], "horizontal");
    assert_eq!(p["Width"], 40);
    let derived = derive::derive("Acme.Dots_MU", &built.content).unwrap();
    assert!(derived.spec.contains("shape:"), "{}", derived.spec);
    assert!(derived.spec.contains("form: roundrect"), "{}", derived.spec);
    let rebuilt = compile(&solution, "Acme", &derived.spec)
        .unwrap_or_else(|e| panic!("{e}\n{}", derived.spec));
    let again: Value = serde_json::from_str(&rebuilt.content).unwrap();
    assert_eq!(
        first(&again["UI"], "shape").unwrap()["Properties"],
        shape["Properties"]
    );
    let error = compile(
        &solution,
        "Acme",
        &spec.replace("form: roundrect", "form: blob"),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("form must be"), "{error}");
}
