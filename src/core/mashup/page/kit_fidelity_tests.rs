use super::*;
use authoring::{session::Session, AddRequest, Recipe};
use serde_json::json;

fn add(session: &mut Session, solution: &Solution, widget: &str, parent: &str) -> String {
    session
        .add(
            solution,
            &AddRequest {
                mashup: "Acme.Errors_MU".into(),
                widget: widget.into(),
                inputs: vec![],
                placement: Some(mashup::edit::Placement::In(parent.into())),
                label: None,
                properties: vec![],
            },
        )
        .unwrap()
}

#[test]
fn filter_header_rule_and_full_bleed_are_shared_with_command_templates() {
    let (_dir, solution) = element_solution();
    let text = error_page("filter_panel: {id: filters, fields: [{text_field: {id: search}}], children: [{label: {id: content, text: Acme}}]}", "");
    let built = compile(&solution, "Acme", &text).unwrap();
    let doc = doc_of(&built);
    let title = doc
        .widgets()
        .into_iter()
        .find(|w| w.display_name == "filter-title")
        .unwrap();
    let header = &title.path[..title.path.len() - 1];
    assert_eq!(doc.at(header)["Properties"]["flex-min-height"], "48px");
    assert_eq!(doc.at(&title.path)["Properties"]["Margin"], "12 40 12 20");
    assert_eq!(doc.at(&title.path)["Properties"]["LabelText"], "Filter");
    let search = doc.find_exact(&built.ids["search"]).unwrap();
    assert!(!search.path.starts_with(header));
    assert_eq!(doc.at(&[0])["Properties"]["Margin"], "");
    let mut command = Session::new(
        "Acme.Errors_MU",
        "Acme",
        "filter-panel",
        "PTC Convergence Theme",
    )
    .unwrap();
    let parent = command.document.find("filters").unwrap().id;
    add(&mut command, &solution, "ptcstextfield", &parent);
    for display in ["filter-header", "filter-title", "filter-column"] {
        let a = doc
            .widgets()
            .into_iter()
            .find(|w| w.display_name == display)
            .unwrap();
        let b = command
            .document
            .widgets()
            .into_iter()
            .find(|w| w.display_name == display)
            .unwrap();
        for property in ["Margin", "ExpandCollapseTabLocation"] {
            assert_eq!(
                doc.at(&a.path)["Properties"][property],
                command.document.at(&b.path)["Properties"][property],
                "{display}.{property}"
            );
        }
    }
    let standalone = compile(&solution, "Acme", &error_page("row: {id: shell, children: [{filter_panel: {id: f, fields: [{text_field: {id: q}}]}}, {column: {id: main, children: [{label: {id: t, text: Acme}}]}}]}", "")).unwrap();
    let doc = doc_of(&standalone);
    let filter = doc.find_exact(&standalone.ids["f"]).unwrap();
    let rule = &doc.at(&filter.path)["Widgets"][1];
    assert_ne!(rule["Properties"]["Visible"], false);
    assert_eq!(rule["Properties"]["flex-min-width"], "1px");
    assert_eq!(doc.at(&[0])["Properties"]["Margin"], "");
    assert_eq!(element_props(&standalone, "main")["Margin"], "24");
}

#[test]
fn side_panel_recipe_moves_the_gutter_to_its_content_sibling() {
    let (_dir, solution) = element_solution();
    let built = compile(&solution, "Acme", &error_page("row: {id: shell, size: fill, children: [{side_panel: {id: side, title: Acme}}, {column: {id: main, children: [{label: {id: title, text: Acme}}]}}]}", "")).unwrap();
    assert_eq!(element_props(&built, "side")["Margin"], "");
    assert_eq!(element_props(&built, "side")["align-self"], "stretch");
    assert_eq!(element_props(&built, "main")["Margin"], "24");
    let doc = doc_of(&built);
    assert_eq!(doc.at(&[0])["Properties"]["Margin"], "");
    assert_eq!(
        widget_style(&built, "side")["parts"]["$"]["$"]["background"],
        "#F7F7F7"
    );
}

#[test]
fn charts_and_grid_defaults_match_recipes_and_explicit_options_win() {
    let (_dir, solution) = element_solution();
    for (kind, widget, value) in [
        ("bar", "ptcschartbar", None),
        ("line", "ptcschartline", None),
        ("pie", "ptcschartpie", Some("Count")),
    ] {
        let field = value.map_or(String::new(), |v| format!(", value: {v}"));
        let text = error_page(
            &format!("chart: {{id: c, kind: {kind}, data: rows, x: Name{field}}}"),
            "",
        );
        let built = compile(&solution, "Acme", &text).unwrap();
        let mut command =
            Session::new("Acme.Errors_MU", "Acme", "blank", "PTC Convergence Theme").unwrap();
        let chart = add(&mut command, &solution, widget, "center");
        command
            .recipe(
                &solution,
                Recipe::Chart {
                    widget: chart.clone(),
                    data: "data:Acme.Data.GetRows".into(),
                    x: "Name".into(),
                    value: value.map(str::to_owned),
                    series: vec![],
                    kind: None,
                    title: None,
                    x_title: None,
                    y_title: None,
                    stacked: false,
                },
            )
            .unwrap();
        let path = command.document.find_exact(&chart).unwrap().path;
        let actual = &command.document.at(&path)["Properties"];
        for property in [
            "LegendPosition",
            "LegendAlignment",
            "HideLegend",
            "ShowValues",
            "HideValues",
            "ValuesPosition",
        ] {
            assert_eq!(
                element_props(&built, "c")[property],
                actual[property],
                "{kind}.{property}"
            );
        }
        assert_eq!(actual["LegendPosition"], "top");
        assert_eq!(actual["HideLegend"], kind != "pie");
        if kind == "pie" {
            assert_eq!(actual["ShowValues"], true);
        }
        if kind == "bar" {
            assert_eq!(actual["HideValues"], false);
            assert_eq!(actual["ValuesPosition"], "inside");
        }
        let explicit = text.replace(
            "id: c,",
            "id: c, legend: true, props: {LegendPosition: right},",
        );
        let explicit = compile(&solution, "Acme", &explicit).unwrap();
        assert_eq!(element_props(&explicit, "c")["LegendPosition"], "right");
        assert_eq!(element_props(&explicit, "c")["HideLegend"], false);
    }
    let built = compile(
        &solution,
        "Acme",
        &error_page("grid: {id: grid, data: rows, columns: [Name, Count]}", ""),
    )
    .unwrap();
    let style = widget_style(&built, "grid");
    assert_eq!(
        style["parts"]["core-grid"]["$"]["border-bottom-style"],
        "none"
    );
    assert_eq!(
        style["parts"]["core-grid"]["$"]["border-bottom-width"],
        "0px"
    );
}

#[test]
fn selected_navigation_and_indents_validate_and_style_exactly() {
    let (_dir, solution) = element_solution();
    let text = error_page("nav_menu: {id: nav, selected: Details, groups: [{title: Acme, items: [Overview, {label: Details, indent: 2}]}]}", "");
    let built = compile(&solution, "Acme", &text).unwrap();
    let first = widget_style(&built, "nav--item-1-1");
    let selected = widget_style(&built, "nav--item-1-2");
    assert!(first["parts"]["$"]["$"].get("background").is_none());
    assert_eq!(selected["parts"]["$"]["$"]["background"], "#e4e7e9");
    assert_eq!(selected["parts"]["$"]["$"]["padding-left"], "60px");
    for text in [
        text.replace("selected: Details", "selected: Missing"),
        text.replace("Overview,", "Details,"),
        text.replace("indent: 2", "indent: 13"),
    ] {
        let error = compile(&solution, "Acme", &text).unwrap_err();
        assert!(error.line > 0);
    }
}

#[test]
fn clean_page_has_exact_card_and_page_styles() {
    benchmark("clean");
    let value: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("bench_clean/page.golden.json")).unwrap(),
    )
    .unwrap();
    let styles = &value["content"]["UI"]["Properties"]["StyleProperties"]["data"];
    let card = &styles[value["ids"]["focus_1"].as_str().unwrap()]["parts"]["$"]["$"];
    assert_eq!(card["background"], "#FFFFFF");
    assert_eq!(card["border-color"], "#E5E7EB");
    assert_eq!(card["border-width"], "1px");
    assert_eq!(card["border-radius"], "8px");
    assert_eq!(card["box-shadow"], "0 1px 3px #0000001A");
    assert_eq!(
        card["font-family"],
        "Inter, ui-sans-serif, Arial, sans-serif"
    );
    let page = value["content"]["UI"]["Widgets"][0]["Properties"]["Id"]
        .as_str()
        .unwrap();
    // A container-level background would stop at this page container's gutter; the root's
    // inline style replaces the white legacy DefaultMashupStyle behind the whole page.
    assert!(styles.get(page).is_none());
    let root_style = &value["content"]["UI"]["Properties"]["Style"];
    assert_eq!(root_style["backgroundColor"], "#F9FAFB");
    assert_eq!(root_style["lineStyle"], "solid");
}

#[test]
fn media_cells_have_exact_artifact_goldens() {
    let solution = Solution::load(&fixtures().join("bench_solution/twaco.toml")).unwrap();
    let text = std::fs::read_to_string(fixtures().join("kit_fidelity/page.yaml")).unwrap();
    let built = compile(&solution, "Acme.Bench", &text).unwrap();
    assert_eq!(
        element_props(&built, "ticker")["VerticalAlignment"],
        "center"
    );
    let pie = widget_style(&built, "pie");
    for (index, color) in ["#CCFF00", "#FF2E93", "#8B5CFF", "#FF8C3C", "#4EE0AE"]
        .into_iter()
        .enumerate()
    {
        let series = format!("serie{}", index + 1);
        assert_eq!(pie["parts"]["slice"][&series]["fill"], color);
        assert_eq!(pie["parts"]["legend-marker"][&series]["background"], color);
    }
    let cell = &built.cells[0].artifacts;
    assert_eq!(element_props(cell, "image")["Scaling"], "contain");
    let image = &cell.ids["image"];
    let content: Value = serde_json::from_str(&cell.content).unwrap();
    assert!(content["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["TargetId"] == *image && b["PropertyMaps"][0]["TargetProperty"] == "SourceURL"));
    assert!(element_props(cell, "status--dot")["ShapeFormatter"].is_object());
    let actual = serde_json::to_string_pretty(&json!({"content":serde_json::from_str::<Value>(&built.content).unwrap(), "ids":built.ids, "cells":built.cells.iter().map(|cell| json!({"spec":cell.spec,"content":serde_json::from_str::<Value>(&cell.artifacts.content).unwrap(),"entity":String::from_utf8(cell.artifacts.entity.clone()).unwrap(),"ids":cell.artifacts.ids})).collect::<Vec<_>>()})).unwrap() + "\n";
    let golden = fixtures().join("kit_fidelity/page.golden.json");
    if std::env::var_os("TWACO_BLESS").is_some() {
        std::fs::write(&golden, &actual).unwrap();
    }
    assert_eq!(std::fs::read_to_string(golden).unwrap(), actual);
}

#[test]
fn command_components_share_selected_navigation_and_generated_media_cells() {
    let (_dir, solution) = bench_workspace();
    let mut session = Session::new(
        "Acme.Commands_MU",
        "Acme.Bench",
        "blank",
        "PTC Convergence Theme",
    )
    .unwrap();
    session
        .source(
            &solution,
            &authoring::SourceAddRequest {
                mashup: "Acme.Commands_MU".into(),
                entity: "Acme.MediaData".into(),
                service: "GetCards".into(),
                alias: Some("assets".into()),
                dynamic: false,
                on_load: true,
                entity_from: None,
                refresh: None,
                cache: false,
                cache_keys: vec![],
                cache_strategy: None,
                max_result_sets: None,
                param_values: vec![],
            },
        )
        .unwrap();
    session.sync().unwrap();
    std::fs::create_dir_all(solution.root.join("Mashups")).unwrap();
    std::fs::write(
        solution.root.join("Mashups/Acme.Commands_MU.xml"),
        &session.entity,
    )
    .unwrap();
    let request = |name: &str, settings: Vec<String>| authoring::RecipeRequest {
        mashup: "Acme.Commands_MU".into(),
        recipe: Recipe::Component {
            container: "center".into(),
            name: name.into(),
            settings,
        },
    };
    let menu = authoring::plan_recipe(&solution, &request("nav_menu", vec!["id=menu".into(), "selected=Details".into(), "groups=[{\"title\":\"Acme\",\"items\":[\"Overview\",{\"label\":\"Details\",\"indent\":1}]}]".into()])).unwrap();
    let actual =
        mashup::edit::parse(&mashup::extract(&menu.edit.entity_after).unwrap().content).unwrap();
    let selected = actual
        .widgets()
        .into_iter()
        .find(|w| actual.at(&w.path)["Properties"]["Label"] == "Details")
        .unwrap();
    assert_eq!(
        actual.root_properties()["StyleProperties"]["data"][&selected.id]["parts"]["$"]["$"]
            ["background"],
        "#e4e7e9"
    );
    assert_eq!(
        actual.root_properties()["StyleProperties"]["data"][&selected.id]["parts"]["$"]["$"]
            ["padding-left"],
        "42px"
    );
    let plan = authoring::plan_recipe(&solution, &request("collection", vec!["id=cards".into(), "data=assets".into(), "cell={\"image\":\"Image\",\"title\":\"Name\",\"status\":{\"field\":\"Status\",\"state\":\"Acme.BenchStatus_ST\"}}".into()])).unwrap();
    assert_eq!(plan.generated.len(), 1);
    let guard = lock::acquire_for(&solution, "test media cells").unwrap();
    authoring::apply_recipe(&solution, &plan, &guard).unwrap();
    let cell = solution.src_root().join("Acme.CommandsCell_MU/page.yaml");
    let spec = std::fs::read_to_string(cell).unwrap();
    assert!(
        spec.contains("\"image\"")
            && spec.contains("\"status\"")
            && spec.contains("Acme.BenchStatus_ST")
    );
    let rebuilt = compile(&solution, "Acme.Bench", &spec).unwrap();
    assert_eq!(element_props(&rebuilt, "image")["Scaling"], "contain");
    assert!(element_props(&rebuilt, "status--dot")["ShapeFormatter"].is_object());
}
