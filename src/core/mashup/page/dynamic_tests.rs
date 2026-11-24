use super::super::derive;
use super::*;

const SPEC: &str = "version: 1\nrules: v1\npage:\n  name: Acme.Dynamic_MU\n  template: blank\n  parameters: {Manager: THINGNAME}\n  data:\n    menu: {service: Acme.Manager_TS.GetMenu, entity: {bind: param.Manager}, on: load}\n  slots:\n    center:\n      - list: {id: items, data: menu, display: Name}\n";

fn dynamic_solution() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = element_solution();
    std::fs::create_dir_all(solution.root.join("acme/ThingShapes")).unwrap();
    std::fs::write(solution.root.join("acme/ThingShapes/Acme.Manager_TS.xml"), r#"<Entities><ThingShapes><ThingShape name="Acme.Manager_TS" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetMenu"><ResultType baseType="INFOTABLE"/></ServiceDefinition></ServiceDefinitions></ThingShape></ThingShape></ThingShapes></Entities>"#).unwrap();
    let solution = Solution::load(&solution.root.join("twaco.toml")).unwrap();
    (dir, solution)
}

/// Sections, calls, binding endpoints and event ends, without ids.
fn wiring(content: &str) -> Value {
    let value: Value = serde_json::from_str(content).unwrap();
    let mut data: Vec<Value> = value["Data"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, section)| {
            let calls: Vec<_> = section["Services"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| format!("{}:{}", c["Name"], c["Target"]))
                .collect();
            serde_json::json!([key, section["EntityType"], section["EntityName"], calls])
        })
        .collect();
    data.sort_by_key(Value::to_string);
    let mut bindings: Vec<String> = value["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| {
            format!(
                "{}.{}.{} -> {}.{}.{}",
                b["SourceArea"],
                b["SourceId"],
                b["PropertyMaps"][0]["SourceProperty"],
                b["TargetArea"],
                b["TargetId"],
                b["PropertyMaps"][0]["TargetProperty"],
            )
        })
        .collect();
    bindings.sort();
    let mut events: Vec<String> = value["Events"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            format!(
                "{}.{}.{} -> {}.{}",
                e["EventTriggerArea"],
                e["EventTriggerId"],
                e["EventTriggerEvent"],
                e["EventHandlerId"],
                e["EventHandlerService"]
            )
        })
        .collect();
    events.sort();
    serde_json::json!({"data": data, "bindings": bindings, "events": events})
}

#[test]
fn a_dynamic_source_bound_from_a_parameter_derives_and_rebuilds_with_its_wiring() {
    let (_dir, solution) = dynamic_solution();
    let built = compile(&solution, "Acme", SPEC).unwrap();
    let before = wiring(&built.content);
    let section = &before["data"][0];
    assert_eq!(section[0], "DynamicThingShapes_Acme.Manager_TS", "{before}");
    assert!(
        before["bindings"].as_array().unwrap().iter().any(|b| b
            .as_str()
            .unwrap()
            .contains("\"Manager\"")
            && b.as_str().unwrap().contains("EntityName")),
        "{before}"
    );
    let derived = derive::derive("Acme.Rebuilt_MU", &built.content).unwrap();
    assert!(
        derived.spec.contains("entity: {bind: param.Manager}"),
        "{}",
        derived.spec
    );
    assert!(
        derived.spec.contains("Manager: THINGNAME"),
        "{}",
        derived.spec
    );
    let rebuilt = compile(&solution, "Acme", &derived.spec)
        .unwrap_or_else(|e| panic!("{e}\n{}", derived.spec));
    assert_eq!(before, wiring(&rebuilt.content), "{}", derived.spec);
}

const OTHER: &str = "version: 1\nrules: v1\npage:\n  name: Acme.Held_MU\n  template: blank\n  slots:\n    center:\n      - button: {id: go, text: Go}\n      - menu_bar: {id: menu}\n  events:\n    - {on: go.Clicked, navigate: {mashup: Acme.Child_MU, mode: popup}}\n";

/// A mashup carrying what the real ones did: a menu width that is not a number, an event on a
/// service the catalogue does not list, a reference to a navigation function's event, and two
/// widgets the catalogue does not know.
fn awkward(solution: &Solution) -> String {
    let built = compile(solution, "Acme", OTHER).unwrap();
    let mut value: Value = serde_json::from_str(&built.content).unwrap();
    fn find<'a>(widget: &'a mut Value, kind: &str) -> Option<&'a mut Value> {
        if widget["Properties"]["Type"] == kind {
            return Some(widget);
        }
        widget["Widgets"]
            .as_array_mut()?
            .iter_mut()
            .find_map(|w| find(w, kind))
    }
    let menu = find(&mut value["UI"], "ptcsmenubar").unwrap();
    menu["Properties"]["MenuMaxWidth"] = serde_json::json!("wide");
    let menu_id = menu["Properties"]["Id"].as_str().unwrap().to_string();
    let go = find(&mut value["UI"], "ptcsbutton").unwrap()["Properties"]["Id"]
        .as_str()
        .unwrap()
        .to_string();
    let nav = find(&mut value["UI"], "navigationfunction").unwrap()["Properties"]["Id"]
        .as_str()
        .unwrap()
        .to_string();
    let event = |trigger: &str, event: &str, handler: &str, service: &str| {
        serde_json::json!({
            "EventTriggerArea": "UI", "EventTriggerId": trigger, "EventTriggerEvent": event,
            "EventTriggerSection": "", "EventHandlerArea": "UI", "EventHandlerId": handler,
            "EventHandlerService": service, "Id": format!("{trigger}-{event}-{service}")
        })
    };
    let events = value["Events"].as_array_mut().unwrap();
    events.push(event(&go, "Clicked", &menu_id, "ResetToDefaultValue"));
    events.push(event(
        &nav,
        "refreshRequested",
        &menu_id,
        "ResetToDefaultValue",
    ));
    let widgets = value["UI"]["Widgets"].as_array_mut().unwrap();
    for (kind, number) in [("gauge", 90), ("logoutfunction", 91)] {
        widgets.push(serde_json::json!({"Properties": {
            "Id": format!("{kind}-{number}"), "DisplayName": format!("{kind}-{number}"),
            "Type": kind, "Width": 200, "Height": 100
        }, "Widgets": []}));
    }
    serde_json::to_string(&value).unwrap()
}

#[test]
fn derive_never_emits_what_its_own_build_refuses() {
    let (_dir, solution) = dynamic_solution();
    let content = awkward(&solution);
    let mut hand: Value = serde_json::from_str(&content).unwrap();
    hand.as_object_mut().unwrap().remove("twacoPageRules");
    let hand = serde_json::to_string(&hand).unwrap();
    for exact in [false, true] {
        let derived = if exact {
            derive::derive_exact("Acme.Copy_MU", &hand)
        } else {
            derive::derive("Acme.Copy_MU", &content)
        }
        .unwrap();
        let rebuilt = compile(&solution, "Acme", &derived.spec)
            .unwrap_or_else(|e| panic!("exact {exact}: {e}\n{}", derived.spec));
        assert!(
            !derived.spec.contains("refreshRequested"),
            "{}",
            derived.spec
        );
        assert!(
            !derived.spec.contains("ResetToDefaultValue"),
            "{}",
            derived.spec
        );
        assert!(
            derived
                .notes
                .iter()
                .any(|n| n.contains("ResetToDefaultValue")),
            "{:?}",
            derived.notes
        );
        assert!(
            derived.notes.iter().any(|n| n.contains("MenuMaxWidth")),
            "{:?}",
            derived.notes
        );
        let gauges = rebuilt.content.matches("\"Type\":\"gauge\"").count()
            + rebuilt.content.matches("\"Type\": \"gauge\"").count();
        assert_eq!(
            gauges,
            usize::from(exact),
            "exact {exact}\n{}",
            derived.spec
        );
    }
}

#[test]
fn an_as_built_derive_carries_root_style_expression_flags_and_positions() {
    let (_dir, solution) = dynamic_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Kept_MU\n  template: blank\n  data:\n    total: {expression: '1 + 1', output: NUMBER}\n  slots:\n    center:\n      - column: {id: box, children: [{menu_bar: {id: menu}}]}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let mut value: Value = serde_json::from_str(&built.content).unwrap();
    value.as_object_mut().unwrap().remove("twacoPageRules");
    value["UI"]["Properties"]["Style"] = serde_json::json!("DefaultContainerStyle");
    fn each(widget: &mut Value, f: &mut impl FnMut(&mut Value)) {
        f(widget);
        for child in widget["Widgets"].as_array_mut().into_iter().flatten() {
            each(child, f);
        }
    }
    each(
        &mut value["UI"],
        &mut |w| match w["Properties"]["Type"].as_str() {
            Some("expression2") => {
                w["Properties"]["DefaultNumberEvaluation"] = serde_json::json!(false);
                w["Properties"]["Description"] = serde_json::json!("Adds");
                w["Properties"]["DisplayName"] = serde_json::json!("Total");
            }
            Some("ptcsmenubar") => {
                w["Properties"]["Left"] = serde_json::json!(305.9);
                w["Properties"]["Top"] = serde_json::json!(196);
            }
            Some("flexcontainer") => {
                w["Properties"]["ResponsiveLayout"] = serde_json::json!(false);
            }
            _ => {}
        },
    );
    let derived =
        derive::derive_exact("Acme.Kept_MU", &serde_json::to_string(&value).unwrap()).unwrap();
    for needle in [
        "Style: DefaultContainerStyle",
        "DefaultNumberEvaluation: false",
        "Description: Adds",
        "DisplayName: Total",
        "Left: 305.9",
        "ResponsiveLayout: false",
    ] {
        assert!(derived.spec.contains(needle), "{needle}\n{}", derived.spec);
    }
    let rebuilt = compile(&solution, "Acme", &derived.spec)
        .unwrap_or_else(|e| panic!("{e}\n{}", derived.spec));
    let again: Value = serde_json::from_str(&rebuilt.content).unwrap();
    assert_eq!(again["UI"]["Properties"]["Style"], "DefaultContainerStyle");
    let mut seen = vec![];
    each(&mut again.clone()["UI"], &mut |w| {
        seen.push((
            w["Properties"]["Type"].as_str().unwrap_or("").to_string(),
            w["Properties"].clone(),
        ))
    });
    let menu = seen.iter().find(|(t, _)| t == "ptcsmenubar").unwrap();
    assert_eq!(menu.1["Left"], serde_json::json!(305.9));
    let expression = seen.iter().find(|(t, _)| t == "expression2").unwrap();
    assert_eq!(expression.1["DefaultNumberEvaluation"], false);
    assert_eq!(expression.1["Description"], "Adds");
}
