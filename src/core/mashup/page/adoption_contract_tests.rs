//! Synthetic Composer shapes for contracts which depend on native widget configuration.

use super::*;
use serde_json::json;

const NAME: &str = "Acme.Hand_MU";
const EMPTY: &str = "version: 1\nrules: v1\npage:\n  name: Acme.Hand_MU\n  template: blank\n";

fn replace_content(solution: &Solution, name: &str, content: &Value) {
    let loaded = authoring::load_existing(solution, name).unwrap();
    let assets = mashup::Assets {
        content: mashup::to_sidecar(content),
        css: loaded.assets.css,
    };
    let entity = mashup::sync(&loaded.entity_before, &assets).unwrap().0;
    std::fs::write(
        solution.root.join(format!("acme/Mashups/{name}.xml")),
        entity,
    )
    .unwrap();
}

fn adopt_unchanged(solution: &Solution, name: &str) -> Value {
    let path = solution.root.join(format!("acme/Mashups/{name}.xml"));
    let before = std::fs::read(&path).unwrap();
    let derived = super::super::extract::extract(solution, name).unwrap();
    adopt::write_spec(solution, name, &derived.spec);
    let report = adopt::apply(solution, name);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "adoption changes entity bytes: {report}"
    );
    let again = super::super::build::plan(solution, name, None, None).unwrap();
    assert!(!again.conflicts(), "{}", again.report);
    assert_eq!(again.report["files"], json!([]), "{}", again.report);
    report
}

fn service(solution: &Solution, shape: Option<&str>) {
    let shape = shape
        .map(|shape| format!(" aspect.dataShape=\"{shape}\""))
        .unwrap_or_default();
    std::fs::write(solution.root.join("acme/Things/Acme.Data.xml"), format!(r#"<Entities><Things><Thing name="Acme.Data" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetRows"><ParameterDefinitions><FieldDefinition name="filter" baseType="STRING"/></ParameterDefinitions><ResultType baseType="INFOTABLE"{shape}/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#)).unwrap();
}

#[test]
fn result_field_binds_back_to_a_both_parameter_in_a_cycle() {
    let (_dir, solution) = element_solution();
    service(&solution, Some("Acme.Rows_DS"));
    let spec = format!("{EMPTY}  parameters: {{period: 'STRING:BOTH'}}\n  data:\n    rows: {{service: Acme.Data.GetRows, params: {{filter: {{bind: pick.SelectedText}}}}}}\n  slots:\n    center:\n      - widget: {{id: pick, type: ptcsdropdown, props: {{SelectedText: {{bind: param.period}}}}}}\n  bindings:\n    - {{from: rows.Name, to: param.period}}\n");
    adopt::handmade(&solution, &spec, NAME);
    let report = adopt_unchanged(&solution, NAME);
    assert_eq!(report["conflicts"], json!([]));
    let content = adopt::current(&solution, NAME);
    assert!(content["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|binding| binding["SourceArea"] == "Data"
            && binding["TargetArea"] == "Mashup"
            && binding["PropertyMaps"][0]["SourcePropertyType"] == "Field"));
}

#[test]
fn legacy_container_has_static_and_cached_dynamic_contracts() {
    let (_dir, solution) = element_solution();
    let spec = format!("{EMPTY}  layout: as-built\n  parameters: {{selected: 'STRING:BOTH'}}\n  slots:\n    center:\n      - widget:\n          id: mashupcontainer-3\n          type: mashupcontainer\n          props:\n            MashupParameters: [{{ParameterName: selected, BaseType: STRING, ParmDef: {{aspects: {{bindingDirection: BOTH}}}}}}]\n            selected: {{bind: param.selected}}\n            Name: {{bind: param.selected}}\n  bindings:\n    - {{from: mashupcontainer-3.selected, to: param.selected}}\n  events:\n    - {{on: load, invoke: mashupcontainer-3.Refresh}}\n");
    adopt::handmade(&solution, &spec, NAME);
    adopt_unchanged(&solution, NAME);
    let catalog = crate::core::widgets::catalog()
        .by_type("mashupcontainer")
        .unwrap();
    assert!(catalog.has_contract());
    assert_eq!(catalog.defaults["Width"], 400);
    assert_eq!(catalog.defaults["Height"], 300);
    assert!(catalog
        .services
        .as_ref()
        .unwrap()
        .iter()
        .any(|service| service == "AddGadgetToDashboard"));
}

#[test]
fn result_fields_resolve_system_cached_and_declared_shapes_but_refuse_unknown_ones() {
    let (_dir, solution) = element_solution();
    let spec = |field: &str| {
        format!("{EMPTY}  data:\n    rows: {{service: Acme.Data.GetRows}}\n  slots:\n    center:\n      - label: {{id: out, props: {{LabelText: {{bind: rows.{field}}}}}}}\n")
    };
    service(&solution, Some("MenuEntry"));
    let built = compile(&solution, "Acme", &spec("title")).unwrap();
    assert!(built.warnings.is_empty());
    for (field, base) in [
        ("menuId", "STRING"),
        ("parentMenuId", "STRING"),
        ("linkType", "STRING"),
        ("linkDestination", "STRING"),
        ("linkTarget", "STRING"),
        ("description", "STRING"),
        ("imageURL", "IMAGELINK"),
        ("isDefault", "BOOLEAN"),
        ("isDisabled", "BOOLEAN"),
        ("isHidden", "BOOLEAN"),
    ] {
        let built = compile(&solution, "Acme", &spec(field)).unwrap();
        let content: Value = serde_json::from_str(&built.content).unwrap();
        assert_eq!(
            content["DataBindings"][0]["PropertyMaps"][0]["SourcePropertyBaseType"], base,
            "{field}"
        );
    }
    assert!(compile(&solution, "Acme", &spec("unknown"))
        .unwrap_err()
        .to_string()
        .contains("MenuEntry has no field unknown"));
    adopt::handmade(&solution, &spec("title"), NAME);
    adopt_unchanged(&solution, NAME);

    service(&solution, Some("Acme.External_DS"));
    assert!(compile(&solution, "Acme", &spec("Name"))
        .unwrap_err()
        .to_string()
        .contains("not in the repository"));
    std::fs::create_dir_all(solution.root.join(".twaco")).unwrap();
    std::fs::write(
        solution.root.join(".twaco/platform.json"),
        r#"{"data_shapes":{"Acme.External_DS":{"Name":{"baseType":"STRING"}}}}"#,
    )
    .unwrap();
    assert!(compile(&solution, "Acme", &spec("Name"))
        .unwrap()
        .warnings
        .is_empty());
    assert!(compile(&solution, "Acme", &spec("unknown"))
        .unwrap_err()
        .to_string()
        .contains("has no field unknown"));
    std::fs::remove_file(solution.root.join(".twaco/platform.json")).unwrap();
    std::fs::write(
        solution.root.join("requirements.toml"),
        "[entities]\nDataShapes = ['Acme.External_DS']\n",
    )
    .unwrap();
    let built = compile(&solution, "Acme", &spec("Name")).unwrap();
    assert!(built
        .warnings
        .iter()
        .any(|warning| warning.contains("declared server-provided")));
}

#[test]
fn collection_global_properties_bind_both_ways_with_declared_types() {
    let (_dir, solution) = element_solution();
    let spec = format!("{EMPTY}  layout: as-built\n  parameters: {{manager: 'THINGNAME:BOTH', enabled: 'BOOLEAN:BOTH'}}\n  slots:\n    center:\n      - widget:\n          id: ptcscollection-3\n          type: ptcscollection\n          props:\n            MashupGlobalPropertyBinding: '{{\"manager\":\"THINGNAME\",\"enabled\":\"BOOLEAN\"}}'\n            manager: {{bind: param.manager}}\n            enabled: {{bind: param.enabled}}\n  bindings:\n    - {{from: ptcscollection-3.manager, to: param.manager}}\n    - {{from: ptcscollection-3.enabled, to: param.enabled}}\n");
    adopt::handmade(&solution, &spec, NAME);
    adopt_unchanged(&solution, NAME);
    let content = adopt::current(&solution, NAME);
    let maps: Vec<_> = content["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| b["PropertyMaps"].as_array().unwrap())
        .collect();
    assert!(maps
        .iter()
        .any(|m| m["SourceProperty"] == "manager" && m["SourcePropertyBaseType"] == "THINGNAME"));
    assert!(maps
        .iter()
        .any(|m| m["SourceProperty"] == "enabled" && m["SourcePropertyBaseType"] == "BOOLEAN"));
}

#[test]
fn dynamic_properties_use_the_same_typed_add_and_set_operations_as_commands() {
    let (_dir, solution) = element_solution();
    let spec = format!("{EMPTY}  slots:\n    center:\n      - widget:\n          id: cards\n          type: ptcscollection\n          props:\n            MashupGlobalPropertyBinding: {{manager: THINGNAME, enabled: BOOLEAN}}\n            manager: Acme.Manager\n            enabled: true\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    assert!(compile(
        &solution,
        "Acme",
        &spec.replace("enabled: true", "enabled: invalid")
    )
    .is_err());
    std::fs::write(
        solution.root.join("acme/Mashups/Acme.Hand_MU.xml"),
        &built.entity,
    )
    .unwrap();
    let request = authoring::SetRequest {
        mashup: NAME.into(),
        widget: built.ids["cards"].clone(),
        properties: vec![
            ("enabled".into(), json!(false)),
            ("manager".into(), json!("Acme.Other")),
        ],
        style_from: None,
        styles: vec![],
    };
    let plan = authoring::plan_set(&solution, &request).unwrap();
    assert!(plan
        .changed
        .iter()
        .any(|change| change.property == "enabled" && change.after == false));
    let mut invalid = request;
    invalid.properties = vec![("enabled".into(), json!("invalid"))];
    assert!(authoring::plan_set(&solution, &invalid).is_err());
}

fn menu_spec(bound: bool) -> String {
    let data = if bound {
        "{bind: rows}"
    } else {
        "{rows: [{menuId: delete, title: Delete}]}"
    };
    format!("{EMPTY}  data:\n    rows: {{service: Acme.Data.GetRows}}\n  parameters: {{flag: BOOLEAN}}\n  slots:\n    center:\n      - widget:\n          id: ptcsmenubutton-3\n          type: ptcsmenubutton\n          props:\n            ItemConfigurationMode: widgetProperties\n            Data: {data}\n            delete_Visible: {{bind: param.flag}}\n            delete_Disabled: false\n  events:\n    - {{on: ptcsmenubutton-3.delete_Clicked, invoke: rows}}\n")
}

#[test]
fn menu_static_items_declare_events_and_properties_and_reject_unknown_ids() {
    let (_dir, solution) = element_solution();
    let spec = menu_spec(false);
    let built = compile(&solution, "Acme", &spec).unwrap();
    assert!(built.warnings.is_empty());
    assert!(compile(
        &solution,
        "Acme",
        &spec.replace("delete_Clicked", "missing_Clicked")
    )
    .unwrap_err()
    .to_string()
    .contains("no event missing_Clicked"));
    assert!(compile(
        &solution,
        "Acme",
        &spec.replace("delete_Visible", "missing_Visible")
    )
    .is_err());
    let from_control = format!(
        "{spec}  bindings:\n    - {{from: ptcsmenubutton-3.delete_Visible, to: param.flag}}\n"
    );
    assert!(compile(&solution, "Acme", &from_control)
        .unwrap_err()
        .to_string()
        .contains("not a source property"));
    adopt::handmade(&solution, &spec, NAME);
    adopt_unchanged(&solution, NAME);
}

#[test]
fn menu_bound_data_keeps_uncheckable_item_wiring_with_a_warning() {
    let (_dir, solution) = element_solution();
    let spec = menu_spec(true);
    let built = compile(&solution, "Acme", &spec).unwrap();
    assert!(built
        .warnings
        .iter()
        .any(|w| w.contains("per-item ids cannot be checked")));
    adopt::handmade(&solution, &spec, NAME);
    let report = adopt_unchanged(&solution, NAME);
    assert!(report["cell_notices"].as_array().unwrap().iter().any(|w| w
        .as_str()
        .unwrap()
        .contains("per-item ids cannot be checked")));
}

#[test]
fn menu_bound_data_takes_precedence_over_static_initial_rows_without_reordering_bindings() {
    let (_dir, solution) = element_solution();
    let spec = format!(
        "{}  bindings:\n    - {{from: rows, to: ptcsmenubutton-3.Data}}\n",
        menu_spec(false).replace("menuId: delete", "menuId: archive")
    );
    let built = compile(&solution, "Acme", &spec).unwrap();
    let content: Value = serde_json::from_str(&built.content).unwrap();
    assert_eq!(
        content["DataBindings"].as_array().unwrap().last().unwrap()["PropertyMaps"][0]
            ["TargetProperty"],
        "Data"
    );
    adopt::handmade(&solution, &spec, NAME);
    let report = adopt_unchanged(&solution, NAME);
    assert!(report["cell_notices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .unwrap()
            .contains("per-item ids cannot be checked")));
}

#[test]
fn logout_and_catalogue_functions_stay_beside_the_layout() {
    let (_dir, solution) = element_solution();
    let spec = format!("{EMPTY}  slots:\n    center:\n      - button: {{id: logout, text: Logout}}\n      - widget: {{id: signout, type: logoutfunction, props: {{URL: /signed-out}}}}\n      - widget: {{id: confirm, type: ptcsconfirmation, props: {{TitleText: Acme confirmation}}}}\n  events:\n    - {{on: logout.Clicked, invoke: signout.Logout}}\n    - {{on: logout.Clicked, invoke: confirm.OpenConfirmation}}\n");
    adopt::handmade(&solution, &spec, NAME);
    let before = adopt::current(&solution, NAME);
    assert!(before["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|widget| widget["Properties"]["Type"] == "logoutfunction"));
    let confirmation = before["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|widget| widget["Properties"]["Type"] == "ptcsconfirmation")
        .expect("confirmation is a top-level function");
    assert_eq!(confirmation["Properties"]["UseTheme"], true);
    adopt_unchanged(&solution, NAME);
}

#[test]
fn parameter_descriptions_survive_both_xml_and_composer_cache() {
    let (_dir, solution) = element_solution();
    let spec = format!("{EMPTY}  parameters:\n    section: {{type: STRING, direction: BOTH, description: 'A section <title> & description'}}\n    rows: {{type: INFOTABLE, data_shape: Acme.Rows_DS, direction: OUT, description: Rows}}\n");
    adopt::handmade(&solution, &spec, NAME);
    let extraction = super::super::extract::extract(&solution, NAME).unwrap();
    let spec: Value = serde_saphyr::from_str(&extraction.spec).unwrap();
    assert_eq!(
        spec["page"]["parameters"]["section"]["description"],
        "A section <title> & description"
    );
    adopt_unchanged(&solution, NAME);
    let xml = std::fs::read_to_string(solution.root.join("acme/Mashups/Acme.Hand_MU.xml")).unwrap();
    assert!(xml.contains("A section &lt;title&gt; &amp; description"));
    let content = adopt::current(&solution, NAME);
    assert!(content["UI"]["Properties"]["_currentParameterDefs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["Description"] == "A section <title> & description"));
    let short = compile(
        &solution,
        "Acme",
        &format!("{EMPTY}  parameters: {{section: 'STRING:BOTH'}}\n"),
    )
    .unwrap();
    let long = compile(
        &solution,
        "Acme",
        &format!("{EMPTY}  parameters: {{section: {{type: STRING, direction: BOTH}}}}\n"),
    )
    .unwrap();
    assert_eq!(short, long);
    assert!(compile(
        &solution,
        "Acme",
        &format!("{EMPTY}  parameters: {{rows: {{type: INFOTABLE}}}}\n")
    )
    .unwrap_err()
    .to_string()
    .contains("needs data_shape"));
    assert!(compile(
        &solution,
        "Acme",
        &format!("{EMPTY}  parameters: {{section: {{type: STRING, mystery: true}}}}\n")
    )
    .is_err());
}

#[test]
fn orphan_contained_parameter_values_warn_and_keep_the_entity_bytes() {
    let (_dir, solution) = element_solution();
    let spec = format!(
        "{EMPTY}  slots:\n    center:\n      - embed: {{id: child, mashup: Acme.Child_MU}}\n"
    );
    adopt::handmade(&solution, &spec, NAME);
    let mut content = adopt::current(&solution, NAME);
    fn orphan(widget: &mut Value) {
        if widget["Properties"]["Type"] == "mashupcontainer" {
            widget["Properties"]["Region"] = json!("Europe");
        }
        for child in widget["Widgets"].as_array_mut().into_iter().flatten() {
            orphan(child);
        }
    }
    orphan(&mut content["UI"]);
    replace_content(&solution, NAME, &content);
    let report = adopt_unchanged(&solution, NAME);
    assert!(
        !report["cell_notices"].as_array().unwrap().iter().any(|w| w
            .as_str()
            .unwrap()
            .contains("orphan contained-mashup parameter value MashupParameters")),
        "{report}"
    );
    assert!(
        report["cell_notices"].as_array().unwrap().iter().any(|w| w
            .as_str()
            .unwrap()
            .contains("orphan contained-mashup parameter value Region")),
        "{report}"
    );
}

#[test]
fn result_infotables_without_shapes_warn_and_keep_whole_and_field_bindings() {
    let (_dir, solution) = element_solution();
    service(&solution, None);
    let spec = format!("{EMPTY}  data:\n    rows: {{service: Acme.Data.GetRows, params: {{filter: {{bind: rows.SelectedRows.UID}}}}}}\n  slots:\n    center:\n      - widget: {{id: table, type: ptcsgrid, props: {{Data: {{bind: rows}}}}}}\n      - label: {{id: uid, props: {{LabelText: {{bind: rows.UID}}}}}}\n");
    let built = compile(&solution, "Acme", &spec).unwrap();
    assert!(built
        .warnings
        .iter()
        .any(|w| w.contains("UID cannot be type-checked")));
    assert!(built
        .warnings
        .iter()
        .any(|w| w.contains("whole INFOTABLE bindings are supported")));
    adopt::handmade(&solution, &spec, NAME);
    let report = adopt_unchanged(&solution, NAME);
    assert!(!report["cell_notices"].as_array().unwrap().is_empty());
}
