use super::new::new_document;
use super::*;
use crate::core::mashup::describe::mashup_parameters;
use crate::core::mashup::edit::Placement;
use crate::core::mashup::templates::Visibility;
use serde_json::json;
use std::path::Path;

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn with_config(config: &str) -> (tempfile::TempDir, Solution) {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "twaco.toml", config);
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution)
}

fn one_project() -> (tempfile::TempDir, Solution) {
    with_config("[[project]]\nname = \"Acme\"\nroot = \"acme\"\n")
}

fn request(name: &str) -> NewRequest {
    NewRequest {
        name: name.to_string(),
        project: None,
        template: "blank".into(),
        primary_menu: None,
        secondary_menu: None,
        theme: None,
        master: false,
        visibility: Vec::new(),
        params: Vec::new(),
        tabs: Vec::new(),
    }
}

fn lock(solution: &Solution) -> WorkspaceLock {
    crate::core::lock::acquire_for(solution, "test").unwrap()
}

#[test]
fn a_blank_mashup_is_three_files_that_sync_agrees_with() {
    let (_dir, solution) = one_project();
    let plan = plan_new(&solution, &request("Acme.Orders_MU")).unwrap();
    assert_eq!(plan.project, "Acme");
    assert!(plan
        .entity_path
        .ends_with("acme/Mashups/Acme.Orders_MU.xml"));
    assert!(plan
        .content_path
        .ends_with("src/Acme.Orders_MU/mashup/content.json"));
    apply_new(&solution, &plan, &lock(&solution)).unwrap();

    let written = std::fs::read(&plan.entity_path).unwrap();
    let assets = workspace::read_mashup(plan.content_path.parent().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(assets, plan.assets);
    // A sync of what is on disk is the identity: the new mashup is already in sync.
    let (synced, changed) = mashup::sync(&written, &assets).unwrap();
    assert!(changed.is_empty(), "{changed:?}");
    assert_eq!(synced, written);
    let text = String::from_utf8(written).unwrap();
    assert!(text.contains("name=\"Acme.Orders_MU\""));
    assert!(text.contains("projectName=\"Acme\""));
    assert!(text.contains("<Visibility></Visibility>"));
    let document = mashup::edit::parse(&assets.content).unwrap();
    assert_eq!(
        document.value()["UI"]["Properties"]["StyleTheme"],
        DEFAULT_THEME
    );
    assert_eq!(document.find("center").unwrap().id, "flexcontainer-2");
    // The entity is one the workspace now finds.
    assert!(workspace::entities(&solution)
        .iter()
        .any(|e| e.info.collection == "Mashups" && e.info.name == "Acme.Orders_MU"));
}

#[test]
fn parameters_are_written_to_the_entity_and_content_together() {
    let (_dir, solution) = one_project();
    let mut request = request("Acme.Parameter_MU");
    request.params = vec!["id:LONG:IN".into(), "title:STRING".into()];
    let plan = plan_new(&solution, &request).unwrap();
    let xml = String::from_utf8(plan.entity.clone()).unwrap();
    assert!(xml.contains(r#"<FieldDefinition aspect.bindingDirection="IN" aspect.isMandatory="false" baseType="LONG" description="" name="id" ordinal="0"/>"#));
    let value: Value = serde_json::from_str(&plan.assets.content).unwrap();
    assert_eq!(
        value["UI"]["Properties"]["_currentParameterDefs"][0]["ParameterName"],
        "id"
    );
    assert_eq!(mashup_parameters(&plan.entity).unwrap().len(), 2);
    apply_new(&solution, &plan, &lock(&solution)).unwrap();
    let parameter = ParameterRequest {
        mashup: "Acme.Parameter_MU".into(),
        action: ParameterAction::Add,
        name: "header".into(),
        base_type: Some("STRING".into()),
        direction: None,
        description: None,
        default: ParameterDefault::Set("Orders".into()),
        data_shape: None,
        force: false,
    };
    let update = plan_parameter(&solution, &parameter).unwrap();
    apply_parameter(&solution, &update, &lock(&solution)).unwrap();
    let content = workspace::read_mashup(&solution.src_root().join("Acme.Parameter_MU/mashup"))
        .unwrap()
        .unwrap()
        .content;
    let value: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["UI"]["Properties"]["header"], "Orders");
    assert_eq!(
        value["UI"]["Properties"]["_currentParameterDefs"][2]["DefaultValue"],
        "Orders"
    );
}

#[test]
fn a_master_mashup_has_a_master_root_and_a_page_container_filling_the_center() {
    let (_dir, solution) = one_project();
    let plan = plan_new(
        &solution,
        &NewRequest {
            master: true,
            ..request("Acme.Main_MM")
        },
    )
    .unwrap();
    let text = String::from_utf8(plan.entity.clone()).unwrap();
    assert!(text.contains("aspect.mashupType=\"target\""));
    let document = mashup::edit::parse(&plan.assets.content).unwrap();
    let root = &document.value()["UI"]["Properties"];
    assert_eq!(root["Type"], "targetmashup");
    assert_eq!(root["id_index"], 3);
    assert_eq!(document.value()["mashupType"], "targetmashup");
    let page = document.find("Page Mashup").unwrap();
    assert_eq!(page.id, "pagemashupcontainer-3");
    assert_eq!(
        document.at(&page.path)["Properties"]["ResponsiveLayout"],
        true
    );
    assert_eq!(page.path, [0, 0], "inside center");
}

#[test]
fn layouts_have_the_composer_tree_and_masters_put_pages_in_center() {
    for (layout, expected) in [
        (
            "sidebar",
            vec![
                ("flexcontainer-2", "flexcontainer", "page", "row"),
                ("flexcontainer-3", "flexcontainer", "sidebar", "row"),
                ("ptcsmenubar-4", "ptcsmenubar", "ptcs-menubar-4", ""),
                ("flexcontainer-5", "flexcontainer", "center", "column"),
            ],
        ),
        (
            "header-footer",
            vec![
                ("flexcontainer-2", "flexcontainer", "page", "column"),
                ("flexcontainer-3", "flexcontainer", "header", "row"),
                ("flexcontainer-4", "flexcontainer", "center", "column"),
                ("flexcontainer-5", "flexcontainer", "footer", "row"),
            ],
        ),
    ] {
        for master in [false, true] {
            let template = mashup::templates::layout(layout).unwrap().unwrap();
            let document = new_document(&template, master, DEFAULT_THEME).unwrap();
            let tree: Vec<(String, String, String, String)> = document
                .widgets()
                .into_iter()
                .skip(1)
                .map(|widget| {
                    let properties = &document.at(&widget.path)["Properties"];
                    (
                        widget.id,
                        widget.type_id,
                        properties["DisplayName"].as_str().unwrap().into(),
                        properties["flex-direction"].as_str().unwrap_or("").into(),
                    )
                })
                .collect();
            let mut expected: Vec<(String, String, String, String)> = expected
                .iter()
                .map(|(id, type_id, name, direction)| {
                    (
                        (*id).into(),
                        (*type_id).into(),
                        (*name).into(),
                        (*direction).into(),
                    )
                })
                .collect();
            if master {
                let at = if layout == "sidebar" { 4 } else { 3 };
                expected.insert(
                    at,
                    (
                        "pagemashupcontainer-6".into(),
                        "pagemashupcontainer".into(),
                        "Page Mashup".into(),
                        "".into(),
                    ),
                );
                let page = document.find("Page Mashup").unwrap();
                assert_eq!(page.path[..page.path.len() - 1], [0, 1]);
            }
            assert_eq!(tree, expected);
        }
    }
}

#[test]
fn every_embedded_template_validates_and_its_fixed_nodes_follow_the_parent_axis() {
    for template in mashup::templates::layouts().unwrap() {
        validate_template(&template).unwrap();
    }
    for (name, expected) in [
        (
            "header-sidebar",
            vec![("flexcontainer-3", "header", "height")],
        ),
        (
            "form-dialog",
            vec![
                ("flexcontainer-3", "title", "height"),
                ("flexcontainer-5", "actions", "height"),
            ],
        ),
        (
            "card",
            vec![
                ("flexcontainer-3", "header", "height"),
                ("flexcontainer-5", "actions", "width"),
            ],
        ),
        (
            "master-detail",
            vec![("flexcontainer-4", "divider-4", "width")],
        ),
        ("kpi-tile", vec![("flexcontainer-4", "divider-4", "height")]),
    ] {
        let template = mashup::templates::layout(name).unwrap().unwrap();
        let document = new_document(&template, false, DEFAULT_THEME).unwrap();
        let expected_tree = match name {
            "header-sidebar" => vec![
                ("flexcontainer-2", "flexcontainer", "page"),
                ("flexcontainer-3", "flexcontainer", "header"),
                ("flexcontainer-4", "flexcontainer", "body"),
                ("flexcontainer-5", "flexcontainer", "sidebar"),
                ("ptcsmenubar-6", "ptcsmenubar", "ptcs-menubar-6"),
                ("flexcontainer-7", "flexcontainer", "center"),
            ],
            "form-dialog" => vec![
                ("flexcontainer-2", "flexcontainer", "page"),
                ("flexcontainer-3", "flexcontainer", "title"),
                ("flexcontainer-4", "flexcontainer", "fields"),
                ("flexcontainer-5", "flexcontainer", "actions"),
            ],
            "card" => vec![
                ("flexcontainer-2", "flexcontainer", "page"),
                ("flexcontainer-3", "flexcontainer", "header"),
                ("flexcontainer-4", "flexcontainer", "title"),
                ("flexcontainer-5", "flexcontainer", "actions"),
                ("flexcontainer-6", "flexcontainer", "body"),
            ],
            "master-detail" => vec![
                ("flexcontainer-2", "flexcontainer", "page"),
                ("flexcontainer-3", "flexcontainer", "list"),
                ("flexcontainer-4", "flexcontainer", "divider-4"),
                ("flexcontainer-5", "flexcontainer", "detail"),
            ],
            "kpi-tile" => vec![
                ("flexcontainer-2", "flexcontainer", "page"),
                ("flexcontainer-3", "flexcontainer", "title"),
                ("flexcontainer-4", "flexcontainer", "divider-4"),
                ("flexcontainer-5", "flexcontainer", "value"),
            ],
            _ => unreachable!("only requested templates"),
        };
        assert_eq!(
            document
                .widgets()
                .into_iter()
                .skip(1)
                .map(|widget| (widget.id, widget.type_id, widget.display_name))
                .collect::<Vec<_>>(),
            expected_tree
                .into_iter()
                .map(|(id, type_id, display_name)| {
                    (id.into(), type_id.into(), display_name.into())
                })
                .collect::<Vec<_>>(),
            "{name}"
        );
        for (id, slot, axis) in expected {
            let widget = document.find_exact(id).unwrap();
            let properties = &document.at(&widget.path)["Properties"];
            assert_eq!(properties["DisplayName"], slot, "{name}: {id}");
            assert_eq!(properties["flex-size"], "fixed", "{name}: {id}");
            assert!(
                properties[format!("flex-min-{axis}")].is_string(),
                "{name}: {id}"
            );
            assert!(
                properties[format!("flex-max-{axis}")].is_string(),
                "{name}: {id}"
            );
        }
        assert!(document.problems().is_empty(), "{name}");
    }
}

#[test]
fn masters_use_each_template_master_slot() {
    for (name, slot) in [("form-dialog", "fields"), ("header-sidebar", "center")] {
        let template = mashup::templates::layout(name).unwrap().unwrap();
        let document = new_document(&template, true, DEFAULT_THEME).unwrap();
        let page = document.find("Page Mashup").unwrap();
        let container = document.find(slot).unwrap();
        assert_eq!(page.path[..page.path.len() - 1], container.path, "{name}");
    }
}

#[test]
fn add_in_a_template_slot_places_the_widget_in_that_container() {
    let (_dir, solution) = one_project();
    let new = plan_new(
        &solution,
        &NewRequest {
            template: "form-dialog".into(),
            ..request("Acme.Edit_MU")
        },
    )
    .unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    let added = plan_add(
        &solution,
        &AddRequest {
            mashup: "Acme.Edit_MU".into(),
            widget: "button".into(),
            inputs: Vec::new(),
            placement: Some(Placement::In("actions".into())),
            label: None,
            properties: Vec::new(),
        },
    )
    .unwrap();
    let content = String::from_utf8(added.edit.sidecars.unwrap().content_after).unwrap();
    let document = mashup::edit::parse(&content).unwrap();
    let button = document.find_exact(&added.placed.id).unwrap();
    assert_eq!(
        button.path[..button.path.len() - 1],
        document.find("actions").unwrap().path
    );
}

#[test]
fn sidebar_menus_are_service_calls_bound_and_loaded_like_bind_and_on() {
    let (dir, solution) = one_project();
    write(
        dir.path(),
        "acme/Things/Acme.Nav.Manager.xml",
        r#"<Entities><Things><Thing name="Acme.Nav.Manager" projectName="Acme"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetPrimaryMenu"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Nav.Menu_DS"/></ServiceDefinition>
            <ServiceDefinition name="GetSecondaryMenu"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Nav.Menu_DS"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    );
    write(
        dir.path(),
        "acme/DataShapes/Acme.Nav.Menu_DS.xml",
        r#"<Entities><DataShapes><DataShape name="Acme.Nav.Menu_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="title" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
    );
    let plan = plan_new(
        &solution,
        &NewRequest {
            template: "sidebar".into(),
            primary_menu: Some("data:Acme.Nav.Manager.GetPrimaryMenu".into()),
            secondary_menu: Some("data:Acme.Nav.Manager.GetSecondaryMenu".into()),
            ..request("Acme.Nav_MU")
        },
    )
    .unwrap();
    assert_eq!(
        plan.menus,
        [
            (
                "data:Acme.Nav.Manager.GetPrimaryMenu".into(),
                "PrimaryNavData".into()
            ),
            (
                "data:Acme.Nav.Manager.GetSecondaryMenu".into(),
                "SecondaryNavData".into()
            ),
        ]
    );
    apply_new(&solution, &plan, &lock(&solution)).unwrap();
    let content: Value = serde_json::from_str(&plan.assets.content).unwrap();
    assert_eq!(
        content["Data"]["Things_Acme.Nav.Manager"]["Services"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let bindings: Vec<&str> = content["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|binding| {
            binding["PropertyMaps"][0]["TargetProperty"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(bindings, ["PrimaryNavData", "SecondaryNavData"]);
    assert_eq!(content["Events"].as_array().unwrap().len(), 2);
    let report = crate::core::check::run(&solution);
    let failing: Vec<_> = report
        .gates
        .iter()
        // The fixture declares service definitions only, which the project gate reports.
        .filter(|gate| !gate.ok() && gate.name != "project")
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
    let description = crate::core::mashup::describe::describe(&solution, "Acme.Nav_MU").unwrap();
    assert_eq!(
        description
            .bindings
            .iter()
            .map(|binding| (binding.from.as_str(), binding.to.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                "data:Acme.Nav.Manager.GetPrimaryMenu",
                "ui:ptcsmenubar-4.PrimaryNavData",
            ),
            (
                "data:Acme.Nav.Manager.GetSecondaryMenu",
                "ui:ptcsmenubar-4.SecondaryNavData",
            ),
        ]
    );
    assert_eq!(
        description
            .events
            .iter()
            .map(|event| (event.trigger.as_str(), event.handler.as_str()))
            .collect::<Vec<_>>(),
        [
            ("mashup:Loaded", "data:Acme.Nav.Manager.GetPrimaryMenu"),
            ("mashup:Loaded", "data:Acme.Nav.Manager.GetSecondaryMenu"),
        ]
    );
}

#[test]
fn sidebar_menu_requests_refuse_missing_wrong_or_non_sidebar_menus() {
    let (dir, solution) = one_project();
    write(
        dir.path(),
        "acme/Things/Acme.Nav.Manager.xml",
        r#"<Entities><Things><Thing name="Acme.Nav.Manager" projectName="Acme"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetMenu"><ResultType baseType="STRING"/></ServiceDefinition>
            <ServiceDefinition name="GetTable"><ParameterDefinitions><FieldDefinition name="filter" baseType="STRING"/></ParameterDefinitions><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Nav.Menu_DS"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    );
    write(
        dir.path(),
        "acme/DataShapes/Acme.Nav.Menu_DS.xml",
        r#"<Entities><DataShapes><DataShape name="Acme.Nav.Menu_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="title" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
    );
    let error = plan_new(
        &solution,
        &NewRequest {
            template: "sidebar".into(),
            ..request("Acme.Nav_MU")
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    let invalid = |primary_menu: &str| {
        plan_new(
            &solution,
            &NewRequest {
                template: "sidebar".into(),
                primary_menu: Some(primary_menu.into()),
                ..request("Acme.Nav_MU")
            },
        )
        .unwrap_err()
    };
    assert!(invalid("data:Acme.Nav.Manager.GetMenu")
        .to_string()
        .contains("menu service must return an INFOTABLE"));
    assert_eq!(
        invalid("data:Acme.Nav.Manager.GetTable.filter").code(),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        invalid("data:Acme.Nav.Manager.GetTable[SelectedRows]").code(),
        ErrorCode::InvalidArguments
    );
    let error = plan_new(
        &solution,
        &NewRequest {
            template: "header-footer".into(),
            primary_menu: Some("data:Acme.Nav.Manager.GetTable".into()),
            ..request("Acme.Nav_MU")
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert!(mashup::templates::layout("stacked").unwrap().is_none());
}

#[test]
fn a_theme_must_be_the_default_or_in_the_solution() {
    let (dir, solution) = one_project();
    let error = plan_new(
        &solution,
        &NewRequest {
            theme: Some("Acme.Theme".into()),
            ..request("Acme.Orders_MU")
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnknownEntity);
    write(
            dir.path(),
            "acme/StyleThemes/Acme.Theme.xml",
            "<Entities><StyleThemes><StyleTheme name=\"Acme.Theme\" projectName=\"Acme\"></StyleTheme></StyleThemes></Entities>",
        );
    let plan = plan_new(
        &solution,
        &NewRequest {
            theme: Some("Acme.Theme".into()),
            ..request("Acme.Orders_MU")
        },
    )
    .unwrap();
    assert!(plan
        .assets
        .content
        .contains("\"StyleTheme\": \"Acme.Theme\""));
}

#[test]
fn visibility_is_only_what_is_asked_and_must_be_well_formed() {
    assert_eq!(
        parse_visibility("OrganizationalUnit/Acme.Org:Acme.Users").unwrap(),
        Visibility {
            kind: "OrganizationalUnit".into(),
            name: "Acme.Org:Acme.Users".into()
        }
    );
    for bad in [
        "Acme.Org",
        "User/Administrator",
        "Organization/",
        "Organization/Acme:X",
        "OrganizationalUnit/Acme.Org",
        "OrganizationalUnit/:x",
    ] {
        assert!(parse_visibility(bad).is_err(), "{bad}");
    }
    let (_dir, solution) = one_project();
    let plan = plan_new(
        &solution,
        &NewRequest {
            visibility: vec!["Organization/Acme.Org".into()],
            ..request("Acme.Orders_MU")
        },
    )
    .unwrap();
    let text = String::from_utf8(plan.entity).unwrap();
    assert!(text.contains("name=\"Acme.Org\"\n                     type=\"Organization\""));
}

#[test]
fn an_existing_mashup_or_file_and_an_unclear_project_are_refused() {
    let (dir, solution) = one_project();
    write(
            dir.path(),
            "acme/Mashups/Acme.Orders_MU.xml",
            "<Entities><Mashups><Mashup name=\"Acme.Orders_MU\" projectName=\"Acme\"><mashupContent><![CDATA[{}]]></mashupContent></Mashup></Mashups></Entities>",
        );
    let error = plan_new(&solution, &request("Acme.Orders_MU")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::AlreadyExists, "{error}");
    // A leftover sidecar folder is not overwritten either.
    write(dir.path(), "src/Acme.Other_MU/mashup/content.json", "{}");
    let error = plan_new(&solution, &request("Acme.Other_MU")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::AlreadyExists, "{error}");

    let (_two, two) = with_config(
            "[[project]]\nname = \"Acme\"\nroot = \"acme\"\n\n[[project]]\nname = \"Acme.UI\"\nroot = \"ui\"\n",
        );
    let error = plan_new(&two, &request("Acme.Orders_MU")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert!(error.to_string().contains("--project"), "{error}");
    let error = plan_new(
        &two,
        &NewRequest {
            project: Some("Nope".into()),
            ..request("Acme.Orders_MU")
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnknownEntity);
    let plan = plan_new(
        &two,
        &NewRequest {
            project: Some("Acme.UI".into()),
            ..request("Acme.Orders_MU")
        },
    )
    .unwrap();
    assert!(plan.entity_path.ends_with("ui/Mashups/Acme.Orders_MU.xml"));
    let error = plan_new(&two, &request("Acme/Orders")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
}

#[test]
fn a_name_that_cannot_be_a_file_name_everywhere_is_refused_when_planning() {
    let (_dir, solution) = one_project();
    for name in [
        "Acme?Orders_MU",
        "Acme*Orders",
        "CON.Orders_MU",
        "Acme|X",
        "Acme.Orders.",
    ] {
        let error = plan_new(&solution, &request(name)).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArguments, "{name}: {error}");
    }
}

#[test]
fn a_file_where_a_folder_goes_is_refused_when_planning() {
    let (dir, solution) = one_project();
    write(dir.path(), "src/Acme.Orders_MU", "not a folder");
    let error = plan_new(&solution, &request("Acme.Orders_MU")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::AlreadyExists, "{error}");
    assert!(error.to_string().contains("needs a folder"), "{error}");
}

#[test]
fn a_new_mashup_passes_every_gate() {
    let (_dir, solution) = one_project();
    for request in [
        request("Acme.Orders_MU"),
        NewRequest {
            master: true,
            ..request("Acme.Main_MM")
        },
    ] {
        let plan = plan_new(&solution, &request).unwrap();
        apply_new(&solution, &plan, &lock(&solution)).unwrap();
    }
    let report = crate::core::check::run(&solution);
    let failing: Vec<_> = report
        .gates
        .iter()
        .filter(|gate| !gate.ok())
        .map(|gate| format!("{}: {:?} {:?}", gate.name, gate.findings, gate.broken))
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
}

#[test]
fn a_file_appearing_after_the_plan_stops_the_write_and_nothing_is_left() {
    let (dir, solution) = one_project();
    let plan = plan_new(&solution, &request("Acme.Orders_MU")).unwrap();
    write(
        dir.path(),
        "src/Acme.Orders_MU/mashup/custom.css",
        "body {}\n",
    );
    let error = apply_new(&solution, &plan, &lock(&solution)).unwrap_err();
    assert!(matches!(error, AuthoringError::Write(_)), "{error}");
    assert!(!plan.entity_path.exists());
    assert!(!plan.content_path.exists());
}

fn add_request(mashup: &str, widget: &str) -> AddRequest {
    AddRequest {
        mashup: mashup.to_string(),
        widget: widget.to_string(),
        inputs: Vec::new(),
        placement: None,
        label: None,
        properties: Vec::new(),
    }
}

#[test]
fn adding_widgets_keeps_sidecars_and_the_rest_of_the_entity_identical() {
    let (_dir, solution) = one_project();
    let name = "Acme.Orders_MU";
    let new = plan_new(&solution, &request(name)).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    let grid = plan_add(&solution, &add_request(name, "grid")).unwrap();
    assert_eq!(grid.placed.id, "ptcsgrid-3");
    apply_edit(&solution, &grid.edit, &lock(&solution)).unwrap();
    let before_button = std::fs::read(&grid.edit.entity_path).unwrap();
    let mut button_request = add_request(name, "button");
    button_request.placement = Some(Placement::Beside {
        anchor: grid.placed.id.clone(),
        side: mashup::edit::Side::Below,
    });
    button_request.label = Some("Save".into());
    button_request.properties = vec![("ButtonType".into(), "secondary".into())];
    let button = plan_add(&solution, &button_request).unwrap();
    apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
    let entity = std::fs::read(&button.edit.entity_path).unwrap();
    let assets = workspace::read_mashup(new.content_path.parent().unwrap())
        .unwrap()
        .unwrap();
    assert!(mashup::sync(&entity, &assets).unwrap().1.is_empty());
    let document = mashup::edit::parse(&assets.content).unwrap();
    let properties = &document.at(&document.find(&button.placed.id).unwrap().path)["Properties"];
    assert_eq!(properties["Label"], "Save");
    assert_eq!(properties["ButtonType"], "secondary");
    let content = |bytes: &[u8]| {
        let start = bytes
            .windows(b"<mashupContent".len())
            .position(|part| part == b"<mashupContent")
            .unwrap();
        let end = bytes
            .windows(b"</mashupContent>".len())
            .position(|part| part == b"</mashupContent>")
            .unwrap()
            + b"</mashupContent>".len();
        (bytes[..start].to_vec(), bytes[end..].to_vec())
    };
    assert_eq!(content(&before_button), content(&entity));
    assert!(crate::core::check::run(&solution).ok());
}

#[test]
fn adding_refuses_drift_and_edits_an_unmanaged_mashup_only_in_its_entity() {
    let (dir, solution) = one_project();
    let name = "Acme.Orders_MU";
    let new = plan_new(&solution, &request(name)).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    write(dir.path(), "src/Acme.Orders_MU/mashup/content.json", "{}\n");
    let error = plan_add(&solution, &add_request(name, "grid")).unwrap_err();
    assert!(matches!(error, AuthoringError::Refused(_)));
    let assets = mashup::extract(&std::fs::read(&new.entity_path).unwrap()).unwrap();
    std::fs::write(&new.content_path, assets.content).unwrap();
    std::fs::remove_dir_all(new.content_path.parent().unwrap()).unwrap();
    let plan = plan_add(&solution, &add_request(name, "grid")).unwrap();
    assert!(plan.edit.sidecars.is_none());
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    assert!(!new.content_path.exists());
}

#[test]
fn add_validates_widget_properties_and_labels() {
    let (_dir, solution) = one_project();
    let name = "Acme.Orders_MU";
    let new = plan_new(&solution, &request(name)).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    for (widget, property) in [
        ("grid", ("Nope", "x")),
        ("grid", ("Visible", "TRUE")),
        ("button", ("ButtonType", "wrong")),
        ("button", ("Id", "x")),
        ("toolbar", ("CustomBaseTypesMapping", "x")),
    ] {
        let mut request = add_request(name, widget);
        request.properties = vec![(property.0.into(), property.1.into())];
        assert!(matches!(
            plan_add(&solution, &request),
            Err(AuthoringError::Invalid(_))
        ));
    }
    let mut label = add_request(name, "flexcontainer");
    label.label = Some("No label".into());
    assert!(matches!(
        plan_add(&solution, &label),
        Err(AuthoringError::Invalid(_))
    ));
    assert!(matches!(
        plan_add(&solution, &add_request(name, "nope")),
        Err(AuthoringError::Invalid(_))
    ));
    assert!(matches!(
        plan_add(&solution, &add_request("Acme.Nope_MU", "grid")),
        Err(AuthoringError::Unknown(_))
    ));
}

#[test]
fn binding_a_service_result_creates_a_thing_call_and_composer_binding() {
    let (dir, solution) = one_project();
    write(
        dir.path(),
        "acme/Things/Acme.Orders.Manager.xml",
        r#"<Entities><Things><Thing name="Acme.Orders.Manager" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    );
    write(
        dir.path(),
        "acme/DataShapes/Acme.Orders.Order_DS.xml",
        r#"<Entities><DataShapes><DataShape name="Acme.Orders.Order_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="id" baseType="LONG"/><FieldDefinition name="note" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
    );
    let name = "Acme.Orders_MU";
    let new = plan_new(&solution, &request(name)).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    let grid = plan_add(&solution, &add_request(name, "grid")).unwrap();
    apply_edit(&solution, &grid.edit, &lock(&solution)).unwrap();
    let plan = plan_bind(
        &solution,
        &BindRequest {
            mashup: name.into(),
            from: "data:Acme.Orders.Manager.GetOrders".into(),
            to: "ui:ptcsgrid-3.Data".into(),
        },
    )
    .unwrap();
    assert_eq!(
        plan.new_source.as_deref(),
        Some("Acme.Orders.Manager.GetOrders")
    );
    let content = plan.edit.sidecars.as_ref().unwrap().content_after.clone();
    let value: Value = serde_json::from_slice(&content).unwrap();
    let call = &value["Data"]["Things_Acme.Orders.Manager"]["Services"][0];
    assert_eq!(call["Name"], "GetOrders");
    let binding = &value["DataBindings"][0];
    assert_eq!(binding["SourceArea"], "Data");
    assert_eq!(binding["SourceDetails"], "AllData");
    assert_eq!(
        binding["PropertyMaps"][0]["SourcePropertyType"],
        "InfoTable"
    );
    assert_eq!(
        binding["PropertyMaps"][0]["SourcePropertyBaseType"],
        "INFOTABLE"
    );
}

/// A Thing with GetOrders and GetOrder(id) (results of Order_DS: id, note) and
/// SaveOrder(id, note), a shape with GetLog,
/// that DataShape, and a blank mashup with a grid, a text field and a label in it.
fn bind_fixture() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = one_project();
    write(
        dir.path(),
        "acme/Things/Acme.Orders.Manager.xml",
        r#"<Entities><Things><Thing name="Acme.Orders.Manager" projectName="Acme"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            <ServiceDefinition name="SaveOrder"><ParameterDefinitions><FieldDefinition name="id" baseType="LONG"/><FieldDefinition name="note" baseType="STRING"/></ParameterDefinitions><ResultType baseType="NOTHING"/></ServiceDefinition>
            <ServiceDefinition name="GetOrder"><ParameterDefinitions><FieldDefinition name="id" baseType="LONG"/></ParameterDefinitions><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    );
    write(
        dir.path(),
        "acme/ThingShapes/Acme.Orders.Audit_TS.xml",
        r#"<Entities><ThingShapes><ThingShape name="Acme.Orders.Audit_TS" projectName="Acme"><ServiceDefinitions>
            <ServiceDefinition name="GetLog"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            </ServiceDefinitions></ThingShape></ThingShapes></Entities>"#,
    );
    write(
        dir.path(),
        "acme/DataShapes/Acme.Orders.Order_DS.xml",
        r#"<Entities><DataShapes><DataShape name="Acme.Orders.Order_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="id" baseType="LONG"/><FieldDefinition name="note" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
    );
    let name = "Acme.Orders_MU";
    let new = plan_new(&solution, &request(name)).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    // The grid last, so it shares center and does not fill it.
    for widget in ["text-field", "label", "grid"] {
        let add = add_request(name, widget);
        let plan = plan_add(&solution, &add).unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    }
    (dir, solution)
}

fn bind(solution: &Solution, from: &str, to: &str) -> Result<BindPlan, AuthoringError> {
    plan_bind(
        solution,
        &BindRequest {
            mashup: "Acme.Orders_MU".into(),
            from: from.into(),
            to: to.into(),
        },
    )
}

fn sidecar_content(solution: &Solution) -> String {
    workspace::read_mashup(&solution.src_root().join("Acme.Orders_MU/mashup"))
        .unwrap()
        .unwrap()
        .content
}

#[test]
fn bindings_to_parameters_and_from_selected_rows_reuse_calls_and_pass_the_gates() {
    let (_dir, solution) = bind_fixture();
    let ids: Vec<String> = mashup::edit::parse(&sidecar_content(&solution))
        .unwrap()
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    assert!(ids.contains(&"ptcstextfield-3".to_string()), "{ids:?}");
    assert!(ids.contains(&"ptcslabel-4".to_string()), "{ids:?}");
    let rows = [
        ("data:Acme.Orders.Manager.GetOrders", "ui:ptcsgrid-5.Data"),
        (
            "ui:ptcstextfield-3.Text",
            "data:Acme.Orders.Manager.SaveOrder.note",
        ),
        (
            "data:Acme.Orders.Manager.GetOrders[SelectedRows].note",
            "ui:ptcslabel-4.LabelText",
        ),
    ];
    for (from, to) in rows {
        let plan = bind(&solution, from, to).unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    }
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let calls: Vec<&str> = value["Data"]["Things_Acme.Orders.Manager"]["Services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| call["Name"].as_str().unwrap())
        .collect();
    assert_eq!(calls, ["GetOrders", "SaveOrder"], "GetOrders is reused");
    let maps: Vec<(&str, &str, &str)> = value["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            let map = &b["PropertyMaps"][0];
            (
                map["SourcePropertyType"].as_str().unwrap(),
                map["TargetPropertyType"].as_str().unwrap(),
                map["SourcePropertyBaseType"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        maps,
        [
            ("InfoTable", "property", "INFOTABLE"),
            ("Property", "Parameter", "STRING"),
            ("Field", "property", "STRING"),
        ]
    );
    assert_eq!(value["DataBindings"][2]["SourceDetails"], "SelectedRows");
    let report = crate::core::check::run(&solution);
    let failing: Vec<_> = report
        .gates
        .iter()
        // The fixture's services have no implementations, which only the project gate minds.
        .filter(|gate| !gate.ok() && gate.name != "project")
        .map(|gate| format!("{}: {:?} {:?}", gate.name, gate.findings, gate.broken))
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
    let described: Vec<(String, String)> =
        crate::core::mashup::describe::describe(&solution, "Acme.Orders_MU")
            .unwrap()
            .bindings
            .into_iter()
            .map(|b| (b.from, b.to))
            .collect();
    assert_eq!(described, rows.map(|(a, b)| (a.to_string(), b.to_string())));
}

#[test]
fn a_binding_twaco_cannot_stand_behind_is_refused() {
    let (dir, solution) = bind_fixture();
    let code = |from: &str, to: &str| bind(&solution, from, to).unwrap_err().code();
    assert_eq!(
        code("ui:nope-9.Text", "ui:ptcsgrid-5.Data"),
        ErrorCode::UnknownEntity
    );
    // STRING into a LONG parameter.
    assert_eq!(
        code(
            "ui:ptcstextfield-3.Text",
            "data:Acme.Orders.Manager.SaveOrder.id"
        ),
        ErrorCode::GuardRefused
    );
    // A grid's Data is an input only.
    assert_eq!(
        code("ui:ptcsgrid-5.Data", "ui:ptcslabel-4.LabelText"),
        ErrorCode::GuardRefused
    );
    // A service as a target needs a parameter.
    assert_eq!(
        code(
            "ui:ptcstextfield-3.Text",
            "data:Acme.Orders.Manager.SaveOrder"
        ),
        ErrorCode::InvalidArguments
    );
    // A shape's service with no call in the mashup yet.
    assert_eq!(
        code("data:Acme.Orders.Audit_TS.GetLog", "ui:ptcsgrid-5.Data"),
        ErrorCode::GuardRefused
    );
    assert_eq!(
        code("bogus", "ui:ptcsgrid-5.Data"),
        ErrorCode::InvalidArguments
    );
    // The same binding twice.
    let plan = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders",
        "ui:ptcsgrid-5.Data",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    assert_eq!(
        code("data:Acme.Orders.Manager.GetOrders", "ui:ptcsgrid-5.Data"),
        ErrorCode::AlreadyExists
    );
    // A widget the catalogue has no contract for: the label becomes an extension's widget, in
    // both the sidecar and the entity file so they agree.
    let content = sidecar_content(&solution).replace("\"ptcslabel\"", "\"acmewidget\"");
    std::fs::write(
        dir.path().join("src/Acme.Orders_MU/mashup/content.json"),
        &content,
    )
    .unwrap();
    let entity_path = dir.path().join("acme/Mashups/Acme.Orders_MU.xml");
    let css =
        std::fs::read_to_string(dir.path().join("src/Acme.Orders_MU/mashup/custom.css")).unwrap();
    let (entity, _) = mashup::sync(
        &std::fs::read(&entity_path).unwrap(),
        &mashup::Assets { content, css },
    )
    .unwrap();
    std::fs::write(&entity_path, entity).unwrap();
    let error = bind(&solution, "ui:ptcstextfield-3.Text", "ui:ptcslabel-4.Text").unwrap_err();
    assert_eq!(error.code(), ErrorCode::GuardRefused, "{error}");
    assert!(
        error
            .to_string()
            .contains("whose contract is not in the embedded ThingWorx 10.2 catalogue"),
        "{error}"
    );
}

fn on(solution: &Solution, event: &str, invoke: &str) -> Result<OnPlan, AuthoringError> {
    plan_on(
        solution,
        &OnRequest {
            mashup: "Acme.Orders_MU".into(),
            event: event.into(),
            invoke: invoke.into(),
        },
    )
}

#[test]
fn events_use_composer_keys_and_describe_their_wiring() {
    let (_dir, solution) = bind_fixture();
    let button = plan_add(&solution, &add_request("Acme.Orders_MU", "button")).unwrap();
    assert_eq!(button.placed.id, "ptcsbutton-6");
    apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
    let rows = [
        (
            "ui:ptcsbutton-6.Clicked",
            "data:Acme.Orders.Manager.SaveOrder",
        ),
        ("mashup:Loaded", "data:Acme.Orders.Manager.GetOrders"),
        (
            "data:Acme.Orders.Manager.SaveOrder.ServiceInvokeCompleted",
            "data:Acme.Orders.Manager.GetOrders",
        ),
    ];
    for (event, invoke) in rows {
        let plan = on(&solution, event, invoke)
            .unwrap_or_else(|error| panic!("{event} -> {invoke}: {error}"));
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    }
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let first = value["Events"][0].as_object().unwrap();
    assert_eq!(
        first.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "EventHandlerArea",
            "EventHandlerId",
            "EventHandlerService",
            "EventHandlerServiceMethod",
            "EventTriggerArea",
            "EventTriggerEvent",
            "EventTriggerId",
            "EventTriggerSection",
            "Id"
        ]
    );
    assert_eq!(first["EventHandlerArea"], "Data");
    assert_eq!(first["EventHandlerId"], "Things_Acme.Orders.Manager");
    assert_eq!(first["EventHandlerService"], "SaveOrder");
    assert_eq!(first["EventHandlerServiceMethod"], "");
    assert_eq!(first["EventTriggerArea"], "UI");
    assert_eq!(first["EventTriggerEvent"], "Clicked");
    assert_eq!(first["EventTriggerId"], "ptcsbutton-6");
    assert_eq!(first["EventTriggerSection"], "");
    assert!(first["Id"]
        .as_str()
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()));
    let described: Vec<(String, String)> =
        crate::core::mashup::describe::describe(&solution, "Acme.Orders_MU")
            .unwrap()
            .events
            .into_iter()
            .map(|event| (event.trigger, event.handler))
            .collect();
    assert_eq!(
        described,
        rows.map(|(event, invoke)| (event.into(), invoke.into()))
    );
    let failing: Vec<_> = crate::core::check::run(&solution)
        .gates
        .into_iter()
        .filter(|gate| !gate.ok() && gate.name != "project")
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
}

#[test]
fn event_authoring_refuses_unverified_wiring() {
    let (dir, solution) = bind_fixture();
    let button = plan_add(&solution, &add_request("Acme.Orders_MU", "button")).unwrap();
    apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
    let code = |event: &str, invoke: &str| on(&solution, event, invoke).unwrap_err().code();
    assert_eq!(
        code("ui:ptcsbutton-6.Nope", "data:Acme.Orders.Manager.SaveOrder"),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code("mashup:Unloaded", "data:Acme.Orders.Manager.SaveOrder"),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "data:Acme.Orders.Manager.SaveOrder.ServiceInvokeCompleted",
            "data:Acme.Orders.Manager.GetOrders"
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code("ui:ptcsbutton-6.Clicked", "ui:ptcsbutton-6.Nope"),
        ErrorCode::InvalidArguments
    );
    let plan = on(
        &solution,
        "ui:ptcsbutton-6.Clicked",
        "data:Acme.Orders.Manager.SaveOrder",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    assert_eq!(
        code(
            "ui:ptcsbutton-6.Clicked",
            "data:Acme.Orders.Manager.SaveOrder"
        ),
        ErrorCode::AlreadyExists
    );
    let content = sidecar_content(&solution).replace("\"ptcsbutton\"", "\"acmewidget\"");
    std::fs::write(
        dir.path().join("src/Acme.Orders_MU/mashup/content.json"),
        &content,
    )
    .unwrap();
    let entity_path = dir.path().join("acme/Mashups/Acme.Orders_MU.xml");
    let css =
        std::fs::read_to_string(dir.path().join("src/Acme.Orders_MU/mashup/custom.css")).unwrap();
    let (entity, _) = mashup::sync(
        &std::fs::read(&entity_path).unwrap(),
        &mashup::Assets { content, css },
    )
    .unwrap();
    std::fs::write(entity_path, entity).unwrap();
    assert_eq!(
        code(
            "ui:ptcsbutton-6.Clicked",
            "data:Acme.Orders.Manager.SaveOrder"
        ),
        ErrorCode::GuardRefused
    );
}

#[test]
fn a_widget_service_handler_is_addressed_by_the_widget() {
    let (_dir, solution) = bind_fixture();
    let button = plan_add(&solution, &add_request("Acme.Orders_MU", "button")).unwrap();
    apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
    let plan = on(&solution, "ui:ptcsbutton-6.Clicked", "ui:ptcsgrid-5.Reset").unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let event = &value["Events"][0];
    assert_eq!(event["EventHandlerArea"], "UI");
    assert_eq!(event["EventHandlerId"], "ptcsgrid-5");
    assert_eq!(event["EventHandlerService"], "Reset");
    let described = crate::core::mashup::describe::describe(&solution, "Acme.Orders_MU").unwrap();
    assert_eq!(described.events[0].handler, "ui:ptcsgrid-5.Reset");
}

/// Change the mashup's content in both its sidecar and its entity file, so they agree.
fn rewrite_content(dir: &Path, edit: impl FnOnce(&mut Value)) {
    let content_path = dir.join("src/Acme.Orders_MU/mashup/content.json");
    let mut value: Value =
        serde_json::from_str(&std::fs::read_to_string(&content_path).unwrap()).unwrap();
    edit(&mut value);
    let content = mashup::edit::parse(&serde_json::to_string(&value).unwrap())
        .unwrap()
        .to_content();
    std::fs::write(&content_path, &content).unwrap();
    let entity_path = dir.join("acme/Mashups/Acme.Orders_MU.xml");
    let css = std::fs::read_to_string(dir.join("src/Acme.Orders_MU/mashup/custom.css")).unwrap();
    let (entity, _) = mashup::sync(
        &std::fs::read(&entity_path).unwrap(),
        &mashup::Assets { content, css },
    )
    .unwrap();
    std::fs::write(&entity_path, entity).unwrap();
}

#[test]
fn named_calls_are_bound_and_invoked_by_name_and_several_calls_need_one() {
    let (_dir, solution) = bind_fixture();
    let plan = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders[as orders]",
        "ui:ptcsgrid-5.Data",
    )
    .unwrap();
    assert_eq!(
        plan.new_source.as_deref(),
        Some("Acme.Orders.Manager.GetOrders")
    );
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    // One call of the service: an address without a name finds it.
    let plan = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders[SelectedRows].note",
        "ui:ptcslabel-4.LabelText",
    )
    .unwrap();
    assert_eq!(plan.new_source, None);
    // A name another service's call has is refused, not given to a second call.
    let error = bind(
        &solution,
        "ui:ptcstextfield-3.Text",
        "data:Acme.Orders.Manager.SaveOrder[as orders].note",
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::AlreadyExists, "{error}");
    assert!(error.to_string().contains("for GetOrders"), "{error}");
    let plan = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders[as second][SelectedRows].note",
        "ui:ptcslabel-4.LabelText",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let plan = on(
        &solution,
        "mashup:Loaded",
        "data:Acme.Orders.Manager.GetOrders[as second]",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let calls: Vec<(&str, &str)> = value["Data"]["Things_Acme.Orders.Manager"]["Services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| {
            (
                call["Name"].as_str().unwrap(),
                call["Target"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(calls, [("orders", "GetOrders"), ("second", "GetOrders")]);
    assert_eq!(value["Events"][0]["EventHandlerService"], "second");
    // Two calls of GetOrders: an address must say which.
    let errors = [
        bind(
            &solution,
            "data:Acme.Orders.Manager.GetOrders",
            "ui:ptcsgrid-5.Data",
        )
        .unwrap_err(),
        on(
            &solution,
            "mashup:Loaded",
            "data:Acme.Orders.Manager.GetOrders",
        )
        .unwrap_err(),
        on(
            &solution,
            "data:Acme.Orders.Manager.GetOrders.ServiceInvokeCompleted",
            "data:Acme.Orders.Manager.SaveOrder",
        )
        .unwrap_err(),
    ];
    for error in errors {
        assert_eq!(error.code(), ErrorCode::Ambiguous, "{error}");
        assert!(error.to_string().contains("orders, second"), "{error}");
    }
    // What describe prints, bind and on parse.
    let described = crate::core::mashup::describe::describe(&solution, "Acme.Orders_MU").unwrap();
    assert_eq!(
        described.bindings[1].from,
        "data:Acme.Orders.Manager.GetOrders[as second][SelectedRows].note"
    );
    assert_eq!(
        described.events[0].handler,
        "data:Acme.Orders.Manager.GetOrders[as second]"
    );
}

#[test]
fn a_result_field_named_like_a_parameter_is_a_source_and_a_parameter_is_not() {
    let (_dir, solution) = bind_fixture();
    let plan = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrder[SelectedRows].id",
        "ui:ptcslabel-4.LabelText",
    )
    .unwrap();
    let value: Value =
        serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    let map = &value["DataBindings"][0]["PropertyMaps"][0];
    assert_eq!(map["SourcePropertyType"], "Field");
    assert_eq!(map["SourcePropertyBaseType"], "LONG");
    let error = bind(
        &solution,
        "data:Acme.Orders.Manager.SaveOrder.note",
        "ui:ptcslabel-4.LabelText",
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments, "{error}");
    assert!(error.to_string().contains("is a parameter"), "{error}");
}

#[test]
fn a_mashup_parameter_binds_to_a_service_parameter() {
    let (dir, solution) = bind_fixture();
    let entity_path = dir.path().join("acme/Mashups/Acme.Orders_MU.xml");
    let entity = std::fs::read_to_string(&entity_path).unwrap().replace(
            "<ParameterDefinitions></ParameterDefinitions>",
            r#"<ParameterDefinitions><FieldDefinition name="OrderId" baseType="LONG" aspect.bindingDirection="IN"/></ParameterDefinitions>"#,
        );
    std::fs::write(&entity_path, entity).unwrap();
    let plan = bind(
        &solution,
        "mashup:OrderId",
        "data:Acme.Orders.Manager.SaveOrder.id",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let binding = &value["DataBindings"][0];
    assert_eq!(binding["SourceArea"], "Mashup");
    assert_eq!(binding["SourceId"], mashup::edit::ROOT_ID);
    assert_eq!(binding["PropertyMaps"][0]["SourceProperty"], "OrderId");
    assert_eq!(binding["PropertyMaps"][0]["TargetProperty"], "id");
    assert_eq!(
        crate::core::mashup::describe::describe(&solution, "Acme.Orders_MU")
            .unwrap()
            .bindings[0]
            .from,
        "mashup:OrderId"
    );
    assert_eq!(
        bind(
            &solution,
            "mashup:Nope",
            "data:Acme.Orders.Manager.SaveOrder.id"
        )
        .unwrap_err()
        .code(),
        ErrorCode::UnknownEntity
    );
}

#[test]
fn a_second_source_for_one_target_is_bound_with_a_warning() {
    let (_dir, solution) = bind_fixture();
    let first = bind(
        &solution,
        "ui:ptcstextfield-3.Text",
        "ui:ptcslabel-4.LabelText",
    )
    .unwrap();
    assert!(first.warnings.is_empty());
    apply_edit(&solution, &first.edit, &lock(&solution)).unwrap();
    let second = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders[SelectedRows].note",
        "ui:ptcslabel-4.LabelText",
    )
    .unwrap();
    assert_eq!(second.warnings.len(), 1, "{:?}", second.warnings);
    assert!(second.warnings[0].contains("ui:ptcslabel-4.LabelText already has another source"));
}

#[test]
fn wiring_names_widgets_exactly_and_does_not_guess_by_catalogue_search() {
    let (_dir, solution) = bind_fixture();
    let document = mashup::edit::parse(&sidecar_content(&solution)).unwrap();
    // `add` would take "gri" for the grid; bind and on do not.
    assert_eq!(document.find("gri").unwrap().id, "ptcsgrid-5");
    let error = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders",
        "ui:gri.Data",
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnknownEntity, "{error}");
    let error = on(
        &solution,
        "ui:gri.DoubleClicked",
        "data:Acme.Orders.Manager.SaveOrder",
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnknownEntity, "{error}");
    // A word the catalogue resolves still names the only widget of its type.
    bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders",
        "ui:grid.Data",
    )
    .unwrap();
}

#[test]
fn a_shape_service_already_called_in_the_mashup_is_reused() {
    let (dir, solution) = bind_fixture();
    rewrite_content(dir.path(), |value| {
        value["Data"] = json!({
            "ThingShapes_Acme.Orders.Audit_TS": {
                "DataName": "ThingShapes_Acme.Orders.Audit_TS",
                "EntityName": "Acme.Orders.Audit_TS",
                "EntityType": "ThingShapes",
                "Id": "a1",
                "RefreshInterval": 0,
                "Services": [{"Name": "GetLog", "Target": "GetLog", "Parameters": {}}]
            }
        });
    });
    let plan = bind(
        &solution,
        "data:Acme.Orders.Audit_TS.GetLog",
        "ui:ptcsgrid-5.Data",
    )
    .unwrap();
    assert_eq!(plan.new_source, None);
    let value: Value =
        serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    assert_eq!(
        value["DataBindings"][0]["SourceSection"],
        "ThingShapes_Acme.Orders.Audit_TS"
    );
    assert_eq!(value["DataBindings"][0]["SourceId"], "GetLog");
}

#[test]
fn planning_add_bind_and_on_writes_nothing() {
    use crate::core::commands::{self, Mode, Notices};
    fn snapshot(dir: &Path, files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                snapshot(&path, files);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    let (dir, solution) = bind_fixture();
    let mut before = std::collections::BTreeMap::new();
    snapshot(dir.path(), &mut before);
    commands::mashup::add(
        &solution,
        &commands::mashup::MashupAddRequest {
            request: add_request("Acme.Orders_MU", "button"),
            mode: Mode::Plan,
            lock_label: "test",
        },
        &mut Notices::default(),
    )
    .unwrap();
    commands::mashup::bind(
        &solution,
        &commands::mashup::MashupBindRequest {
            request: BindRequest {
                mashup: "Acme.Orders_MU".into(),
                from: "data:Acme.Orders.Manager.GetOrders".into(),
                to: "ui:ptcsgrid-5.Data".into(),
            },
            mode: Mode::Plan,
            lock_label: "test",
        },
        &mut Notices::default(),
    )
    .unwrap();
    commands::mashup::on(
        &solution,
        &commands::mashup::MashupOnRequest {
            request: OnRequest {
                mashup: "Acme.Orders_MU".into(),
                event: "mashup:Loaded".into(),
                invoke: "data:Acme.Orders.Manager.GetOrders".into(),
            },
            mode: Mode::Plan,
            lock_label: "test",
        },
        &mut Notices::default(),
    )
    .unwrap();
    let mut after = std::collections::BTreeMap::new();
    snapshot(dir.path(), &mut after);
    assert!(before == after, "planning changed a file");
}

fn param_request(mashup: &str, action: ParameterAction, name: &str) -> ParameterRequest {
    ParameterRequest {
        mashup: mashup.into(),
        action,
        name: name.into(),
        base_type: None,
        direction: None,
        description: None,
        default: ParameterDefault::Unchanged,
        data_shape: None,
        force: false,
    }
}

fn param(solution: &Solution, request: &ParameterRequest) -> Result<ParameterPlan, AuthoringError> {
    let plan = plan_parameter(solution, request)?;
    apply_parameter(solution, &plan, &lock(solution))?;
    Ok(plan)
}

fn content_of(solution: &Solution, mashup: &str) -> Value {
    let text = workspace::read_mashup(&solution.src_root().join(format!("{mashup}/mashup")))
        .unwrap()
        .unwrap()
        .content;
    serde_json::from_str(&text).unwrap()
}

/// Change a mashup's content in its sidecar and entity file together.
fn rewrite_mashup(dir: &Path, mashup: &str, edit: impl FnOnce(&mut Value)) {
    let content_path = dir.join(format!("src/{mashup}/mashup/content.json"));
    let mut value: Value =
        serde_json::from_str(&std::fs::read_to_string(&content_path).unwrap()).unwrap();
    edit(&mut value);
    let content = mashup::edit::parse(&serde_json::to_string(&value).unwrap())
        .unwrap()
        .to_content();
    std::fs::write(&content_path, &content).unwrap();
    let entity_path = dir.join(format!("acme/Mashups/{mashup}.xml"));
    let css = std::fs::read_to_string(dir.join(format!("src/{mashup}/mashup/custom.css"))).unwrap();
    let (entity, _) = mashup::sync(
        &std::fs::read(&entity_path).unwrap(),
        &mashup::Assets { content, css },
    )
    .unwrap();
    std::fs::write(&entity_path, entity).unwrap();
}

/// A child `Acme.Card_MU` with `id` (LONG, IN) whose content binds `mashup:id` into a label, and a
/// parent `Acme.Board_MU` holding it in `mashupcontainer-9` with a stale cache (no entries), a
/// static `id` of 7, and a binding from the container's `id` into a label.
fn parent_and_child() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = one_project();
    let mut child = request("Acme.Card_MU");
    child.params = vec!["id:LONG:IN".into()];
    apply_new(
        &solution,
        &plan_new(&solution, &child).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let label = plan_add(&solution, &add_request("Acme.Card_MU", "label")).unwrap();
    apply_edit(&solution, &label.edit, &lock(&solution)).unwrap();
    rewrite_mashup(dir.path(), "Acme.Card_MU", |value| {
        value["DataBindings"] = json!([{
            "Id": "child-1",
            "PropertyMaps": [{"SourceProperty": "id", "SourcePropertyBaseType": "LONG",
                "SourcePropertyType": "Property", "TargetProperty": "LabelText",
                "TargetPropertyBaseType": "STRING", "TargetPropertyType": "property"}],
            "SourceArea": "Mashup", "SourceDetails": "", "SourceId": "mashup-root", "SourceSection": "",
            "TargetArea": "UI", "TargetId": "ptcslabel-3", "TargetSection": ""
        }]);
    });
    apply_new(
        &solution,
        &plan_new(&solution, &request("Acme.Board_MU")).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let label = plan_add(&solution, &add_request("Acme.Board_MU", "label")).unwrap();
    apply_edit(&solution, &label.edit, &lock(&solution)).unwrap();
    rewrite_mashup(dir.path(), "Acme.Board_MU", |value| {
        value["UI"]["Widgets"][0]["Widgets"].as_array_mut().unwrap().push(json!({
                "Properties": {"Id": "mashupcontainer-9", "Type": "mashupcontainer",
                    "DisplayName": "mashupcontainer-9", "Name": "Acme.Card_MU", "ResponsiveLayout": true,
                    "MashupParameters": [{"BaseType": "LONG", "DefaultValue": 7, "Description": "",
                        "ParameterName": "id", "ParmDef": {"aspects": {"bindingDirection": "IN",
                        "isMandatory": "false"}, "baseType": "LONG", "description": "", "name": "id",
                        "ordinal": 0}}],
                    "id": 7},
                "Widgets": []
            }));
        value["DataBindings"] = json!([{
            "Id": "parent-1",
            "PropertyMaps": [{"SourceProperty": "id", "SourcePropertyBaseType": "LONG",
                "SourcePropertyType": "property", "TargetProperty": "LabelText",
                "TargetPropertyBaseType": "STRING", "TargetPropertyType": "property"}],
            "SourceArea": "UI", "SourceDetails": "", "SourceId": "mashupcontainer-9", "SourceSection": "",
            "TargetArea": "UI", "TargetId": "ptcslabel-3", "TargetSection": ""
        }]);
    });
    (dir, solution)
}

fn container(value: &Value) -> &Value {
    value["UI"]["Widgets"][0]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["Properties"]["Type"] == "mashupcontainer")
        .map(|w| &w["Properties"])
        .unwrap()
}

fn contained_fixture() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = one_project();
    let mut child = request("Acme.Card_MU");
    child.params = vec![
        "id:LONG:IN".into(),
        "title:STRING:BOTH".into(),
        "done:NUMBER:OUT".into(),
    ];
    apply_new(
        &solution,
        &plan_new(&solution, &child).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    apply_new(
        &solution,
        &plan_new(&solution, &request("Acme.Board_MU")).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    (dir, solution)
}

#[test]
fn add_contained_in_center_writes_composer_cache_and_passes_the_project_gate() {
    let (_dir, solution) = contained_fixture();
    let plan = plan_add_contained(
        &solution,
        &AddContainedRequest {
            mashup: "Acme.Board_MU".into(),
            child: "Acme.Card_MU".into(),
            placement: None,
            settings: vec![("id".into(), "7".into())],
        },
    )
    .unwrap();
    assert_eq!(plan.placed.id, "mashupcontainer-3");
    let value: Value =
        serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    let properties = &value["UI"]["Widgets"][0]["Widgets"][0]["Properties"];
    assert_eq!(properties["Name"], "Acme.Card_MU");
    assert_eq!(properties["ResponsiveLayout"], true);
    assert_eq!(properties["Margin"], "5");
    assert_eq!(properties["id"], 7);
    assert_eq!(properties["MashupParameters"][0]["DefaultValue"], 7);
    assert_eq!(
        properties["MashupParameters"][0],
        cache_entry(
            &ParameterDefinition {
                name: "id".into(),
                base_type: "LONG".into(),
                direction: "IN".into(),
                description: String::new(),
                default: None,
                data_shape: None,
                visible: true
            },
            Some(json!(7))
        )
    );
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let failures: Vec<_> = crate::core::check::run(&solution)
        .gates
        .into_iter()
        .filter(|gate| gate.name == "project" && !gate.ok())
        .collect();
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn add_contained_refuses_invalid_children_and_settings() {
    let (_dir, solution) = contained_fixture();
    let add = |child: &str, settings: Vec<(String, String)>| {
        plan_add_contained(
            &solution,
            &AddContainedRequest {
                mashup: "Acme.Board_MU".into(),
                child: child.into(),
                placement: None,
                settings,
            },
        )
    };
    assert_eq!(
        add("Acme.Nope_MU", vec![]).unwrap_err().code(),
        ErrorCode::UnknownEntity
    );
    assert_eq!(
        plan_add_contained(
            &solution,
            &AddContainedRequest {
                mashup: "Acme.Board_MU".into(),
                child: "Acme.Board_MU".into(),
                placement: None,
                settings: vec![]
            }
        )
        .unwrap_err()
        .code(),
        ErrorCode::GuardRefused
    );
    assert_eq!(
        add("Acme.Card_MU", vec![("done".into(), "1".into())])
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        add("Acme.Card_MU", vec![("nope".into(), "1".into())])
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        add("Acme.Card_MU", vec![("id".into(), "nope".into())])
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArguments
    );
    let master = plan_new(
        &solution,
        &NewRequest {
            master: true,
            ..request("Acme.Master_MM")
        },
    )
    .unwrap();
    apply_new(&solution, &master, &lock(&solution)).unwrap();
    assert_eq!(
        add("Acme.Master_MM", vec![]).unwrap_err().code(),
        ErrorCode::GuardRefused
    );
}

#[test]
fn add_contained_beside_a_label_uses_the_normal_placement_rules() {
    let (_dir, solution) = contained_fixture();
    let label = plan_add(&solution, &add_request("Acme.Board_MU", "label")).unwrap();
    apply_edit(&solution, &label.edit, &lock(&solution)).unwrap();
    let plan = plan_add_contained(
        &solution,
        &AddContainedRequest {
            mashup: "Acme.Board_MU".into(),
            child: "Acme.Card_MU".into(),
            placement: Some(Placement::Beside {
                anchor: "ptcslabel-3".into(),
                side: mashup::edit::Side::Right,
            }),
            settings: vec![],
        },
    )
    .unwrap();
    let value: Value =
        serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    let ids = value["UI"]["Widgets"][0]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|widget| widget["Properties"]["Id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["ptcslabel-3", "mashupcontainer-4"]);
}

#[test]
fn contained_parameters_bind_refresh_and_events_follow_the_container_contract() {
    let (_dir, solution) = contained_fixture();
    let add = plan_add_contained(
        &solution,
        &AddContainedRequest {
            mashup: "Acme.Board_MU".into(),
            child: "Acme.Card_MU".into(),
            placement: None,
            settings: vec![],
        },
    )
    .unwrap();
    apply_edit(&solution, &add.edit, &lock(&solution)).unwrap();
    let add_sibling = |widget: &str, anchor: &str| {
        let mut request = add_request("Acme.Board_MU", widget);
        request.placement = Some(Placement::Beside {
            anchor: anchor.into(),
            side: mashup::edit::Side::Right,
        });
        let plan = plan_add(&solution, &request).unwrap();
        let id = plan.placed.id.clone();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        id
    };
    let text = add_sibling("text-field", "mashupcontainer-3");
    let label = add_sibling("label", &text);
    let button = add_sibling("button", &label);
    rewrite_mashup(_dir.path(), "Acme.Board_MU", |value| {
        fn clear(widget: &mut Value) {
            if widget["Properties"]["Id"] == "mashupcontainer-3" {
                widget["Properties"]["MashupParameters"] = json!([]);
            }
            if let Some(children) = widget["Widgets"].as_array_mut() {
                for child in children {
                    clear(child);
                }
            }
        }
        clear(&mut value["UI"]);
    });
    let bind_board = |from: String, to: String| {
        plan_bind(
            &solution,
            &BindRequest {
                mashup: "Acme.Board_MU".into(),
                from,
                to,
            },
        )
    };
    let title = bind_board(
        format!("ui:{text}.Text"),
        "ui:mashupcontainer-3.title".into(),
    )
    .unwrap();
    let value: Value =
        serde_json::from_slice(&title.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    assert_eq!(value["DataBindings"][0]["TargetArea"], "UI");
    assert_eq!(value["DataBindings"][0]["TargetId"], "mashupcontainer-3");
    assert_eq!(
        value["DataBindings"][0]["PropertyMaps"][0]["TargetPropertyType"],
        "property"
    );
    assert_eq!(title.refreshed, ["mashupcontainer-3"]);
    assert_eq!(
        bind_board(
            "ui:mashupcontainer-3.done".into(),
            format!("ui:{label}.LabelText")
        )
        .unwrap()
        .edit
        .mashup,
        "Acme.Board_MU"
    );
    assert_eq!(
        bind_board(
            format!("ui:{text}.Text"),
            "ui:mashupcontainer-3.done".into()
        )
        .unwrap_err()
        .code(),
        ErrorCode::GuardRefused
    );
    assert_eq!(
        bind_board(
            "ui:mashupcontainer-3.id".into(),
            format!("ui:{label}.LabelText")
        )
        .unwrap_err()
        .code(),
        ErrorCode::GuardRefused
    );
    let unknown = bind_board(
        format!("ui:{text}.Text"),
        "ui:mashupcontainer-3.nope".into(),
    )
    .unwrap_err();
    assert_eq!(unknown.code(), ErrorCode::UnknownEntity);
    assert!(unknown.to_string().contains("id, title, done"));
    let on_plan = plan_on(
        &solution,
        &OnRequest {
            mashup: "Acme.Board_MU".into(),
            event: format!("ui:{button}.Clicked"),
            invoke: "ui:mashupcontainer-3.Refresh".into(),
        },
    )
    .unwrap();
    let event: Value =
        serde_json::from_slice(&on_plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    assert_eq!(event["Events"][0]["EventHandlerArea"], "UI");
    assert_eq!(event["Events"][0]["EventHandlerId"], "mashupcontainer-3");
    assert_eq!(event["Events"][0]["EventHandlerService"], "Refresh");
    assert_eq!(
        plan_on(
            &solution,
            &OnRequest {
                mashup: "Acme.Board_MU".into(),
                event: "ui:mashupcontainer-3.Anything".into(),
                invoke: "ui:mashupcontainer-3.Refresh".into()
            }
        )
        .unwrap_err()
        .code(),
        // A contained mashup's events are its BOTH/OUT parameters; it has no Anything.
        ErrorCode::UnknownEntity
    );
}

#[test]
fn hidden_and_config_form_child_parameters_are_not_offered_or_bindable() {
    let (dir, solution) = contained_fixture();
    rewrite_mashup(dir.path(), "Acme.Card_MU", |value| {
        value["UI"]["Properties"]["_currentParameterDefs"].as_array_mut().unwrap().push(json!({
                "ParameterName": "ConfigForm_mode", "BaseType": "STRING", "Description": "", "Aspects": {"bindingDirection": "BOTH"}
            }));
        value["UI"]["Properties"]["_currentParameterDefs"][1]["Aspects"]["isVisible"] =
            json!(false);
    });
    let plan = plan_add_contained(
        &solution,
        &AddContainedRequest {
            mashup: "Acme.Board_MU".into(),
            child: "Acme.Card_MU".into(),
            placement: None,
            settings: vec![],
        },
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let text = plan_add(
        &solution,
        &AddRequest {
            mashup: "Acme.Board_MU".into(),
            widget: "text-field".into(),
            inputs: Vec::new(),
            placement: Some(Placement::Beside {
                anchor: "mashupcontainer-3".into(),
                side: mashup::edit::Side::Right,
            }),
            label: None,
            properties: vec![],
        },
    )
    .unwrap();
    apply_edit(&solution, &text.edit, &lock(&solution)).unwrap();
    let error = plan_bind(
        &solution,
        &BindRequest {
            mashup: "Acme.Board_MU".into(),
            from: format!("ui:{}.Text", text.placed.id),
            to: "ui:mashupcontainer-3.title".into(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnknownEntity);
    assert!(!error.to_string().contains("title, "), "{error}");
    let error = plan_bind(
        &solution,
        &BindRequest {
            mashup: "Acme.Board_MU".into(),
            from: format!("ui:{}.Text", text.placed.id),
            to: "ui:mashupcontainer-3.ConfigForm_mode".into(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnknownEntity);
}

#[test]
fn adding_a_parameter_refreshes_every_container_of_the_mashup_and_keeps_static_values() {
    let (_dir, solution) = parent_and_child();
    let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
    add.base_type = Some("STRING".into());
    let plan = param(&solution, &add).unwrap();
    assert_eq!(
        plan.refreshed,
        [("mashupcontainer-9".to_string(), "Acme.Board_MU".to_string())]
    );
    let parent = content_of(&solution, "Acme.Board_MU");
    let cache = &container(&parent)["MashupParameters"];
    assert_eq!(
        cache[1],
        json!({"BaseType": "STRING", "Description": "", "ParameterName": "title",
                "ParmDef": {"aspects": {"bindingDirection": "BOTH", "isMandatory": "false"},
                "baseType": "STRING", "description": "", "name": "title", "ordinal": 0}})
    );
    assert_eq!(
        serde_json::to_string(&cache[1]).unwrap(),
        r#"{"BaseType":"STRING","Description":"","ParameterName":"title","ParmDef":{"aspects":{"bindingDirection":"BOTH","isMandatory":"false"},"baseType":"STRING","description":"","name":"title","ordinal":0}}"#,
        "key order as Composer writes it"
    );
    // The unrelated `id` keeps the static value the container passes.
    assert_eq!(cache[0]["DefaultValue"], 7);
    assert_eq!(container(&parent)["id"], 7);
    // Both mashups changed in one plan, and the result passes the gates.
    assert_eq!(plan.edits.len(), 2);
    let failing: Vec<_> = crate::core::check::run(&solution)
        .gates
        .into_iter()
        .filter(|gate| !gate.ok() && gate.name != "project")
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
    let described = crate::core::mashup::describe::describe(&solution, "Acme.Board_MU").unwrap();
    assert_eq!(described.containers[0].mashup, "Acme.Card_MU");
    assert!(described.containers[0]
        .parameters
        .iter()
        .all(|parameter| parameter.status == "ok"));
}

#[test]
fn describe_reports_a_container_cache_that_misses_a_parameter() {
    let (dir, solution) = parent_and_child();
    rewrite_mashup(dir.path(), "Acme.Board_MU", |value| {
        value["UI"]["Widgets"][0]["Widgets"][1]["Properties"]["MashupParameters"] = json!([]);
    });
    let described = crate::core::mashup::describe::describe(&solution, "Acme.Board_MU").unwrap();
    assert_eq!(
        described.containers[0].parameters[0].status,
        "missing in cache"
    );
}

#[test]
fn a_type_change_retypes_bindings_in_both_mashups_and_drops_the_stale_static_value() {
    let (_dir, solution) = parent_and_child();
    let mut set = param_request("Acme.Card_MU", ParameterAction::Set, "id");
    set.base_type = Some("STRING".into());
    let plan = param(&solution, &set).unwrap();
    assert_eq!(plan.retyped, ["child-1", "parent-1"]);
    let child = content_of(&solution, "Acme.Card_MU");
    assert_eq!(
        child["DataBindings"][0]["PropertyMaps"][0]["SourcePropertyBaseType"],
        "STRING"
    );
    assert_eq!(
        child["UI"]["Properties"]["_currentParameterDefs"][0]["BaseType"],
        "STRING"
    );
    let entity =
        std::fs::read_to_string(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
    assert!(entity.contains(
            r#"<FieldDefinition aspect.bindingDirection="IN" aspect.isMandatory="false" baseType="STRING" description="" name="id" ordinal="0"/>"#
        ));
    let parent = content_of(&solution, "Acme.Board_MU");
    assert_eq!(
        parent["DataBindings"][0]["PropertyMaps"][0]["SourcePropertyBaseType"],
        "STRING"
    );
    let properties = container(&parent);
    assert!(
        properties.get("id").is_none(),
        "a LONG static value is not a STRING one"
    );
    assert_eq!(properties["MashupParameters"][0]["BaseType"], "STRING");
    assert!(properties["MashupParameters"][0]
        .get("DefaultValue")
        .is_none());
}

#[test]
fn a_type_change_that_breaks_a_binding_is_refused_and_writes_nothing() {
    let (dir, solution) = parent_and_child();
    rewrite_mashup(dir.path(), "Acme.Card_MU", |value| {
        let map = &mut value["DataBindings"][0]["PropertyMaps"][0];
        map["TargetPropertyBaseType"] = json!("LONG");
    });
    let before = std::fs::read(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
    let mut set = param_request("Acme.Card_MU", ParameterAction::Set, "id");
    set.base_type = Some("BOOLEAN".into());
    let error = plan_parameter(&solution, &set).unwrap_err();
    assert_eq!(error.code(), ErrorCode::GuardRefused, "{error}");
    assert!(error.to_string().contains("child-1"), "{error}");
    assert_eq!(
        std::fs::read(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap(),
        before
    );
}

#[test]
fn removing_a_bound_parameter_needs_force_which_removes_the_bindings_and_cache_entries() {
    let (_dir, solution) = parent_and_child();
    let remove = param_request("Acme.Card_MU", ParameterAction::Remove, "id");
    let error = plan_parameter(&solution, &remove).unwrap_err();
    assert_eq!(error.code(), ErrorCode::GuardRefused, "{error}");
    assert!(error.to_string().contains("child-1"), "{error}");
    let plan = param(
        &solution,
        &ParameterRequest {
            force: true,
            ..remove
        },
    )
    .unwrap();
    assert_eq!(plan.removed, ["child-1", "parent-1"]);
    let child = content_of(&solution, "Acme.Card_MU");
    assert_eq!(child["DataBindings"], json!([]));
    assert_eq!(
        child["UI"]["Properties"]["_currentParameterDefs"],
        json!([])
    );
    let entity = std::fs::read(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
    assert!(mashup_parameters(&entity).unwrap().is_empty());
    let parent = content_of(&solution, "Acme.Board_MU");
    assert_eq!(parent["DataBindings"], json!([]));
    let properties = container(&parent);
    assert_eq!(properties["MashupParameters"], json!([]));
    assert!(properties.get("id").is_none());
}

#[test]
fn removing_keeps_the_order_of_the_other_root_properties() {
    let (_dir, solution) = parent_and_child();
    let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
    add.base_type = Some("STRING".into());
    add.default = ParameterDefault::Set("Orders".into());
    param(&solution, &add).unwrap();
    let keys = |value: &Value| -> Vec<String> {
        value["UI"]["Properties"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| *key != "title")
            .cloned()
            .collect()
    };
    let before = keys(&content_of(&solution, "Acme.Card_MU"));
    param(
        &solution,
        &param_request("Acme.Card_MU", ParameterAction::Remove, "title"),
    )
    .unwrap();
    assert_eq!(keys(&content_of(&solution, "Acme.Card_MU")), before);
}

#[test]
fn a_parent_changed_after_planning_stops_the_whole_write() {
    let (_dir, solution) = parent_and_child();
    let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
    add.base_type = Some("STRING".into());
    let plan = plan_parameter(&solution, &add).unwrap();
    let child_path = solution.root.join("acme/Mashups/Acme.Card_MU.xml");
    let child_before = std::fs::read(&child_path).unwrap();
    let parent_path = solution.root.join("acme/Mashups/Acme.Board_MU.xml");
    let mut parent = std::fs::read(&parent_path).unwrap();
    parent.extend_from_slice(b"\n");
    std::fs::write(&parent_path, &parent).unwrap();
    assert!(apply_parameter(&solution, &plan, &lock(&solution)).is_err());
    assert_eq!(std::fs::read(&child_path).unwrap(), child_before);
    assert_eq!(std::fs::read(&parent_path).unwrap(), parent);
}

#[test]
fn an_infotable_parameter_needs_a_data_shape_from_the_solution() {
    let (dir, solution) = parent_and_child();
    let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "rows");
    add.base_type = Some("INFOTABLE".into());
    assert_eq!(
        plan_parameter(&solution, &add).unwrap_err().code(),
        ErrorCode::InvalidArguments
    );
    add.data_shape = Some("Acme.Orders.Row_DS".into());
    assert_eq!(
        plan_parameter(&solution, &add).unwrap_err().code(),
        ErrorCode::UnknownEntity
    );
    write(
        dir.path(),
        "acme/DataShapes/Acme.Orders.Row_DS.xml",
        r#"<Entities><DataShapes><DataShape name="Acme.Orders.Row_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="id" baseType="LONG"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
    );
    param(&solution, &add).unwrap();
    let entity = std::fs::read_to_string(dir.path().join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
    assert!(entity.contains(
            r#"<FieldDefinition aspect.bindingDirection="BOTH" aspect.dataShape="Acme.Orders.Row_DS" aspect.isMandatory="false" baseType="INFOTABLE" description="" name="rows" ordinal="0"/>"#
        ));
    let child = content_of(&solution, "Acme.Card_MU");
    assert_eq!(
        child["UI"]["Properties"]["_currentParameterDefs"][1]["Aspects"],
        json!({"bindingDirection": "BOTH", "dataShape": "Acme.Orders.Row_DS", "isMandatory": false})
    );
    let parent = content_of(&solution, "Acme.Board_MU");
    assert_eq!(
        container(&parent)["MashupParameters"][1]["ParmDef"]["aspects"]["dataShape"],
        "Acme.Orders.Row_DS"
    );
}

#[test]
fn parameter_requests_twaco_cannot_stand_behind_are_refused() {
    let (_dir, solution) = parent_and_child();
    let code = |name: &str, base_type: Option<&str>, action: ParameterAction| {
        let mut request = param_request("Acme.Card_MU", action, name);
        request.base_type = base_type.map(str::to_string);
        plan_parameter(&solution, &request).unwrap_err().code()
    };
    assert_eq!(
        code("id", Some("LONG"), ParameterAction::Add),
        ErrorCode::AlreadyExists
    );
    assert_eq!(
        code("1x", Some("STRING"), ParameterAction::Add),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code("Entity", Some("STRING"), ParameterAction::Add),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code("x", Some("WORD"), ParameterAction::Add),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code("x", None, ParameterAction::Add),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code("nope", None, ParameterAction::Remove),
        ErrorCode::UnknownEntity
    );
    assert_eq!(
        code("nope", Some("STRING"), ParameterAction::Set),
        ErrorCode::UnknownEntity
    );
    let mut bad_default = param_request("Acme.Card_MU", ParameterAction::Add, "count");
    bad_default.base_type = Some("LONG".into());
    bad_default.default = ParameterDefault::Set("many".into());
    assert_eq!(
        plan_parameter(&solution, &bad_default).unwrap_err().code(),
        ErrorCode::InvalidArguments
    );
}

#[test]
fn a_mashup_without_sidecars_still_refreshes_its_containers() {
    let (dir, solution) = parent_and_child();
    std::fs::remove_dir_all(dir.path().join("src/Acme.Card_MU")).unwrap();
    let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
    add.base_type = Some("STRING".into());
    let plan = param(&solution, &add).unwrap();
    assert!(plan.edits[0].sidecars.is_none());
    let names: Vec<_> = container(&content_of(&solution, "Acme.Board_MU"))["MashupParameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["ParameterName"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["id", "title"]);
}

#[test]
fn template_containers_have_no_margin_and_their_widgets_keep_composers() {
    // A 5px margin, the 10.2 default nearly no real container keeps, shrinks every fixed band
    // by 10px and hides a 1px divider; Composer clears it on a container it drops.
    for template in mashup::templates::layouts().unwrap() {
        for master in [false, true] {
            if master && template.master_slot.is_none() && !template.children.is_empty() {
                continue;
            }
            let document = new_document(&template, master, DEFAULT_THEME).unwrap();
            for widget in document.widgets().into_iter().skip(1) {
                let margin = &document.at(&widget.path)["Properties"]["Margin"];
                if widget.type_id == "flexcontainer" {
                    assert_eq!(margin, "", "{} {}", template.name, widget.id);
                } else {
                    assert_eq!(margin, "5", "{} {}", template.name, widget.id);
                }
            }
        }
    }
}

#[test]
fn a_menu_bar_sits_in_a_row_so_it_fills_the_sidebar_height() {
    for name in ["sidebar", "header-sidebar"] {
        let template = mashup::templates::layout(name).unwrap().unwrap();
        let document = new_document(&template, false, DEFAULT_THEME).unwrap();
        let sidebar = document.find("sidebar").unwrap();
        assert_eq!(
            document.at(&sidebar.path)["Properties"]["flex-direction"],
            "row",
            "{name}"
        );
    }
}

#[test]
fn parameter_definitions_go_where_the_server_keeps_them_sorted_by_name_ignoring_case() {
    // A live 10.1 server read back `done, orderId, title` for definitions sent as
    // `orderId, title, done`; exports sort `cardWidth, Manager, mashupName` the same way.
    let (_dir, solution) = parent_and_child();
    for (name, base_type) in [("title", "STRING"), ("Alpha", "STRING"), ("done", "NUMBER")] {
        let mut add = param_request("Acme.Card_MU", ParameterAction::Add, name);
        add.base_type = Some(base_type.into());
        param(&solution, &add).unwrap();
    }
    let entity = std::fs::read(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
    let names: Vec<String> = mashup_parameters(&entity)
        .unwrap()
        .into_iter()
        .map(|parameter| parameter.name)
        .collect();
    assert_eq!(names, ["Alpha", "done", "id", "title"]);
    // The content keeps the order they were made in, as the server does.
    let content: Vec<String> = content_of(&solution, "Acme.Card_MU")["UI"]["Properties"]
        ["_currentParameterDefs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["ParameterName"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(content, ["id", "title", "Alpha", "done"]);
}

#[test]
fn parameter_definitions_join_the_root_properties_in_the_order_the_server_keeps() {
    // A live server read the content back with `_currentParameterDefs` before `id_index`.
    for master in [false, true] {
        let (_dir, solution) = one_project();
        let mut new = request("Acme.Sorted_MU");
        new.master = master;
        new.params = vec!["id:LONG".into()];
        let plan = plan_new(&solution, &new).unwrap();
        let value: Value = serde_json::from_str(&plan.assets.content).unwrap();
        let keys: Vec<&String> = value["UI"]["Properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "master: {master}");
    }
}

#[test]
fn describe_shows_each_container_with_its_child_parameters_and_their_bindings() {
    let (_dir, solution) = parent_and_child();
    let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
    add.base_type = Some("STRING".into());
    param(&solution, &add).unwrap();
    let description = crate::core::mashup::describe::describe(&solution, "Acme.Board_MU").unwrap();
    let text = crate::core::mashup::describe::render(&description);
    assert!(
        text.contains("mashupcontainer-9 shows Acme.Card_MU (id IN, title BOTH)"),
        "{text}"
    );
    assert!(
        text.contains("ui:mashupcontainer-9.id -> ui:ptcslabel-3.LabelText"),
        "{text}"
    );
}

#[test]
fn a_navigation_function_opening_the_mashup_gets_both_its_parameter_caches_refreshed() {
    let (dir, solution) = parent_and_child();
    // Navigation functions sit beside the page container, under the root widget.
    rewrite_mashup(dir.path(), "Acme.Board_MU", |value| {
        value["UI"]["Widgets"].as_array_mut().unwrap().push(json!({
            "Properties": {"Id": "navigationfunction-11", "Type": "navigationfunction",
                "DisplayName": "OpenCard", "TargetMashup": "Acme.Card_MU",
                "TargetWindowType": "modal-popup", "MashupParameters": [],
                "_currentParameterDefs": []},
            "Widgets": []
        }));
        value["DataBindings"].as_array_mut().unwrap().push(json!({
            "Id": "parent-2",
            "PropertyMaps": [{"SourceProperty": "LabelText", "SourcePropertyBaseType": "STRING",
                "SourcePropertyType": "property", "TargetProperty": "id",
                "TargetPropertyBaseType": "LONG", "TargetPropertyType": "property"}],
            "SourceArea": "UI", "SourceDetails": "", "SourceId": "ptcslabel-3", "SourceSection": "",
            "TargetArea": "UI", "TargetId": "navigationfunction-11", "TargetSection": ""
        }));
    });
    let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
    add.base_type = Some("STRING".into());
    let plan = param(&solution, &add).unwrap();
    assert!(plan.refreshed.contains(&(
        "navigationfunction-11".to_string(),
        "Acme.Board_MU".to_string()
    )));
    let parent = content_of(&solution, "Acme.Board_MU");
    let navigation = &parent["UI"]["Widgets"][1]["Properties"];
    let names: Vec<&str> = navigation["MashupParameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["ParameterName"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["id", "title"]);
    assert_eq!(
        serde_json::to_string(&navigation["_currentParameterDefs"]).unwrap(),
        r#"[{"BaseType":"LONG","Description":"","ParameterName":"id","bindingDirection":"Target","bindingType":"Property","isBindingSource":false,"isBindingTarget":true,"shown":true},{"BaseType":"STRING","Description":"","ParameterName":"title","bindingDirection":"Both","bindingType":"Property","isBindingSource":true,"isBindingTarget":true,"shown":true}]"#
    );
    // A navigation function keeps no static values of its own.
    assert!(navigation.get("id").is_none() && navigation.get("title").is_none());
    let mut set = param_request("Acme.Card_MU", ParameterAction::Set, "id");
    set.base_type = Some("STRING".into());
    let plan = param(&solution, &set).unwrap();
    assert!(
        plan.retyped.contains(&"parent-2".to_string()),
        "{:?}",
        plan.retyped
    );
    let parent = content_of(&solution, "Acme.Board_MU");
    assert_eq!(
        parent["DataBindings"][1]["PropertyMaps"][0]["TargetPropertyBaseType"],
        "STRING"
    );
}

/// (id, DisplayName, flex-direction, min height, min width) of every widget below the root.
fn tree_of(name: &str, master: bool) -> Vec<(String, String, String, String, String)> {
    let template = mashup::templates::layout(name).unwrap().unwrap();
    let document = new_document(&template, master, DEFAULT_THEME).unwrap();
    document
        .widgets()
        .into_iter()
        .skip(1)
        .map(|widget| {
            let p = &document.at(&widget.path)["Properties"];
            let text = |key: &str| p[key].as_str().unwrap_or("").to_string();
            (
                widget.id,
                text("DisplayName"),
                text("flex-direction"),
                text("flex-min-height"),
                text("flex-min-width"),
            )
        })
        .collect()
}

fn rows(rows: &[(&str, &str, &str, &str, &str)]) -> Vec<(String, String, String, String, String)> {
    rows.iter()
        .map(|(a, b, c, d, e)| {
            (
                a.to_string(),
                b.to_string(),
                c.to_string(),
                d.to_string(),
                e.to_string(),
            )
        })
        .collect()
}

#[test]
fn the_sidebar_header_shell_has_the_stock_master_shape() {
    assert_eq!(
        tree_of("sidebar-header", true),
        rows(&[
            ("flexcontainer-2", "page", "row", "", ""),
            ("flexcontainer-3", "sidebar", "row", "", ""),
            ("ptcsmenubar-4", "ptcs-menubar-4", "", "", ""),
            ("flexcontainer-5", "main", "column", "", ""),
            ("flexcontainer-6", "header", "row", "50px", ""),
            ("flexcontainer-7", "divider-7", "row", "1px", ""),
            ("flexcontainer-8", "center", "column", "", ""),
            ("pagemashupcontainer-9", "Page Mashup", "", "", ""),
        ])
    );
}

#[test]
fn the_dashboard_has_four_tiles_above_two_panels() {
    assert_eq!(
        tree_of("dashboard", false),
        rows(&[
            ("flexcontainer-2", "page", "column", "", ""),
            ("flexcontainer-3", "title", "row", "40px", ""),
            ("flexcontainer-4", "tiles", "row", "240px", ""),
            ("flexcontainer-5", "tile-1", "column", "", ""),
            ("flexcontainer-6", "tile-2", "column", "", ""),
            ("flexcontainer-7", "tile-3", "column", "", ""),
            ("flexcontainer-8", "tile-4", "column", "", ""),
            ("flexcontainer-9", "panels", "row", "", ""),
            ("flexcontainer-10", "side", "column", "", ""),
            ("flexcontainer-11", "main", "column", "", ""),
        ])
    );
    let template = mashup::templates::layout("dashboard").unwrap().unwrap();
    let document = new_document(&template, false, DEFAULT_THEME).unwrap();
    let main = document.find("main").unwrap();
    assert_eq!(document.at(&main.path)["Properties"]["flex-grow"], 3);
}

fn tabs_request(name: &str, tabs: &[&str]) -> NewRequest {
    NewRequest {
        template: "tabs".into(),
        tabs: tabs.iter().map(|tab| tab.to_string()).collect(),
        ..request(name)
    }
}

#[test]
fn a_tabs_page_shows_a_mashup_in_each_tab_as_composer_lays_them_out() {
    let (_dir, solution) = parent_and_child();
    let plan = plan_new(
        &solution,
        &tabs_request(
            "Acme.Tabs_MU",
            &["Card=Acme.Card_MU", "[[Acme.Board]]=Acme.Board_MU", "Notes"],
        ),
    )
    .unwrap();
    assert_eq!(
        plan.tabs,
        [
            ("Card".to_string(), Some("Acme.Card_MU".to_string())),
            (
                "[[Acme.Board]]".to_string(),
                Some("Acme.Board_MU".to_string())
            ),
            ("Notes".to_string(), None),
        ]
    );
    let document = mashup::edit::parse(&plan.assets.content).unwrap();
    let tabset = document.find("ptcstabset-5").unwrap();
    let properties = &document.at(&tabset.path)["Properties"];
    assert_eq!(properties["NumberOfTabs"], 3);
    assert_eq!(properties["Tab2Name"], "[[Acme.Board]]");
    assert_eq!(properties["Tab3Value"], "TabValue3");
    assert_eq!(
        serde_json::to_string(&properties["Items"]).unwrap(),
        r#"[{"disabled":false,"name":"Card","value":"TabValue1","visible":true},{"disabled":false,"name":"[[Acme.Board]]","value":"TabValue2","visible":true},{"disabled":false,"name":"Notes","value":"TabValue3","visible":true}]"#
    );
    // One container per tab, named after the tab set, holding the mashup the tab shows.
    let tabs = document.at(&tabset.path)["Widgets"].as_array().unwrap();
    assert_eq!(tabs.len(), 3);
    let first = &tabs[0]["Properties"];
    assert_eq!(first["DisplayName"], "ptcs-tabset-5 #1");
    assert_eq!(first["userCannotRemove"], true);
    assert_eq!(first["Margin"], "");
    let card = &tabs[0]["Widgets"][0]["Properties"];
    assert_eq!(card["Type"], "mashupcontainer");
    assert_eq!(card["Name"], "Acme.Card_MU");
    assert_eq!(card["ResponsiveLayout"], true);
    assert_eq!(card["MashupParameters"][0]["ParameterName"], "id");
    assert_eq!(tabs[1]["Widgets"][0]["Properties"]["Name"], "Acme.Board_MU");
    assert!(tabs[2]["Widgets"].as_array().unwrap().is_empty());
    apply_new(&solution, &plan, &lock(&solution)).unwrap();
    let failing: Vec<_> = crate::core::check::run(&solution)
        .gates
        .into_iter()
        .filter(|gate| !gate.ok() && gate.name != "project")
        .collect();
    // The one finding is the made-up label token, which the solution's table does not have.
    assert_eq!(failing.len(), 1, "{failing:?}");
    assert_eq!(failing[0].name, "localization tokens");
    assert!(failing[0].findings[0].message.starts_with("[[Acme.Board]]"));
    let described = crate::core::mashup::describe::describe(&solution, "Acme.Tabs_MU").unwrap();
    assert_eq!(described.containers.len(), 2);
    assert!(described.containers[0]
        .parameters
        .iter()
        .all(|parameter| parameter.status == "ok"));
}

#[test]
fn a_tabs_page_without_tabs_has_the_two_empty_tabs_a_dropped_tab_set_has() {
    let (_dir, solution) = one_project();
    let plan = plan_new(&solution, &tabs_request("Acme.Tabs_MU", &[])).unwrap();
    assert_eq!(
        plan.tabs,
        [
            ("Tab Name 1".to_string(), None),
            ("Tab Name 2".to_string(), None)
        ]
    );
    let document = mashup::edit::parse(&plan.assets.content).unwrap();
    let tabset = document.find("ptcstabset-5").unwrap();
    assert_eq!(document.at(&tabset.path)["Properties"]["NumberOfTabs"], 2);
    assert_eq!(
        document.at(&tabset.path)["Widgets"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn tab_requests_twaco_cannot_stand_behind_are_refused() {
    let (dir, solution) = parent_and_child();
    let master = NewRequest {
        master: true,
        ..request("Acme.Shell_MM")
    };
    apply_new(
        &solution,
        &plan_new(&solution, &master).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let code = |request: NewRequest| plan_new(&solution, &request).unwrap_err().code();
    assert_eq!(
        code(NewRequest {
            template: "card".into(),
            tabs: vec!["A=Acme.Card_MU".into()],
            ..request("Acme.X_MU")
        }),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(NewRequest {
            master: true,
            ..tabs_request("Acme.X_MU", &[])
        }),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(tabs_request("Acme.X_MU", &["A=Acme.Nope_MU"])),
        ErrorCode::UnknownEntity
    );
    assert_eq!(
        code(tabs_request("Acme.X_MU", &["A=Acme.Shell_MM"])),
        ErrorCode::GuardRefused
    );
    assert_eq!(
        code(tabs_request("Acme.X_MU", &["=Acme.Card_MU"])),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(tabs_request("Acme.X_MU", &["A="])),
        ErrorCode::InvalidArguments
    );
    let many: Vec<String> = (1..=17).map(|n| format!("T{n}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        code(tabs_request("Acme.X_MU", &many)),
        ErrorCode::InvalidArguments
    );
    // Nothing was written for any of them.
    assert!(!dir.path().join("acme/Mashups/Acme.X_MU.xml").exists());
}

#[test]
fn a_set_that_keeps_the_type_only_refreshes_the_copies_and_leaves_bindings_alone() {
    let (dir, solution) = parent_and_child();
    // An odd binding from before: a BOOLEAN into the LONG parameter. Not twaco's to judge
    // unless the type changes.
    rewrite_mashup(dir.path(), "Acme.Card_MU", |value| {
        value["DataBindings"][0]["PropertyMaps"][0]["TargetPropertyBaseType"] = json!("BOOLEAN");
    });
    rewrite_mashup(dir.path(), "Acme.Board_MU", |value| {
        value["UI"]["Widgets"][0]["Widgets"][1]["Properties"]["MashupParameters"] = json!([]);
    });
    let plan = param(
        &solution,
        &param_request("Acme.Card_MU", ParameterAction::Set, "id"),
    )
    .unwrap();
    assert!(plan.retyped.is_empty());
    let parent = content_of(&solution, "Acme.Board_MU");
    assert_eq!(
        container(&parent)["MashupParameters"][0]["ParameterName"],
        "id"
    );
    let child = content_of(&solution, "Acme.Card_MU");
    assert_eq!(
        child["DataBindings"][0]["PropertyMaps"][0]["TargetPropertyBaseType"],
        "BOOLEAN"
    );
}

fn add_function(
    solution: &Solution,
    widget: &str,
    inputs: &[&str],
    properties: &[(&str, &str)],
) -> Result<AddPlan, AuthoringError> {
    let plan = plan_add(
        solution,
        &AddRequest {
            inputs: inputs.iter().map(|input| input.to_string()).collect(),
            properties: properties
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            ..add_request("Acme.Orders_MU", widget)
        },
    )?;
    apply_edit(solution, &plan.edit, &lock(solution))?;
    Ok(plan)
}

#[test]
fn an_expression_goes_beside_the_layout_with_its_inputs_and_wires_like_composers() {
    let (_dir, solution) = bind_fixture();
    let button = plan_add(&solution, &add_request("Acme.Orders_MU", "button")).unwrap();
    apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
    // `expression` is the legacy widget's word; adding it adds its replacement.
    let plan = add_function(
        &solution,
        "expression",
        &["note:STRING"],
        &[
            ("Expression", "Output = note === '';"),
            ("OutputBaseType", "BOOLEAN"),
        ],
    )
    .unwrap();
    assert_eq!(plan.placed.id, "expression2-7");
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let root = value["UI"]["Widgets"].as_array().unwrap();
    assert_eq!(root.len(), 2, "beside the page container, not in it");
    let expression = &root[1]["Properties"];
    assert_eq!(expression["Type"], "expression2");
    assert_eq!(expression["ResponsiveLayout"], false);
    assert!(expression.get("UseTheme").is_none());
    assert_eq!(
        serde_json::to_string(&expression["_currentParameterDefs"]).unwrap(),
        r#"[{"BaseType":"STRING","Description":"","ParameterName":"note","bindingDirection":"Target","bindingType":"Property","isBindingSource":false,"isBindingTarget":true,"shown":true}]"#
    );
    for (from, to) in [
        ("ui:ptcstextfield-3.Text", "ui:expression2-7.note"),
        ("ui:expression2-7.Output", "ui:ptcsbutton-6.Disabled"),
    ] {
        let plan = bind(&solution, from, to).unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    }
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let maps: Vec<(&str, &str)> = value["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["PropertyMaps"][0]["SourcePropertyBaseType"]
                    .as_str()
                    .unwrap(),
                b["PropertyMaps"][0]["TargetPropertyBaseType"]
                    .as_str()
                    .unwrap(),
            )
        })
        .collect();
    assert_eq!(maps, [("STRING", "STRING"), ("BOOLEAN", "BOOLEAN")]);
    assert_eq!(
        bind(
            &solution,
            "ui:ptcstextfield-3.Text",
            "ui:expression2-7.Output"
        )
        .unwrap_err()
        .code(),
        ErrorCode::GuardRefused
    );
    assert_eq!(
        bind(
            &solution,
            "ui:ptcstextfield-3.Text",
            "ui:expression2-7.nope"
        )
        .unwrap_err()
        .code(),
        ErrorCode::UnknownEntity
    );
    for (event, invoke) in [
        ("ui:ptcstextfield-3.Changed", "ui:expression2-7.Evaluate"),
        (
            "ui:expression2-7.Changed",
            "data:Acme.Orders.Manager.GetOrders",
        ),
    ] {
        let plan =
            on(&solution, event, invoke).unwrap_or_else(|e| panic!("{event} -> {invoke}: {e}"));
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    }
    let failing: Vec<_> = crate::core::check::run(&solution)
        .gates
        .into_iter()
        .filter(|gate| !gate.ok() && gate.name != "project")
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
}

#[test]
fn an_events_router_takes_as_many_inputs_as_it_says_of_its_data_type() {
    let (_dir, solution) = bind_fixture();
    let plan = add_function(
        &solution,
        "events-router",
        &[],
        &[("NumberOfInputs", "2"), ("DataType", "STRING")],
    )
    .unwrap();
    let id = plan.placed.id;
    assert!(id.starts_with("eventsrouter-"), "{id}");
    for input in ["Input1", "Input2"] {
        let plan = bind(
            &solution,
            "ui:ptcstextfield-3.Text",
            &format!("ui:{id}.{input}"),
        )
        .unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    }
    assert_eq!(
        bind(
            &solution,
            "ui:ptcstextfield-3.Text",
            &format!("ui:{id}.Input3")
        )
        .unwrap_err()
        .code(),
        ErrorCode::UnknownEntity
    );
    bind(
        &solution,
        &format!("ui:{id}.Output"),
        "ui:ptcslabel-4.LabelText",
    )
    .unwrap();
    on(
        &solution,
        &format!("ui:{id}.Changed"),
        "data:Acme.Orders.Manager.GetOrders",
    )
    .unwrap();
}

#[test]
fn function_widgets_refuse_a_place_in_the_layout_and_inputs_where_they_take_none() {
    let (_dir, solution) = bind_fixture();
    let code = |request: AddRequest| plan_add(&solution, &request).unwrap_err().code();
    assert_eq!(
        code(AddRequest {
            placement: Some(Placement::In("center".into())),
            ..add_request("Acme.Orders_MU", "expression2")
        }),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(AddRequest {
            inputs: vec!["x:STRING".into()],
            ..add_request("Acme.Orders_MU", "button")
        }),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(AddRequest {
            inputs: vec!["x:STRING".into()],
            ..add_request("Acme.Orders_MU", "status-message")
        }),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(AddRequest {
            inputs: vec!["x".into()],
            ..add_request("Acme.Orders_MU", "expression2")
        }),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(AddRequest {
            inputs: vec!["x:STRING".into(), "x:NUMBER".into()],
            ..add_request("Acme.Orders_MU", "validator2")
        }),
        ErrorCode::InvalidArguments
    );
}

#[test]
fn a_child_event_is_a_timestamped_parameter_and_a_parent_reacts_through_an_events_router() {
    let (_dir, solution) = parent_and_child();
    let button = plan_add(&solution, &add_request("Acme.Card_MU", "button")).unwrap();
    apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
    let clicked = format!("ui:{}.Clicked", button.placed.id);
    let plan = plan_event(
        &solution,
        &EventRequest {
            mashup: "Acme.Card_MU".into(),
            name: "saved".into(),
            when: clicked.clone(),
        },
    )
    .unwrap();
    assert!(plan
        .refreshed
        .contains(&("mashupcontainer-9".to_string(), "Acme.Board_MU".to_string())));
    apply_parameter(&solution, &plan, &lock(&solution)).unwrap();
    // The child: the parameter, the expression that sets it to the time, the trigger, the binding.
    let entity = std::fs::read(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
    let saved = mashup_parameters(&entity)
        .unwrap()
        .into_iter()
        .find(|p| p.name == "saved")
        .unwrap();
    assert_eq!(
        (saved.base_type.as_str(), saved.direction.as_str()),
        ("NUMBER", "BOTH")
    );
    let child = content_of(&solution, "Acme.Card_MU");
    let expression = child["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| &w["Properties"])
        .find(|p| p["Type"] == "expression2")
        .unwrap();
    assert_eq!(expression["Expression"], "Output = Date.now();");
    assert_eq!(expression["DataChangeType"], "ALWAYS");
    assert_eq!(expression["OutputBaseType"], "NUMBER");
    let id = expression["Id"].as_str().unwrap();
    let fires = child["Events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["EventHandlerId"] == id)
        .unwrap();
    assert_eq!(fires["EventTriggerEvent"], "Clicked");
    assert_eq!(fires["EventHandlerService"], "Evaluate");
    let sets = child["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["SourceId"] == id)
        .unwrap();
    assert_eq!(sets["TargetArea"], "Mashup");
    assert_eq!(sets["PropertyMaps"][0]["TargetProperty"], "saved");
    // The parent: its copy knows the event.
    let parent = content_of(&solution, "Acme.Board_MU");
    assert!(container(&parent)["MashupParameters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["ParameterName"] == "saved"));
    // The parent reacts: an events router fed by the container's parameter runs the handler.
    let parent_on = |invoke: &str| {
        on_in_mashup(
            &solution,
            "Acme.Board_MU",
            "ui:mashupcontainer-9.saved",
            invoke,
        )
    };
    parent_on("ui:mashupcontainer-9.Refresh").unwrap();
    let parent = content_of(&solution, "Acme.Board_MU");
    let routers: Vec<&Value> = parent["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| &w["Properties"])
        .filter(|p| p["Type"] == "eventsrouter")
        .collect();
    assert_eq!(routers.len(), 1);
    assert_eq!(routers[0]["DataType"], "NUMBER");
    assert_eq!(routers[0]["NumberOfInputs"], "1");
    let router = routers[0]["Id"].as_str().unwrap().to_string();
    let feed = parent["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["TargetId"] == router)
        .unwrap();
    assert_eq!(feed["SourceId"], "mashupcontainer-9");
    assert_eq!(feed["PropertyMaps"][0]["SourceProperty"], "saved");
    assert_eq!(feed["PropertyMaps"][0]["TargetProperty"], "Input1");
    let reacts = parent["Events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["EventTriggerId"] == router)
        .unwrap();
    assert_eq!(reacts["EventTriggerEvent"], "Changed");
    assert_eq!(reacts["EventHandlerService"], "Refresh");
    // A second handler for the same event reuses the router.
    let label = plan_add(&solution, &add_request("Acme.Board_MU", "button")).unwrap();
    apply_edit(&solution, &label.edit, &lock(&solution)).unwrap();
    parent_on(&format!("ui:{}.TriggerClick", label.placed.id)).unwrap();
    let parent = content_of(&solution, "Acme.Board_MU");
    let count = parent["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["Properties"]["Type"] == "eventsrouter")
        .count();
    assert_eq!(count, 1);
    let second = parent["Events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["EventHandlerId"] == label.placed.id.as_str())
        .expect("the second handler was added");
    assert_eq!(second["EventTriggerId"], router.as_str());
    assert_eq!(second["EventTriggerEvent"], "Changed");
    let failing: Vec<_> = crate::core::check::run(&solution)
        .gates
        .into_iter()
        .filter(|gate| !gate.ok() && gate.name != "project")
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
    // An input is not an event, and neither is a name the child lacks.
    let code = |event: &str| {
        plan_on(
            &solution,
            &OnRequest {
                mashup: "Acme.Board_MU".into(),
                event: event.into(),
                invoke: "ui:mashupcontainer-9.Refresh".into(),
            },
        )
        .unwrap_err()
        .code()
    };
    assert_eq!(code("ui:mashupcontainer-9.id"), ErrorCode::GuardRefused);
    assert_eq!(code("ui:mashupcontainer-9.nope"), ErrorCode::UnknownEntity);
    // An event name that is already a parameter is refused.
    assert_eq!(
        plan_event(
            &solution,
            &EventRequest {
                mashup: "Acme.Card_MU".into(),
                name: "saved".into(),
                when: clicked,
            },
        )
        .unwrap_err()
        .code(),
        ErrorCode::AlreadyExists
    );
}

fn on_in_mashup(
    solution: &Solution,
    mashup: &str,
    event: &str,
    invoke: &str,
) -> Result<(), AuthoringError> {
    let plan = plan_on(
        solution,
        &OnRequest {
            mashup: mashup.into(),
            event: event.into(),
            invoke: invoke.into(),
        },
    )?;
    apply_edit(solution, &plan.edit, &lock(solution))
}

#[test]
fn a_popup_closes_itself_with_the_mashups_own_service() {
    let (_dir, solution) = bind_fixture();
    let plan = on(&solution, "mashup:Loaded", "mashup:CloseIfPopup").unwrap();
    let value: Value =
        serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    let event = &value["Events"][0];
    assert_eq!(event["EventHandlerArea"], "Mashup");
    assert_eq!(event["EventHandlerId"], "mashup-root");
    assert_eq!(event["EventHandlerService"], "CloseIfPopup");
    assert_eq!(
        on(&solution, "mashup:Loaded", "mashup:Destroy")
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArguments
    );
}

fn recipe(solution: &Solution, name: &str, args: RecipeArgs) -> Result<RecipePlan, AuthoringError> {
    let plan = plan_recipe(
        solution,
        &RecipeRequest {
            mashup: "Acme.Orders_MU".into(),
            recipe: Recipe::from_args(name, &args)?,
        },
    )?;
    apply_edit(solution, &plan.edit, &lock(solution))?;
    Ok(plan)
}

/// The bind fixture plus a button (ptcsbutton-6), a SaveOrder call it runs, and a mashup
/// Acme.Detail_MU with a STRING parameter note to open as a popup.
fn recipe_fixture() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = bind_fixture();
    let button = plan_add(&solution, &add_request("Acme.Orders_MU", "button")).unwrap();
    apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
    let plan = on(
        &solution,
        "ui:ptcsbutton-6.Clicked",
        "data:Acme.Orders.Manager.SaveOrder",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let detail = NewRequest {
        params: vec!["note:STRING".into()],
        ..request("Acme.Detail_MU")
    };
    apply_new(
        &solution,
        &plan_new(&solution, &detail).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    (dir, solution)
}

fn widget_of<'a>(value: &'a Value, id: &str) -> &'a Value {
    value["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| &w["Properties"])
        .find(|p| p["Id"] == id)
        .unwrap_or_else(|| panic!("{id} under the root"))
}

fn has_event(value: &Value, trigger: &str, event: &str, handler: &str, service: &str) -> bool {
    value["Events"].as_array().unwrap().iter().any(|e| {
        e["EventTriggerId"] == trigger
            && e["EventTriggerEvent"] == event
            && e["EventHandlerId"] == handler
            && e["EventHandlerService"] == service
    })
}

fn has_binding(value: &Value, source: &str, from: &str, target: &str, to: &str) -> bool {
    value["DataBindings"].as_array().unwrap().iter().any(|b| {
        b["SourceId"] == source
            && b["PropertyMaps"][0]["SourceProperty"] == from
            && b["TargetId"] == target
            && b["PropertyMaps"][0]["TargetProperty"] == to
    })
}

#[test]
fn each_recipe_writes_the_wiring_real_mashups_use() {
    let (_dir, solution) = recipe_fixture();
    let input = || vec!["note=ui:ptcstextfield-3.Text".to_string()];
    // enable-when: an expression into Disabled, evaluated on load and as its input changes.
    let plan = recipe(
        &solution,
        "enable-when",
        RecipeArgs {
            widget: Some("ptcsbutton-6".into()),
            condition: Some("note !== ''".into()),
            inputs: input(),
            ..Default::default()
        },
    )
    .unwrap();
    let enable = plan.added[0].clone();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let expression = widget_of(&value, &enable);
    assert_eq!(expression["Type"], "expression2");
    assert_eq!(expression["Expression"], "Output = !(note !== '');");
    assert_eq!(expression["OutputBaseType"], "BOOLEAN");
    assert_eq!(expression["AutoEvaluate"], true);
    assert_eq!(expression["_currentParameterDefs"][0]["BaseType"], "STRING");
    assert!(has_binding(
        &value,
        "ptcstextfield-3",
        "Text",
        &enable,
        "note"
    ));
    assert!(has_binding(
        &value,
        &enable,
        "Output",
        "ptcsbutton-6",
        "Disabled"
    ));
    assert!(has_event(
        &value,
        "mashup-root",
        "Loaded",
        &enable,
        "Evaluate"
    ));
    // show-when: into Visible.
    let plan = recipe(
        &solution,
        "show-when",
        RecipeArgs {
            widget: Some("ptcslabel-4".into()),
            condition: Some("note.length > 3".into()),
            inputs: input(),
            ..Default::default()
        },
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    assert!(has_binding(
        &value,
        &plan.added[0],
        "Output",
        "ptcslabel-4",
        "Visible"
    ));
    // text: a STRING expression into the label's text.
    let plan = recipe(
        &solution,
        "text",
        RecipeArgs {
            widget: Some("ptcslabel-4".into()),
            expression: Some("'Note: ' + note".into()),
            inputs: input(),
            ..Default::default()
        },
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    assert_eq!(
        widget_of(&value, &plan.added[0])["Expression"],
        "Output = ('Note: ' + note);"
    );
    assert!(has_binding(
        &value,
        &plan.added[0],
        "Output",
        "ptcslabel-4",
        "LabelText"
    ));
    // message: a status message when SaveOrder completes.
    let plan = recipe(
        &solution,
        "message",
        RecipeArgs {
            after: Some("data:Acme.Orders.Manager.SaveOrder".into()),
            say: Some("Saved".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let message = widget_of(&value, &plan.added[0]);
    assert_eq!(
        (
            message["Type"].as_str(),
            message["Message"].as_str(),
            message["MessageType"].as_str()
        ),
        (Some("statusmessage"), Some("Saved"), Some("info"))
    );
    assert!(has_event(
        &value,
        "SaveOrder",
        "ServiceInvokeCompleted",
        &plan.added[0],
        "ShowMessage"
    ));
    // confirm: the click opens a confirmation whose action runs the handler.
    let plan = recipe(
        &solution,
        "confirm",
        RecipeArgs {
            widget: Some("ptcsbutton-6".into()),
            then: Some("data:Acme.Orders.Manager.GetOrders".into()),
            title: Some("Reload?".into()),
            say: Some("Reload the orders?".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let confirmation = &plan.added[0];
    assert_eq!(widget_of(&value, confirmation)["TitleText"], "Reload?");
    assert!(has_event(
        &value,
        "ptcsbutton-6",
        "Clicked",
        confirmation,
        "OpenConfirmation"
    ));
    assert!(has_event(
        &value,
        confirmation,
        "ActionClick",
        "Things_Acme.Orders.Manager",
        "GetOrders"
    ));
    // popup: a navigation function opening Acme.Detail_MU with note passed and a reload on close.
    let plan = recipe(
        &solution,
        "popup",
        RecipeArgs {
            widget: Some("ptcsbutton-6".into()),
            target: Some("Acme.Detail_MU".into()),
            size: Some("600x400".into()),
            pass: input(),
            reload: Some("data:Acme.Orders.Manager.GetOrders".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let navigation = widget_of(&value, &plan.added[0]);
    assert_eq!(navigation["TargetMashup"], "Acme.Detail_MU");
    assert_eq!(navigation["TargetWindowType"], "modal-popup");
    assert_eq!(
        (
            navigation["PopupWidth"].as_u64(),
            navigation["PopupHeight"].as_u64()
        ),
        (Some(600), Some(400))
    );
    assert_eq!(navigation["MashupParameters"][0]["ParameterName"], "note");
    assert_eq!(
        navigation["_currentParameterDefs"][0]["bindingDirection"],
        "Both"
    );
    assert!(has_binding(
        &value,
        "ptcstextfield-3",
        "Text",
        &plan.added[0],
        "note"
    ));
    assert!(has_event(
        &value,
        "ptcsbutton-6",
        "Clicked",
        &plan.added[0],
        "Navigate"
    ));
    assert!(has_event(
        &value,
        &plan.added[0],
        "WindowClosed",
        "Things_Acme.Orders.Manager",
        "GetOrders"
    ));
    // close-after: the mashup closes itself when SaveOrder completes.
    recipe(
        &solution,
        "close-after",
        RecipeArgs {
            after: Some("data:Acme.Orders.Manager.SaveOrder".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    assert!(has_event(
        &value,
        "SaveOrder",
        "ServiceInvokeCompleted",
        "mashup-root",
        "CloseIfPopup"
    ));
    // auto-refresh: GetOrders every 30 seconds.
    let plan = recipe(
        &solution,
        "auto-refresh",
        RecipeArgs {
            then: Some("data:Acme.Orders.Manager.GetOrders".into()),
            every: Some(30),
            ..Default::default()
        },
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    assert_eq!(widget_of(&value, &plan.added[0])["RefreshInterval"], 30);
    assert!(has_event(
        &value,
        &plan.added[0],
        "Refresh",
        "Things_Acme.Orders.Manager",
        "GetOrders"
    ));
    // validate: the click checks; true runs the handler, false shows the message.
    let plan = recipe(
        &solution,
        "validate",
        RecipeArgs {
            widget: Some("ptcsbutton-6".into()),
            condition: Some("note !== ''".into()),
            inputs: input(),
            then: Some("data:Acme.Orders.Manager.SaveOrder".into()),
            else_say: Some("A note is required".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let (validator, message) = (&plan.added[0], &plan.added[1]);
    assert_eq!(widget_of(&value, validator)["Type"], "validator2");
    assert!(has_event(
        &value,
        "ptcsbutton-6",
        "Clicked",
        validator,
        "Evaluate"
    ));
    assert!(has_event(
        &value,
        validator,
        "True",
        "Things_Acme.Orders.Manager",
        "SaveOrder"
    ));
    assert!(has_event(
        &value,
        validator,
        "False",
        message,
        "ShowMessage"
    ));
    assert_eq!(widget_of(&value, message)["MessageType"], "error");
    let failing: Vec<_> = crate::core::check::run(&solution)
        .gates
        .into_iter()
        .filter(|gate| !gate.ok() && gate.name != "project")
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
}

#[test]
fn recipe_requests_twaco_cannot_stand_behind_are_refused() {
    let (_dir, solution) = recipe_fixture();
    let code = |name: &str, args: RecipeArgs| recipe(&solution, name, args).unwrap_err().code();
    assert_eq!(
        code("sparkle", RecipeArgs::default()),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "enable-when",
            RecipeArgs {
                widget: Some("ptcsbutton-6".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "message",
            RecipeArgs {
                after: Some("data:Acme.Orders.Manager.SaveOrder".into()),
                say: Some("x".into()),
                from: Some("ui:ptcstextfield-3.Text".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "message",
            RecipeArgs {
                after: Some("ui:ptcsbutton-6".into()),
                say: Some("x".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "popup",
            RecipeArgs {
                widget: Some("ptcsbutton-6".into()),
                target: Some("Acme.Orders_MU".into()),
                ..Default::default()
            }
        ),
        ErrorCode::GuardRefused
    );
    assert_eq!(
        code(
            "popup",
            RecipeArgs {
                widget: Some("ptcsbutton-6".into()),
                target: Some("Acme.Detail_MU".into()),
                window: Some("sideways".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "auto-refresh",
            RecipeArgs {
                then: Some("data:Acme.Orders.Manager.GetOrders".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "enable-when",
            RecipeArgs {
                widget: Some("ptcsbutton-6".into()),
                condition: Some("x".into()),
                inputs: vec!["x".into()],
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    // Nothing was written by any of them.
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    assert_eq!(value["UI"]["Widgets"].as_array().unwrap().len(), 1);
}

#[test]
fn describe_says_what_each_function_widget_does() {
    let (_dir, solution) = recipe_fixture();
    let input = || vec!["note=ui:ptcstextfield-3.Text".to_string()];
    recipe(
        &solution,
        "enable-when",
        RecipeArgs {
            widget: Some("ptcsbutton-6".into()),
            condition: Some("note !== ''".into()),
            inputs: input(),
            ..Default::default()
        },
    )
    .unwrap();
    recipe(
        &solution,
        "popup",
        RecipeArgs {
            widget: Some("ptcsbutton-6".into()),
            target: Some("Acme.Detail_MU".into()),
            pass: input(),
            reload: Some("data:Acme.Orders.Manager.GetOrders".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let description = crate::core::mashup::describe::describe(&solution, "Acme.Orders_MU").unwrap();
    let text = crate::core::mashup::describe::render(&description);
    assert!(text.contains("\nfunctions\n"), "{text}");
    assert!(text.contains("computes `Output = !(note !== '');` (BOOLEAN) from note from ui:ptcstextfield-3.Text; feeds ui:ptcsbutton-6.Disabled; evaluated on mashup:Loaded"), "{text}");
    assert!(text.contains("opens Acme.Detail_MU (modal-popup) on ui:ptcsbutton-6.Clicked; passes note from ui:ptcstextfield-3.Text; when it closes runs data:Acme.Orders.Manager.GetOrders"), "{text}");
}

#[test]
fn chart_columns_and_picker_recipes_configure_widgets_from_the_result_shape() {
    let (_dir, solution) = recipe_fixture();
    let mut ids = Vec::new();
    for widget in ["bar-chart", "pie-chart", "dropdown"] {
        let plan = plan_add(&solution, &add_request("Acme.Orders_MU", widget)).unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        ids.push(plan.placed.id);
    }
    let (bar, pie, dropdown) = (&ids[0], &ids[1], &ids[2]);
    let orders = || Some("data:Acme.Orders.Manager.GetOrders".to_string());
    recipe(
        &solution,
        "chart",
        RecipeArgs {
            widget: Some(bar.clone()),
            data: orders(),
            x: Some("note".into()),
            kind: Some("column".into()),
            ..Default::default()
        },
    )
    .unwrap();
    recipe(
        &solution,
        "chart",
        RecipeArgs {
            widget: Some(pie.clone()),
            data: orders(),
            x: Some("note".into()),
            value: Some("id".into()),
            ..Default::default()
        },
    )
    .unwrap();
    recipe(
        &solution,
        "columns",
        RecipeArgs {
            widget: Some("ptcsgrid-5".into()),
            data: orders(),
            columns: vec!["note:Note".into(), "id:Order:120".into()],
            ..Default::default()
        },
    )
    .unwrap();
    // Run again with the same data call: the grid's binding is already there, not an error.
    recipe(
        &solution,
        "columns",
        RecipeArgs {
            widget: Some("ptcsgrid-5".into()),
            data: orders(),
            columns: vec!["note:Note".into(), "id:Order:120".into()],
            ..Default::default()
        },
    )
    .unwrap();
    recipe(
        &solution,
        "picker",
        RecipeArgs {
            widget: Some(dropdown.clone()),
            data: orders(),
            x: Some("note".into()),
            value: Some("id".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let document = mashup::edit::parse(&sidecar_content(&solution)).unwrap();
    let props =
        |id: &str| document.at(&document.find_exact(id).unwrap().path)["Properties"].clone();
    assert_eq!(props(bar)["XAxisField"], "note");
    assert_eq!(props(bar)["ChartType"], "column");
    assert_eq!(
        (
            props(pie)["DataField"].clone(),
            props(pie)["ValueField"].clone()
        ),
        (json!("note"), json!("id"))
    );
    let format = &props("ptcsgrid-5")["ColumnFormat"]["formatInfo"];
    assert_eq!(format[0]["FieldName"], "note");
    assert_eq!(format[0]["Title"], "Note");
    assert_eq!(format[0]["FormatOptions"]["renderer"], "STRING");
    assert_eq!(format[1]["Title"], "Order");
    assert_eq!(format[1]["Width"], 120);
    assert_eq!(format[1]["FormatOptions"]["renderer"], "NUMBER");
    assert_eq!(format[1]["FormatOptions"]["FormatString"], "0.00");
    assert_eq!(props(dropdown)["DisplayField"], "note");
    assert_eq!(props(dropdown)["DisplayFieldBaseType"], "STRING");
    assert_eq!(props(dropdown)["ValueField"], "id");
    for id in [bar.as_str(), pie.as_str(), "ptcsgrid-5", dropdown.as_str()] {
        assert!(
            has_binding(&value, "GetOrders", "", id, "Data"),
            "{id} gets the result"
        );
    }
    let failing: Vec<_> = crate::core::check::run(&solution)
        .gates
        .into_iter()
        .filter(|gate| !gate.ok() && gate.name != "project")
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
    // Refusals: a field the result lacks, a pie without a value, a value on a bar, a wrong
    // chart type, a non-chart, no columns.
    let code = |name: &str, args: RecipeArgs| recipe(&solution, name, args).unwrap_err().code();
    assert_eq!(
        code(
            "chart",
            RecipeArgs {
                widget: Some(bar.clone()),
                data: orders(),
                x: Some("nope".into()),
                ..Default::default()
            }
        ),
        ErrorCode::UnknownEntity
    );
    assert_eq!(
        code(
            "chart",
            RecipeArgs {
                widget: Some(pie.clone()),
                data: orders(),
                x: Some("note".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "chart",
            RecipeArgs {
                widget: Some(bar.clone()),
                data: orders(),
                x: Some("note".into()),
                value: Some("id".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "chart",
            RecipeArgs {
                widget: Some(bar.clone()),
                data: orders(),
                x: Some("note".into()),
                kind: Some("donut".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "chart",
            RecipeArgs {
                widget: Some("ptcsgrid-5".into()),
                data: orders(),
                x: Some("note".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "columns",
            RecipeArgs {
                widget: Some("ptcsgrid-5".into()),
                ..Default::default()
            }
        ),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(
            "picker",
            RecipeArgs {
                widget: Some(dropdown.clone()),
                data: orders(),
                x: Some("nope".into()),
                ..Default::default()
            }
        ),
        ErrorCode::UnknownEntity
    );
}

#[test]
fn a_retype_changes_both_sides_of_a_binding_between_two_copies_of_the_same_mashup() {
    let (dir, solution) = parent_and_child();
    // A second container of Acme.Card_MU, and a binding from one container's id to the other's.
    rewrite_mashup(dir.path(), "Acme.Board_MU", |value| {
        let first = value["UI"]["Widgets"][0]["Widgets"][1].clone();
        let mut second = first.clone();
        second["Properties"]["Id"] = json!("mashupcontainer-10");
        second["Properties"]["DisplayName"] = json!("mashupcontainer-10");
        value["UI"]["Widgets"][0]["Widgets"]
            .as_array_mut()
            .unwrap()
            .push(second);
        value["DataBindings"] = json!([{
            "Id": "between",
            "PropertyMaps": [{"SourceProperty": "id", "SourcePropertyBaseType": "LONG",
                "SourcePropertyType": "property", "TargetProperty": "id",
                "TargetPropertyBaseType": "LONG", "TargetPropertyType": "property"}],
            "SourceArea": "UI", "SourceDetails": "", "SourceId": "mashupcontainer-9", "SourceSection": "",
            "TargetArea": "UI", "TargetId": "mashupcontainer-10", "TargetSection": ""
        }]);
    });
    let mut set = param_request("Acme.Card_MU", ParameterAction::Set, "id");
    set.base_type = Some("STRING".into());
    param(&solution, &set).unwrap();
    let parent = content_of(&solution, "Acme.Board_MU");
    let map = &parent["DataBindings"][0]["PropertyMaps"][0];
    assert_eq!(map["SourcePropertyBaseType"], "STRING");
    assert_eq!(map["TargetPropertyBaseType"], "STRING");
}

#[test]
fn binding_into_a_navigation_function_refreshes_a_missing_or_stale_copy_first() {
    let (dir, solution) = recipe_fixture();
    let plan = recipe(
        &solution,
        "popup",
        RecipeArgs {
            widget: Some("ptcsbutton-6".into()),
            target: Some("Acme.Detail_MU".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let navigation = plan.added[0].clone();
    // Make the copy stale: note cached as LONG; a new parameter count missing from it.
    let mut add = param_request("Acme.Detail_MU", ParameterAction::Add, "count");
    add.base_type = Some("NUMBER".into());
    let detail = plan_parameter(&solution, &add).unwrap();
    // Apply only the child's own edit, so the parent's copy stays stale.
    apply_edit(&solution, &detail.edits[0], &lock(&solution)).unwrap();
    rewrite_mashup(dir.path(), "Acme.Orders_MU", |value| {
        let widgets = value["UI"]["Widgets"].as_array_mut().unwrap();
        let nav = widgets
            .iter_mut()
            .find(|w| w["Properties"]["Id"] == navigation.as_str())
            .unwrap();
        nav["Properties"]["MashupParameters"][0]["BaseType"] = json!("LONG");
    });
    let plan = bind(
        &solution,
        "ui:ptcstextfield-3.Text",
        &format!("ui:{navigation}.count"),
    );
    assert!(
        plan.is_err(),
        "STRING into the NUMBER count is refused by type, not as unknown"
    );
    assert_eq!(plan.unwrap_err().code(), ErrorCode::GuardRefused);
    let plan = bind(
        &solution,
        "ui:ptcstextfield-3.Text",
        &format!("ui:{navigation}.note"),
    )
    .unwrap();
    assert!(plan.refreshed.contains(&navigation), "{:?}", plan.refreshed);
    let value: Value =
        serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    let nav = widget_of(&value, &navigation);
    let names: Vec<(&str, &str)> = nav["MashupParameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["ParameterName"].as_str().unwrap(),
                e["BaseType"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(names, [("note", "STRING"), ("count", "NUMBER")]);
    assert_eq!(nav["_currentParameterDefs"][1]["ParameterName"], "count");
    let binding = value["DataBindings"].as_array().unwrap().last().unwrap();
    assert_eq!(
        binding["PropertyMaps"][0]["TargetPropertyBaseType"],
        "STRING"
    );
}

#[test]
fn add_places_a_widget_an_extension_project_defines() {
    let (dir, solution) = with_config(
        "[[project]]\nname = \"Acme\"\nroot = \"acme\"\n[[extension]]\nroot = \"widgets\"\n",
    );
    write(
        dir.path(),
        "widgets/src/ide.ts",
        "import { TWWidgetDefinition, property, defaultValue, bindingTarget } from 'typescriptwebpacksupport/widgetIDESupport';\n\
         @TWWidgetDefinition('Acme Label')\nclass AcmeLabel extends TWComposerWidget {\n\
         @property('STRING', bindingTarget, defaultValue('hello')) Value: string;\n\
         @property('INFOTABLE', bindingTarget) Rows: any;\n}\n",
    );
    // Build output beside the source is never read as source.
    write(
        dir.path(),
        "widgets/build/ide.ts",
        "not typescript, and not read",
    );
    let name = "Acme.Orders_MU";
    let new = plan_new(&solution, &request(name)).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    let mut add = add_request(name, "acmelabel");
    add.properties = vec![
        ("Value".into(), "Hi".into()),
        ("Rows".into(), "{\"rows\":[]}".into()),
    ];
    let plan = plan_add(&solution, &add).unwrap();
    assert_eq!(plan.placed.id, "AcmeLabel-3");
    assert_eq!(plan.widget, "AcmeLabel");
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let assets = workspace::read_mashup(new.content_path.parent().unwrap())
        .unwrap()
        .unwrap();
    let document = mashup::edit::parse(&assets.content).unwrap();
    let properties = &document.at(&document.find("AcmeLabel-3").unwrap().path)["Properties"];
    assert_eq!(properties["Type"], "AcmeLabel");
    assert_eq!(properties["__TypeDisplayName"], "Acme Label");
    assert_eq!(properties["DisplayName"], "AcmeLabel-3");
    assert_eq!(properties["Value"], "Hi");
    assert_eq!(properties["Rows"], json!({"rows": []}));
    assert_eq!(properties["Area"], "UI");
    // UseTheme is what Composer adds to its own PTCS widgets, not to an extension's.
    assert!(properties.get("UseTheme").is_none(), "{properties}");

    let error = plan_add(&solution, &add_request(name, "acmemissing")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("nor does an [[extension]] project of this solution define it"),
        "{error}"
    );
}
