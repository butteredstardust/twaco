use super::super::derive;
use super::*;
use serde_json::json;

const HEADER: &str =
    "version: 1\nrules: v1\npage:\n  name: Acme.Native_MU\n  template: blank\n  layout: as-built\n";

fn round_trip(solution: &Solution, text: &str, elements: &[&str]) -> Value {
    let built = compile(solution, "Acme", text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let before: Value = serde_json::from_str(&built.content).unwrap();
    for exact in [true, false] {
        let derived = if exact {
            derive::derive_exact("Acme.Copy_MU", &built.content)
        } else {
            derive::derive("Acme.Copy_MU", &built.content)
        }
        .unwrap();
        for element in elements {
            assert!(
                derived.spec.contains(&format!("{element}:")),
                "{}",
                derived.spec
            );
        }
        assert_eq!(derived.raw, 0, "{}", derived.spec);
        let rebuilt = compile(solution, "Acme", &derived.spec)
            .unwrap_or_else(|e| panic!("{e}\n{}", derived.spec));
        for tier in [mashup::diff::Tier::Executable, mashup::diff::Tier::Layout] {
            if !exact && matches!(tier, mashup::diff::Tier::Layout) {
                continue;
            }
            let after: Value = serde_json::from_str(&rebuilt.content).unwrap();
            let diff = mashup::diff::compare("a", "b", &before, &after, None, None, tier);
            assert!(diff.equal, "{:?}\n{}", diff.differences, derived.spec);
        }
    }
    before
}

#[test]
fn native_navigation_keeps_identity_snapshots_parameters_and_multiple_triggers() {
    let (_dir, solution) = element_solution();
    let text = format!("{HEADER}  parameters: {{target: MASHUPNAME, selected: STRING, height: NUMBER}}\n  slots:\n    center:\n      - button: {{id: open, text: Open}}\n      - navigate:\n          id: nav\n          widget_id: navigationfunction-17\n          destination: {{bind: param.target}}\n          mode: modal-popup\n          popup_title: Details\n          popup_width: 640\n          popup_height: {{bind: param.height}}\n          parameter_defs: [{{ParameterName: selected, BaseType: STRING, DefaultValue: Acme}}]\n          params: {{selected: {{bind: param.selected}}, fixed: Acme}}\n          props:\n            MashupParameters: [{{ParameterName: selected, BaseType: STRING}}, {{ParameterName: fixed, BaseType: STRING}}]\n      - label: {{id: returned, props: {{LabelText: {{bind: nav.selected}}}}}}\n  events:\n    - {{on: load, invoke: nav.Navigate}}\n    - {{on: open.Clicked, invoke: nav.Navigate}}\n    - {{on: nav.WindowClosed, invoke: mashup.CloseIfPopup}}\n    - {{on: nav.WindowClosed, invoke: mashup.ResetInputsToDefaultValue}}\n");
    let content = round_trip(&solution, &text, &["navigate"]);
    let nav = content["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["Properties"]["Type"] == "navigationfunction")
        .unwrap();
    assert_eq!(nav["Properties"]["Id"], "navigationfunction-17");
    assert_eq!(
        nav["Properties"]["_currentParameterDefs"][0]["DefaultValue"],
        "Acme"
    );
    assert_eq!(nav["Properties"]["fixed"], "Acme");
}

#[test]
fn native_grid_keeps_column_json_selection_editing_events_and_styles() {
    let (_dir, solution) = element_solution();
    for config in [
        json!({"formatInfo": [{"FieldName": "Name", "Title": "Name", "Width": "auto", "custom": {"x": true}}]}),
        json!("{\"formatInfo\":[{\"FieldName\":\"Name\"}]}"),
    ] {
        let text = format!("{HEADER}  data:\n    rows: {{service: Acme.Data.GetRows}}\n  slots:\n    center:\n      - grid:\n          id: ptcsgrid-7\n          data: rows\n          selection: multiple\n          editable: true\n          column_config: {config}\n          styles: {{cell: {{$: {{color: '#123456'}}}}}}\n  events:\n    - {{on: ptcsgrid-7.SelectedRowsChanged, invoke: rows}}\n");
        round_trip(&solution, &text, &["grid", "column_config"]);
    }
}

#[test]
fn native_dynamic_container_warns_for_runtime_parameters_and_keeps_bindings() {
    let (_dir, solution) = element_solution();
    let text = format!("{HEADER}  parameters: {{target: MASHUPNAME, selected: STRING}}\n  slots:\n    center:\n      - mashup_container: {{id: mashupcontainer-9, destination: {{bind: param.target}}, params: {{ASelection: {{bind: param.selected}}}}, props: {{MashupParameters: [{{ParameterName: ASelection, BaseType: STRING}}]}}}}\n");
    let built = compile(&solution, "Acme", &text).unwrap();
    assert!(
        built.warnings.iter().any(|w| w.contains("run time")),
        "{:?}",
        built.warnings
    );
    round_trip(&solution, &text, &["mashup_container"]);
}

#[test]
fn native_dynamic_panel_keeps_named_panes_ids_and_collapse_binding() {
    let (_dir, solution) = element_solution();
    let text = format!("{HEADER}  parameters: {{closed: BOOLEAN}}\n  slots:\n    center:\n      - dynamic_panel:\n          id: ptcsdynamicpanel-4\n          collapsed: {{bind: param.closed}}\n          side: right\n          panel_size: '320'\n          collapsed_size: '24'\n          main:\n            column: {{id: flexcontainer-5, children: [{{label: {{id: ptcslabel-7, text: Main}}}}]}}\n          panel:\n            column: {{id: flexcontainer-6, children: [{{label: {{id: ptcslabel-8, text: Panel}}}}]}}\n");
    round_trip(&solution, &text, &["dynamic_panel", "main", "panel"]);
    round_trip(
        &solution,
        &text.replace("'320'", "''").replace("'24'", "''"),
        &["dynamic_panel", "panel_size", "collapsed_size"],
    );
    let error = compile(
        &solution,
        "Acme",
        &text.replace("          panel:\n", "          children:\n"),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("page.yaml:"), "{error}");
}

#[test]
fn native_shape_keeps_inline_style_objects() {
    let (_dir, solution) = element_solution();
    let text = format!("{HEADER}  slots:\n    center:\n      - shape: {{id: shape-2, form: rectangle, fill: {{backgroundColor: '#123456', lineColor: '#654321', lineThickness: 2}}, unfilled: {{backgroundColor: '#ffffff'}}}}\n");
    round_trip(&solution, &text, &["shape", "fill", "unfilled"]);
}

#[test]
fn native_fields_reject_unsupported_modes_selection_and_conflicting_columns() {
    let (_dir, solution) = element_solution();
    for (node, message) in [
        (
            "navigate: {id: nav, mode: sideways}",
            "navigate mode must be",
        ),
        ("grid: {id: rows, selection: sideways}", "grid selection is"),
        (
            "grid: {id: rows, columns: [Name], column_config: {}}",
            "grid takes columns or column_config",
        ),
        (
            "dynamic_panel: {id: split, main: {column: {id: main}}}",
            "needs both main and panel",
        ),
        (
            "label: {id: label, column_config: {}}",
            "column_config/editable belongs to grid",
        ),
    ] {
        let text = format!("{HEADER}  slots:\n    center:\n      - {node}\n");
        let error = compile(&solution, "Acme", &text).unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
        assert!(error.starts_with("page.yaml:"), "{error}");
    }
}

#[test]
fn native_navigation_accepts_every_native_window_mode() {
    let (_dir, solution) = element_solution();
    for mode in ["replace", "new", "popup", "modal-popup", "fullscreen"] {
        let text = format!("{HEADER}  slots:\n    center:\n      - navigate: {{id: navigationfunction-9, destination: Acme.External_MU, mode: {mode}}}\n");
        round_trip(&solution, &text, &["navigate"]);
    }
}

#[test]
fn native_navigation_literals_use_the_child_contract_without_a_manual_snapshot() {
    let (_dir, solution) = element_solution();
    let child = compile(
        &solution,
        "Acme",
        &format!("{HEADER}  parameters: {{selected: BOOLEAN}}\n")
            .replace("Acme.Native_MU", "Acme.Child_MU"),
    )
    .unwrap();
    std::fs::write(
        solution.root.join("acme/Mashups/Acme.Child_MU.xml"),
        child.entity,
    )
    .unwrap();
    let text = format!("{HEADER}  slots:\n    center:\n      - navigate: {{id: nav, destination: Acme.Child_MU, params: {{selected: false}}}}\n").replace("  layout: as-built\n", "");
    compile(&solution, "Acme", &text).unwrap();
    let error = compile(
        &solution,
        "Acme",
        &text.replace("selected: false", "missing: false"),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("missing"), "{error}");
    let error = compile(
        &solution,
        "Acme",
        &text.replace("selected: false", "selected: invalid"),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("true or false"), "{error}");
}

#[test]
fn native_static_navigation_with_one_trigger_keeps_close_events_and_its_snapshot() {
    let (_dir, solution) = element_solution();
    let text = format!("{HEADER}  slots:\n    center:\n      - navigate: {{id: navigationfunction-9, destination: Acme.External_MU, parameter_defs: [{{ParameterName: selected, BaseType: BOOLEAN, DefaultValue: false}}], props: {{MashupParameters: []}}}}\n  events:\n    - {{on: load, invoke: navigationfunction-9.Navigate}}\n    - {{on: navigationfunction-9.WindowClosed, invoke: mashup.CloseIfPopup}}\n").replace("  layout: as-built\n", "");
    round_trip(&solution, &text, &["navigate", "parameter_defs"]);
    let without_snapshot = text.replace(
        ", parameter_defs: [{ParameterName: selected, BaseType: BOOLEAN, DefaultValue: false}]",
        "",
    );
    round_trip(&solution, &without_snapshot, &["navigate"]);
}

#[test]
fn native_navigation_carries_the_parameter_snapshot_even_when_empty_text() {
    let (_dir, solution) = element_solution();
    for snapshot in [json!([]), json!(""), json!({"custom": false})] {
        let text = format!("{HEADER}  slots:\n    center:\n      - navigate: {{id: navigationfunction-9, destination: Acme.External_MU, parameter_defs: {snapshot}, props: {{MashupParameters: []}}}}\n");
        let built = compile(&solution, "Acme", &text).unwrap();
        for exact in [true, false] {
            let derived = if exact {
                derive::derive_exact("Acme.Copy_MU", &built.content)
            } else {
                derive::derive("Acme.Copy_MU", &built.content)
            }
            .unwrap();
            assert!(derived.spec.contains("parameter_defs:"), "{}", derived.spec);
            let rebuilt = compile(&solution, "Acme", &derived.spec).unwrap();
            let doc = mashup::edit::parse(&rebuilt.content).unwrap();
            let nav = doc.find_exact("navigationfunction-9").unwrap();
            assert_eq!(
                doc.at(&nav.path)["Properties"]["_currentParameterDefs"],
                snapshot
            );
        }
    }
}
