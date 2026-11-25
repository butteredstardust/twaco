use super::*;
use serde_json::json;

#[test]
fn brief83_field_page_diagnostic_names_the_expression_and_its_line() {
    let (_dir, solution) = element_solution();
    let spec = include_str!("../../../../tests/fixtures/backlog/card.page.yaml");
    let file = solution.root.join("Acme.field.page.yaml");
    std::fs::write(&file, spec).unwrap();
    let error = super::super::build::plan(&solution, "Acme.CardCopy_MU", Some("Acme"), Some(&file))
        .unwrap_err()
        .to_string();
    assert!(error.contains("expression2_12.input2"), "{error}");
    assert!(error.contains("Acme.field.page.yaml:29:"), "{error}");
    // Re-deriving from correctly associated expression widgets avoids these stale props.
    let mut value: Value = serde_saphyr::from_str(spec).unwrap();
    for source in value["page"]["data"].as_object_mut().unwrap().values_mut() {
        source.as_object_mut().unwrap().remove("props");
    }
    std::fs::write(&file, super::super::derive::render(&value)).unwrap();
    let plan = super::super::build::plan(&solution, "Acme.CardCopy_MU", Some("Acme"), Some(&file))
        .unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
}

#[test]
fn brief83_expression_diagnostics_name_spec_nodes() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Expressions_MU\n  template: blank\n  parameters: {value: STRING}\n  data:\n    expression2_12:\n      expression: input\n      inputs: {input: {type: STRING}}\n      output: STRING\n  bindings:\n    - {from: param.value, to: expression2_12.input2}\n";
    let error = compile(&solution, "Acme", spec).unwrap_err();
    assert!(
        error.message.contains("expression2_12") && error.message.contains("input2"),
        "{error}"
    );
    assert_eq!(error.line, 13, "{error}");
    assert!(!error.message.contains("expression2-"), "{error}");
}

#[test]
fn brief83_widget_warnings_name_the_node_and_its_spec_line() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.View_MU\n  template: blank\n  layout: as-built\n  slots:\n    center:\n      - widget: {id: viewer, type: AcmeViewer}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    assert!(
        built.warnings.iter().any(|w| w.starts_with("page.yaml:9:")
            && w.contains("viewer:")
            && !w.contains("AcmeViewer-")),
        "{:?}",
        built.warnings
    );
}

#[test]
fn brief83_spec_urls_keep_the_protocol_relative_note() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.View_MU\n  template: blank\n  slots:\n    center:\n      - image: {id: preview, source: '//Thingworx/MediaEntities/Acme.Image'}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    assert!(
        built
            .warnings
            .iter()
            .any(|w| w.contains("protocol-relative") && w.contains("MSYS_NO_PATHCONV=1")),
        "{:?}",
        built.warnings
    );
}

#[test]
fn brief83_expression_props_stay_with_their_widget() {
    let (_dir, solution) = element_solution();
    let mut spec = String::from(
        "version: 1\nrules: v1\npage:\n  name: Acme.Expressions_MU\n  template: blank\n  parameters: {enabled: BOOLEAN}\n  data:\n",
    );
    for n in [5, 12, 14] {
        spec.push_str(&format!("    expression2_{n}:\n      widget_id: expression2-{n}\n      expression: 'input + {n}'\n      inputs: {{input: {{type: NUMBER}}{}}}\n      output: NUMBER\n      props: {{DisplayName: AcmeExpression{n}, DefaultNumberEvaluation: false}}\n", if n == 12 {", input2: {type: BOOLEAN, value: {bind: param.enabled}}"} else {""}));
    }
    let made = compile(&solution, "Acme", &spec).unwrap();
    let mut before: Value = serde_json::from_str(&made.content).unwrap();
    before.as_object_mut().unwrap().remove("twacoPageRules");
    // Composer stores function widgets in numeric order, unlike the data alias map.
    before["UI"]["Widgets"]
        .as_array_mut()
        .unwrap()
        .sort_by_key(|w| {
            w["Properties"]["Id"]
                .as_str()
                .unwrap()
                .rsplit('-')
                .next()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        });
    let name = "Acme.Expressions_MU";
    let entity = mashup::sync(
        &made.entity,
        &mashup::Assets {
            content: before.to_string(),
            css: made.css,
        },
    )
    .unwrap()
    .0;
    std::fs::create_dir_all(solution.root.join("acme/Mashups")).unwrap();
    std::fs::write(
        solution.root.join(format!("acme/Mashups/{name}.xml")),
        entity,
    )
    .unwrap();
    let derived = super::super::extract::extract(&solution, name).unwrap();
    let value: Value = serde_saphyr::from_str(&derived.spec).unwrap();
    for n in [5, 12, 14] {
        let entry = &value["page"]["data"][format!("expression2_{n}")];
        assert_eq!(
            entry["props"]["DisplayName"],
            format!("AcmeExpression{n}"),
            "{}",
            derived.spec
        );
        if let Some(expression) = entry["props"].get("Expression") {
            assert_eq!(expression, &entry["expression"], "{}", derived.spec);
        }
    }
    let after = compile(&solution, "Acme", &derived.spec).unwrap();
    let after = mashup::edit::parse(&after.content).unwrap();
    let before = mashup::edit::MashupDocument::new(before).unwrap();
    for n in [5, 12, 14] {
        let id = format!("expression2-{n}");
        let left = before.find_exact(&id).unwrap();
        let right = after.find_exact(&id).unwrap();
        for key in [
            "Expression",
            "DisplayName",
            "_currentParameterDefs",
            "DefaultNumberEvaluation",
        ] {
            assert_eq!(
                before.at(&left.path)["Properties"][key],
                after.at(&right.path)["Properties"][key],
                "{id}.{key}: {}",
                derived.spec
            );
        }
    }
}

#[test]
fn brief83_css_hand_edit_is_reported_and_conflicts_when_spec_changes() {
    let (_dir, solution) = solution();
    create_page(&solution);
    let files = page_files(&solution);
    let page = SPEC.replace("  template:", "  css: page.css\n  template:");
    std::fs::write(
        files[4].parent().unwrap().join("page.css"),
        ".acme {color: red}\n",
    )
    .unwrap();
    std::fs::write(&files[4], &page).unwrap();
    let mut plan = super::super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    {
        let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
        plan.apply(&solution, &guard).unwrap();
    }
    let assets = mashup::Assets {
        content: std::fs::read_to_string(&files[1]).unwrap(),
        css: ".acme {color: blue}\n".into(),
    };
    let entity = mashup::sync(&std::fs::read(&files[0]).unwrap(), &assets)
        .unwrap()
        .0;
    std::fs::write(&files[0], entity).unwrap();
    std::fs::write(&files[2], &assets.css).unwrap();
    let plan = super::super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(
        plan.report["hand_edits_kept"]
            .as_array()
            .unwrap()
            .contains(&json!("custom.css")),
        "{}",
        plan.report
    );
    std::fs::write(
        files[4].parent().unwrap().join("page.css"),
        ".acme {color: green}\n",
    )
    .unwrap();
    let plan = super::super::build::plan(&solution, "Acme.Alerts_MU", None, None).unwrap();
    assert!(plan.conflicts(), "{}", plan.report);
    assert!(
        plan.report["conflicts"].to_string().contains("custom.css"),
        "{}",
        plan.report
    );
}

#[test]
fn brief83_page_floor_survives_a_display_name_change() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Floor_MU\n  template: blank\n  slots:\n    center:\n      - label: {id: title, text: Acme}\n      - label: {id: detail, text: Detail}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let mut doc = mashup::edit::parse(&built.content).unwrap();
    let center = doc.find("center").unwrap();
    let expected = doc.at(&center.path)["Properties"]["flex-min-height"].clone();
    assert_ne!(expected, json!(""));
    doc.set_property(&center.path, "DisplayName", json!("AcmePage"));
    doc.set_property(&center.path, "flex-min-height", json!(""));
    mashup::layout::finish_page(&mut doc);
    assert_eq!(
        doc.at(&center.path)["Properties"]["flex-min-height"],
        expected
    );
}

#[test]
fn brief83_template_content_slot_and_master_area_keep_their_floor_when_renamed() {
    let (_dir, solution) = element_solution();
    for master in [false, true] {
        let spec = format!("version: 1\nrules: v1\npage:\n  name: Acme.Floor_MU\n  template: list-page\n{}  slots:\n    body:\n      - label: {{id: title, text: Acme}}\n      - label: {{id: detail, text: Detail}}\n{}", if master {"  kind: master\n"} else {""}, if master {"      - page_host: {id: host}\n"} else {""});
        let built = compile(&solution, "Acme", &spec).unwrap();
        let mut doc = mashup::edit::parse(&built.content).unwrap();
        let area = doc.find("body").unwrap();
        let expected = doc.at(&area.path)["Properties"]["flex-min-height"].clone();
        assert_ne!(expected, json!(""));
        doc.set_property(&area.path, "DisplayName", json!("AcmeBody"));
        doc.set_property(&area.path, "flex-min-height", json!(""));
        mashup::layout::finish_page(&mut doc);
        assert_eq!(
            doc.at(&area.path)["Properties"]["flex-min-height"],
            expected
        );
    }
}
