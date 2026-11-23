use super::super::{build, extract};
use super::adopt::{apply, current, handmade, write_spec};
use super::*;

const MASTER: &str = include_str!("../../../../tests/fixtures/mashup/pages/master/page.yaml");

fn counts(content: &Value) -> (usize, usize, usize, usize) {
    fn widgets(w: &Value) -> usize {
        1 + w["Widgets"]
            .as_array()
            .into_iter()
            .flatten()
            .map(widgets)
            .sum::<usize>()
    }
    (
        widgets(&content["UI"]),
        content["Data"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
            .map(|s| s["Services"].as_array().map_or(0, Vec::len))
            .sum(),
        content["DataBindings"].as_array().map_or(0, Vec::len),
        content["Events"].as_array().map_or(0, Vec::len),
    )
}

#[test]
fn master_golden() {
    let (_dir, solution) = element_solution();
    let built = compile(&solution, "Acme", MASTER).unwrap();
    let value: Value = serde_json::from_str(&built.content).unwrap();
    assert_eq!(value["mashupType"], "targetmashup");
    let mut problems = vec![];
    mashup::validate::check_content(&built.content, &mut problems);
    assert!(
        !problems
            .iter()
            .any(|p| p.severity == mashup::validate::Severity::Error),
        "{problems:?}"
    );
    assert_eq!(value["UI"]["Properties"]["Type"], "targetmashup");
    assert_eq!(
        value["UI"]["Properties"]["Style"],
        "DefaultMasterMashupStyle"
    );
    let actual = serde_json::to_string_pretty(&serde_json::json!({
        "entity": String::from_utf8(built.entity).unwrap(), "content": value,
        "css": built.css, "ids": built.ids
    }))
    .unwrap()
        + "\n";
    let golden = fixtures().join("master/page.golden.json");
    if std::env::var_os("TWACO_BLESS").is_some() {
        std::fs::write(&golden, &actual).unwrap();
    }
    assert_eq!(std::fs::read_to_string(golden).unwrap(), actual);
}

#[test]
fn master_derives_adopts_without_changes_and_builds_fresh_with_parity() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, MASTER, "Acme.Master_MM");
    let before = current(&solution, &name);
    let path = solution.root.join("acme/Mashups/Acme.Master_MM.xml");
    let xml = std::fs::read(&path).unwrap();
    let assets = mashup::extract(&xml).unwrap();
    let sidecars = solution.src_root().join(&name).join("mashup");
    std::fs::create_dir_all(&sidecars).unwrap();
    std::fs::write(sidecars.join("content.json"), &assets.content).unwrap();
    std::fs::write(sidecars.join("custom.css"), &assets.css).unwrap();
    let derived = extract::extract(&solution, &name).unwrap();
    assert!(derived.spec.contains("kind: master"), "{}", derived.spec);
    assert!(derived.spec.contains("page_host:"), "{}", derived.spec);
    assert!(derived.spec.contains("menu_bar:"), "{}", derived.spec);
    assert!(derived.spec.contains("heading:"), "{}", derived.spec);
    assert_eq!(derived.raw, 0, "{:?}", derived.notes);
    write_spec(&solution, &name, &derived.spec);
    let plan = build::plan(&solution, &name, None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let files = plan.report["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{}", plan.report);
    assert!(files[0].as_str().unwrap().ends_with(".page-baseline.json"));
    apply(&solution, &name);
    assert_eq!(std::fs::read(path).unwrap(), xml);
    assert_eq!(current(&solution, &name), before);
    assert_eq!(
        std::fs::read_to_string(sidecars.join("content.json")).unwrap(),
        assets.content
    );
    assert_eq!(
        std::fs::read_to_string(sidecars.join("custom.css")).unwrap(),
        assets.css
    );
    assert_eq!(
        build::plan(&solution, &name, None, None).unwrap().report["files"],
        serde_json::json!([])
    );
    let copy = derived.spec.replace("Acme.Master_MM", "Acme.Copy_MM");
    let input = solution.root.join("copy.yaml");
    std::fs::write(&input, &copy).unwrap();
    let plan = build::plan(&solution, "Acme.Copy_MM", Some("Acme"), Some(&input)).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let fresh = compile(&solution, "Acme", &copy).unwrap();
    let fresh: Value = serde_json::from_str(&fresh.content).unwrap();
    assert_eq!(counts(&fresh), counts(&before));
    assert_eq!(fresh["mashupType"], "targetmashup");
    for key in ["Style", "WindowStyle", "Favicon", "Font", "AcmeRootOption"] {
        assert_eq!(
            fresh["UI"]["Properties"][key], before["UI"]["Properties"][key],
            "{key}"
        );
    }
}

#[test]
fn master_root_and_page_host_match_new_master_command() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Master_MM\n  kind: master\n  template: blank\n  slots:\n    center:\n      - page_host: {id: host}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let value: Value = serde_json::from_str(&built.content).unwrap();
    let request = authoring::NewRequest {
        name: "Acme.Command_MM".into(),
        project: None,
        template: "blank".into(),
        rules: None,
        gadget: false,
        layout: None,
        primary_menu: None,
        menu_entity: None,
        secondary_menu: None,
        theme: None,
        master: true,
        visibility: vec![],
        params: vec![],
        tabs: vec![],
        from: None,
        replacements: vec![],
    };
    let command = authoring::plan_new(&solution, &request).unwrap();
    let command: Value = serde_json::from_str(&command.assets.content).unwrap();
    let mut have = value["UI"]["Properties"].clone();
    let mut want = command["UI"]["Properties"].clone();
    have.as_object_mut().unwrap().remove("id_index");
    want.as_object_mut().unwrap().remove("id_index");
    assert_eq!(have, want);
    let mut have = value["UI"]["Widgets"][0]["Widgets"][0].clone();
    let mut want = command["UI"]["Widgets"][0]["Widgets"][0].clone();
    for w in [&mut have, &mut want] {
        w.as_object_mut()
            .unwrap()
            .retain(|k, _| !k.starts_with("twaco"));
        for key in ["Id", "DisplayName"] {
            w["Properties"].as_object_mut().unwrap().remove(key);
        }
    }
    assert_eq!(have, want);
}

#[test]
fn masters_require_a_host_and_pages_cannot_host_pages() {
    let (_dir, solution) = element_solution();
    for spec in [
        MASTER.replace(
            "page_host: {id: host}",
            "label: {id: placeholder, text: Acme}",
        ),
        MASTER.replace("  kind: master\n", ""),
        MASTER.replace(
            "{id: host}",
            "{id: host}\n                  - page_host: {id: extra}",
        ),
    ] {
        let error = compile(&solution, "Acme", &spec).unwrap_err().to_string();
        assert!(error.contains("page_host"), "{error}");
    }
    let built = compile(&solution, "Acme", MASTER).unwrap();
    let mut value: Value = serde_json::from_str(&built.content).unwrap();
    fn remove(w: &mut Value) {
        if let Some(children) = w["Widgets"].as_array_mut() {
            children.retain(|c| c["Properties"]["Type"] != "pagemashupcontainer");
            children.iter_mut().for_each(remove);
        }
    }
    remove(&mut value["UI"]);
    let error = super::super::derive::derive_exact("Acme.Master_MM", &value.to_string())
        .unwrap_err()
        .to_string();
    assert!(error.contains("page_host"), "{error}");
}

#[test]
fn master_root_props_remove_defaults_and_refuse_structural_properties() {
    let (_dir, solution) = element_solution();
    let built = compile(
        &solution,
        "Acme",
        &MASTER.replace("Font: Arial", "Font: null"),
    )
    .unwrap();
    let value: Value = serde_json::from_str(&built.content).unwrap();
    assert!(value["UI"]["Properties"].get("Font").is_none());
    for key in ["Id", "Type", "Widgets", "id_index", "_currentParameterDefs"] {
        let error = compile(
            &solution,
            "Acme",
            &MASTER.replace("AcmeRootOption: preserved", &format!("{key}: invalid")),
        )
        .unwrap_err();
        assert!(error.message.contains(key), "{error}");
        assert_eq!(error.line, 12);
    }
    let page = "version: 1\nrules: v1\npage: {name: Acme.Page_MU, template: blank}\n";
    let explicit = page.replace("template: blank", "template: blank, kind: page");
    assert_eq!(
        compile(&solution, "Acme", page).unwrap(),
        compile(&solution, "Acme", &explicit).unwrap()
    );
}

/// Changing an existing mashup between page and master would leave its entity XML saying one kind
/// and its content the other, so a build refuses it.
#[test]
fn a_build_refuses_to_change_the_kind_of_an_existing_mashup() {
    let (_dir, solution) = element_solution();
    let page = "version: 1\nrules: v1\npage:\n  name: Acme.Kind_MU\n  template: blank\n  slots:\n    center:\n      - label: {id: hello, text: Hello}\n";
    let name = handmade(&solution, page, "Acme.Kind_MU");
    let master = "version: 1\nrules: v1\npage:\n  name: Acme.Kind_MU\n  kind: master\n  template: blank\n  slots:\n    center:\n      - page_host: {id: host}\n";
    write_spec(&solution, &name, master);
    let error = super::super::build::plan(&solution, &name, None, None).unwrap_err();
    assert!(error.to_string().contains("changing the kind"), "{error}");
}

/// A filter panel's title keeps the id the generated `<id>--title` heading had before the kit
/// header, so pages rebuilt from older specs keep their title widget.
#[test]
fn a_filter_title_keeps_the_id_of_the_former_title_heading() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Filter_MU\n  template: blank\n  slots:\n    center:\n      - filter_panel: {id: f, title: Filter, fields: [{text_field: {id: q, hint: Find}}], footer: [apply]}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let mut ids = super::super::ids::Ids::new(std::iter::empty());
    let expected = ids.widget("ptcslabel", "f--title", "widget").unwrap();
    assert_eq!(built.ids["f--title"], expected);
    let doc = crate::core::mashup::edit::parse(&built.content).unwrap();
    let title = doc.find_exact(&expected).unwrap();
    assert_eq!(doc.at(&title.path)["Properties"]["LabelText"], "Filter");
}

/// A root prop the spec sets to null is removed from the page container, not stored as null.
#[test]
fn null_root_props_remove_the_property() {
    let (_dir, solution) = element_solution();
    let spec = "version: 1\nrules: v1\npage:\n  name: Acme.Root_MU\n  template: blank\n  root: {props: {Margin: null, CustomClass: acme-page}}\n  slots:\n    center:\n      - label: {id: hello, text: Hello}\n";
    let built = compile(&solution, "Acme", spec).unwrap();
    let doc = crate::core::mashup::edit::parse(&built.content).unwrap();
    let center = doc.find("center").unwrap();
    let props = &doc.at(&center.path)["Properties"];
    assert!(props.get("Margin").is_none(), "{props}");
    assert_eq!(props["CustomClass"], "acme-page");
}
