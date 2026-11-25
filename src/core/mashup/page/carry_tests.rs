//! A mashup's look (custom.css and stored widget styles) survives derive and a fresh build.

use super::super::extract as extraction;
use super::adopt::{apply, current, handmade, write_spec, HAND_SPEC};
use super::*;
use crate::core::mashup::{diff, Assets};

const CSS: &str = "#acme-sidebar { width: 240px; }\r\n/* kept as written */\r\n.acme-card > div { padding: 4px; }";

/// A hand-made mashup with a stylesheet and stored styles on two containers: one whose parts a
/// `style:` list could say, one with a state and a number. One entry names no widget at all.
fn styled(solution: &Solution) -> String {
    let name = handmade(
        solution,
        &HAND_SPEC.replace("Acme.Hand_MU", "Acme.Look_MU"),
        "Acme.Look_MU",
    );
    let loaded = authoring::load_existing(solution, &name).unwrap();
    let mut value: Value = serde_json::from_str(&loaded.assets.content).unwrap();
    let containers: Vec<String> = {
        fn walk(widget: &Value, out: &mut Vec<String>) {
            if widget["Properties"]["Type"] == "flexcontainer" {
                out.push(widget["Properties"]["Id"].as_str().unwrap().to_string());
            }
            for child in widget["Widgets"].as_array().into_iter().flatten() {
                walk(child, out);
            }
        }
        let mut out = vec![];
        walk(&value["UI"], &mut out);
        out
    };
    assert!(containers.len() >= 3, "{containers:?}");
    let data = &mut value["UI"]["Properties"]["StyleProperties"];
    *data = serde_json::json!({"data": {
        containers[1].clone(): {"parts": {"$": {"$": {"background": "#F7F7F7", "border-style": "solid"}}}},
        containers[2].clone(): {"parts": {"$": {"hover": {"opacity": 0.5}}, "title": {"$": {"color": "#112233"}}}},
        "gone-9": {"parts": {"$": {"$": {"background": "red"}}}}
    }});
    let assets = Assets {
        content: mashup::to_sidecar(&value),
        css: format!("{CSS}\n"),
    };
    let entity = mashup::sync(
        &std::fs::read(solution.root.join(format!("acme/Mashups/{name}.xml"))).unwrap(),
        &assets,
    )
    .unwrap()
    .0;
    std::fs::write(
        solution.root.join(format!("acme/Mashups/{name}.xml")),
        entity,
    )
    .unwrap();
    name
}

fn styles_of(solution: &Solution, name: &str) -> Value {
    current(solution, name)["UI"]["Properties"]["StyleProperties"]["data"].clone()
}

#[test]
fn a_derived_spec_carries_the_stylesheet_and_stored_styles_into_a_fresh_copy() {
    let (_dir, solution) = element_solution();
    let name = styled(&solution);
    let source = authoring::load_existing(&solution, &name).unwrap().assets;
    let out = solution.root.join("specs");
    std::fs::create_dir_all(&out).unwrap();
    let spec_path = out.join("look.yaml");
    let derived = extraction::export(&solution, &name, Some(&spec_path)).unwrap();
    assert!(derived.spec.contains("css: page.css"), "{}", derived.spec);
    assert!(derived.spec.contains("styles:"), "{}", derived.spec);
    assert!(
        !derived
            .notes
            .iter()
            .any(|note| note.contains("unsupported wiring is not carried")
                || note.contains("styles of widgets folded")),
        "{:?}",
        derived.notes
    );
    // The file is the sidecar, byte for byte (CRLF inside included).
    assert_eq!(
        std::fs::read(out.join("page.css")).unwrap(),
        source.css.as_bytes()
    );
    // What nothing could carry is itemised.
    assert!(
        derived
            .notes
            .iter()
            .any(|n| n == "stale style entry for removed widget gone-9 (harmless; dropped)"),
        "{:?}",
        derived.notes
    );

    let copy = "Acme.LookCopy_MU";
    let text = std::fs::read_to_string(&spec_path)
        .unwrap()
        .replace(&format!("name: {name}"), &format!("name: {copy}"));
    std::fs::write(&spec_path, &text).unwrap();
    let mut plan =
        super::super::build::plan(&solution, copy, Some("Acme"), Some(&spec_path)).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let guard = lock::acquire(&solution.root, "carry test", &[]).unwrap();
    plan.apply(&solution, &guard).unwrap();

    let built = authoring::load_existing(&solution, copy).unwrap().assets;
    assert_eq!(built.css.as_bytes(), source.css.as_bytes());
    let mut want = styles_of(&solution, &name);
    want.as_object_mut().unwrap().remove("gone-9");
    assert_eq!(styles_of(&solution, copy), want);
    // The css never lands in content, and the copy keeps the file for rebuilds.
    assert!(std::fs::metadata(solution.src_root().join(copy).join("page.css")).is_ok());
    let result = diff::diff(&solution, &name, copy, diff::Tier::Visual).unwrap();
    let styled: Vec<_> = result
        .differences
        .iter()
        .filter(|d| d.path.contains("Style") || d.path.contains("css"))
        .collect();
    assert!(styled.is_empty(), "{styled:?}");
    assert_eq!(result.summary.layout, 0, "{:?}", result.differences);

    // Nothing to rebuild.
    let again = super::super::build::plan(&solution, copy, None, None).unwrap();
    assert_eq!(
        again.report["files"],
        serde_json::json!([]),
        "{}",
        again.report
    );
}

#[test]
fn adopting_a_styled_mashup_writes_nothing_and_a_rebuild_reports_nothing() {
    let (_dir, solution) = element_solution();
    let name = styled(&solution);
    let dir = solution.src_root().join(&name);
    let derived = extraction::extract(&solution, &name).unwrap();
    write_spec(&solution, &name, &derived.spec);
    std::fs::write(dir.join("page.css"), derived.css.as_ref().unwrap()).unwrap();
    let xml = solution.root.join(format!("acme/Mashups/{name}.xml"));
    let before = std::fs::read(&xml).unwrap();
    let css_before = authoring::load_existing(&solution, &name)
        .unwrap()
        .assets
        .css;
    let styles_before = styles_of(&solution, &name);
    let report = apply(&solution, &name);
    assert_eq!(report["conflicts"], serde_json::json!([]), "{report}");
    assert_eq!(std::fs::read(&xml).unwrap(), before);
    assert_eq!(
        authoring::load_existing(&solution, &name)
            .unwrap()
            .assets
            .css,
        css_before
    );
    assert_eq!(styles_of(&solution, &name), styles_before);
    let again = super::super::build::plan(&solution, &name, None, None).unwrap();
    assert_eq!(
        again.report["files"],
        serde_json::json!([]),
        "{}",
        again.report
    );
    assert_eq!(again.report["hand_edits_kept"], serde_json::json!([]));
}

#[test]
fn a_missing_page_css_is_a_located_spec_error() {
    let (_dir, solution) = element_solution();
    let text = HAND_SPEC.replace(
        "  layout: as-built\n",
        "  layout: as-built\n  css: missing.css\n",
    );
    let error = compile(&solution, "Acme", &text).unwrap_err();
    assert!(error.message.contains("missing.css"), "{error}");
    assert_eq!((error.line, error.column), (7, 8), "{error}");
}

#[test]
fn a_spec_returned_without_a_file_does_not_reference_page_css() {
    let (_dir, solution) = element_solution();
    let name = styled(&solution);
    // `mashup spec --json` without --out (and the MCP tool) write no file beside the spec.
    let derived = extraction::export(&solution, &name, None).unwrap();
    assert!(!derived.spec.contains("css:"), "{}", derived.spec);
    assert!(derived.css.is_some());
    assert!(
        derived.notes.iter().any(|n| n.contains("--out <file>")),
        "{:?}",
        derived.notes
    );
    // So the spec builds against the mashup it came from.
    let folder = solution.src_root().join(&name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("page.yaml"), &derived.spec).unwrap();
    let plan = super::super::build::plan(&solution, &name, None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
}

#[test]
fn uncarried_whole_widget_styles_name_the_widget_in_words() {
    let (_dir, solution) = element_solution();
    let name = styled(&solution);
    let mut content = current(&solution, &name);
    // The root's own styles are carried as page styles; a child's are not by plain derive.
    let id = content["UI"]["Widgets"][0]["Properties"]["Id"]
        .as_str()
        .unwrap()
        .to_string();
    content["UI"]["Properties"]["StyleProperties"]["data"][&id] =
        serde_json::json!({"parts":{"$":{"$":{"background":"red"}}}});
    let derived = super::super::derive::derive(&name, &content.to_string()).unwrap();
    assert!(
        derived
            .notes
            .iter()
            .any(|note| note.contains(&id) && note.contains("the widget itself")),
        "{:?}",
        derived.notes
    );
    assert!(!derived.notes.iter().any(|note| note.contains("parts $")));
}
