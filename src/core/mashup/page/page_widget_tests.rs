//! Derive carries what a page has (alignment, pane heights, state formatting, root bindings) and
//! `widget:` nodes are checked against the widget's contract.
use super::super::derive;
use super::*;
use serde_json::json;

fn source(solution: &Solution, body: &str) -> Value {
    let text = format!("version: 1\nrules: v1\npage:\n  name: Acme.Source_MU\n  template: blank\n  layout: as-built\n{body}");
    let built = compile(solution, "Acme", &text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    serde_json::from_str(&built.content).unwrap()
}

fn spec_error(solution: &Solution, body: &str) -> String {
    let text =
        format!("version: 1\nrules: v1\npage:\n  name: Acme.Source_MU\n  template: blank\n{body}");
    compile(solution, "Acme", &text).unwrap_err().to_string()
}

fn derived(before: &Value, exact: bool) -> derive::Derived {
    if exact {
        derive::derive_exact("Acme.Copy_MU", &before.to_string())
    } else {
        derive::derive("Acme.Copy_MU", &before.to_string())
    }
    .unwrap()
}

/// Derive through both paths, build a fresh copy and require no layout or executable difference.
fn round_trips(solution: &Solution, before: &Value) -> Vec<(String, Value)> {
    [true, false]
        .into_iter()
        .map(|exact| {
            let derived = derived(before, exact);
            let built = compile(solution, "Acme", &derived.spec)
                .unwrap_or_else(|e| panic!("exact={exact}: {e}\n{}", derived.spec));
            let after: Value = serde_json::from_str(&built.content).unwrap();
            let tiers = if exact {
                vec![mashup::diff::Tier::Executable, mashup::diff::Tier::Layout]
            } else {
                vec![mashup::diff::Tier::Executable]
            };
            for tier in tiers {
                let diff = mashup::diff::compare(
                    "Acme.Source_MU",
                    "Acme.Copy_MU",
                    before,
                    &after,
                    None,
                    None,
                    tier,
                );
                assert!(diff.equal, "exact={exact}: {diff:?}\n{}", derived.spec);
            }
            (derived.spec, after)
        })
        .collect()
}

fn find<'a>(widget: &'a Value, id: &str) -> Option<&'a Value> {
    if widget["Properties"]["Id"] == id {
        return Some(widget);
    }
    widget["Widgets"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|child| find(child, id))
}

#[test]
fn a_container_that_aligns_to_the_start_keeps_it_through_both_derive_paths() {
    let (_dir, solution) = element_solution();
    let before = source(
        &solution,
        "  slots:
    center:
      - row:
          id: flexcontainer-5
          props: {align-items: flex-start, flex-wrap: wrap}
          children:
            - column:
                id: flexcontainer-6
                props: {align-items: flex-end}
                children:
                  - label: {id: ptcslabel-7, text: Acme}
            - label: {id: ptcslabel-8, text: Acme}
",
    );
    assert_eq!(
        find(&before["UI"], "flexcontainer-5").unwrap()["Properties"]["align-items"],
        "flex-start"
    );
    for (spec, after) in round_trips(&solution, &before) {
        assert!(spec.contains("align-items: flex-start"), "{spec}");
        assert_eq!(
            find(&after["UI"], "flexcontainer-5").unwrap()["Properties"]["flex-wrap"],
            "wrap",
            "{spec}"
        );
    }
}

fn pane<'a>(content: &'a Value, id: &str) -> &'a Value {
    let tabs = find(&content["UI"], id).unwrap();
    &tabs["Widgets"][0]
}

#[test]
fn a_tab_pane_with_a_chart_keeps_its_own_minimum_height() {
    let (_dir, solution) = element_solution();
    let before = source(
        &solution,
        "  data:
    rows: {service: Acme.Data.GetRows, on: load}
  slots:
    center:
      - tabs:
          id: ptcstabset-9
          tabs:
            - label: Trend
              props: {flex-min-height: \"\"}
              content:
                - chart: {id: ptcschartline-12, kind: line, data: rows, x: Name}
            - label: Other
              content:
                - label: {id: ptcslabel-14, text: Acme}
",
    );
    assert_eq!(
        pane(&before, "ptcstabset-9")["Properties"]["flex-min-height"],
        ""
    );
    // The as-built path: the other writes the v1 template's tab body, a different structure.
    for (spec, after) in round_trips(&solution, &before).into_iter().take(1) {
        assert!(spec.contains("flex-min-height"), "{spec}");
        assert_eq!(
            pane(&after, "ptcstabset-9")["Properties"]["flex-min-height"],
            "",
            "{spec}"
        );
    }
}

#[test]
fn state_formatting_on_a_widget_is_carried_verbatim_without_a_note() {
    let (_dir, solution) = element_solution();
    let formatter = json!({
        "FieldName": "Status", "StateDefinition": "Acme.Status_ST", "StateDefinitionType": "string",
        "StateFormats": [{"comparator": "==", "defaultValue": "Done", "stateName": "Done", "value": "Done",
            "state": {"backgroundColor": "rgba(1,2,3,1)", "lineStyle": "solid", "lineThickness": 1}}]
    });
    let before = source(
        &solution,
        &format!(
            "  slots:\n    center:\n      - widget: {{id: shape-5, type: shape, props: {{ShapeFormatter: {formatter}}}}}\n"
        ),
    );
    assert_eq!(
        find(&before["UI"], "shape-5").unwrap()["Properties"]["ShapeFormatter"],
        formatter
    );
    for exact in [true, false] {
        let derived = derived(&before, exact);
        assert!(
            !derived.notes.iter().any(|n| n.contains("state formatting")),
            "{:?}",
            derived.notes
        );
        assert!(derived.spec.contains("StateFormats"), "{}", derived.spec);
    }
    for (spec, after) in round_trips(&solution, &before) {
        assert_eq!(
            find(&after["UI"], "shape-5").unwrap()["Properties"]["ShapeFormatter"],
            formatter,
            "{spec}"
        );
    }
}

#[test]
fn a_binding_to_a_mashup_root_property_is_written_as_page_dot_property() {
    let (_dir, solution) = element_solution();
    let mut before = source(
        &solution,
        "  parameters: {Heading: STRING}\n  bindings:\n    - {from: param.Heading, to: page.Title}\n  slots:\n    center:\n      - label: {id: ptcslabel-5, text: Acme}\n",
    );
    // Composer files a root binding's target under the `Mashup` area.
    let bindings = before["DataBindings"].as_array_mut().unwrap();
    assert_eq!(bindings.len(), 1, "{bindings:?}");
    bindings[0]["TargetArea"] = json!("Mashup");
    for exact in [true, false] {
        let derived = derived(&before, exact);
        assert!(derived.spec.contains("page.Title"), "{}", derived.spec);
        assert!(
            !derived.notes.iter().any(|n| n.contains("left out")),
            "{:?}",
            derived.notes
        );
    }
}

const FIELD: &str = "      - widget:
          id: field
          type: ptcstextfield
          props: {Text: Acme}
";

#[test]
fn a_widget_node_takes_its_events_and_builds_the_handler() {
    let (_dir, solution) = element_solution();
    let before = source(
        &solution,
        &format!(
            "  data:\n    rows: {{service: Acme.Data.GetRows}}\n  slots:\n    center:\n{FIELD}          events:\n            - {{on: Changed, invoke: rows}}\n"
        ),
    );
    let events = before["Events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    let field = find(&before["UI"], "field")
        .or_else(|| {
            before["UI"]["Widgets"][0]["Widgets"]
                .as_array()
                .and_then(|w| {
                    w.iter()
                        .find(|w| w["Properties"]["Type"] == "ptcstextfield")
                })
        })
        .unwrap();
    assert_eq!(events[0]["EventTriggerId"], field["Properties"]["Id"]);
    assert_eq!(events[0]["EventTriggerEvent"], "Changed");
    let copies = round_trips(&solution, &before);
    assert_eq!(copies[1].1["Events"].as_array().unwrap().len(), 1);
}

#[test]
fn a_widget_node_is_refused_with_a_suggestion_for_a_wrong_prop_event_service_or_type() {
    let (_dir, solution) = element_solution();
    let page = |node: &str| {
        format!("  data:\n    rows: {{service: Acme.Data.GetRows}}\n  slots:\n    center:\n{node}")
    };
    let prop = spec_error(&solution, &page(&FIELD.replace("Text:", "Texxt:")));
    assert!(
        prop.contains("has no property Texxt") && prop.contains("did you mean Text"),
        "{prop}"
    );
    let base = spec_error(
        &solution,
        &page(&FIELD.replace("{Text: Acme}", "{Disabled: maybe}")),
    );
    assert!(base.contains("Disabled must be true or false"), "{base}");
    let event = spec_error(
        &solution,
        &page(&format!(
            "{FIELD}          events:\n            - {{on: Chnged, invoke: rows}}\n"
        )),
    );
    assert!(
        event.contains("no event Chnged") && event.contains("did you mean Changed"),
        "{event}"
    );
    let service = spec_error(
        &solution,
        &page(&format!(
            "{FIELD}          events:\n            - {{on: Changed, invoke: field.ShowValidationStat}}\n"
        )),
    );
    assert!(
        service.contains("no service ShowValidationStat")
            && service.contains("did you mean ShowValidationState"),
        "{service}"
    );
    let kind = spec_error(
        &solution,
        &page(&FIELD.replace("ptcstextfield", "ptcstextfeld")),
    );
    assert!(
        kind.contains("no widget type ptcstextfeld") && kind.contains("did you mean ptcstextfield"),
        "{kind}"
    );
    let qualified = spec_error(
        &solution,
        &page(&format!(
            "{FIELD}          events:\n            - {{on: field.Changed, invoke: rows}}\n"
        )),
    );
    assert!(qualified.contains("bare event name"), "{qualified}");
    let element = spec_error(
        &solution,
        &page("      - label: {id: note, text: Acme, events: [{on: Clicked, invoke: rows}]}\n"),
    );
    assert!(
        element.contains("events belong to widget nodes"),
        "{element}"
    );
}

#[test]
fn a_widget_node_holds_children_in_its_own_widgets() {
    let (_dir, solution) = element_solution();
    let before = source(
        &solution,
        "  slots:
    center:
      - widget:
          id: ptcsdynamicpanel-5
          type: ptcsdynamicpanel
          children:
            - label: {id: ptcslabel-6, text: Acme}
",
    );
    let panel = find(&before["UI"], "ptcsdynamicpanel-5").unwrap();
    assert!(find(panel, "ptcslabel-6").is_some(), "{panel}");
    round_trips(&solution, &before);
}

#[test]
fn widgets_documents_what_a_widget_node_accepts() {
    let detail = crate::core::widgets_view::detail("ptcstextfield").unwrap();
    assert!(
        detail.page_spec.contains("type: ptcstextfield"),
        "{}",
        detail.page_spec
    );
    assert!(
        detail.page_spec.contains("events: [{on: Changed"),
        "{}",
        detail.page_spec
    );
    let container = crate::core::widgets_view::detail("flexcontainer").unwrap();
    assert!(
        container.page_spec.contains("children:"),
        "{}",
        container.page_spec
    );
}
