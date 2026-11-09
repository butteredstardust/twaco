use super::new::new_document;
use super::*;
use crate::core::mashup::describe::mashup_parameters;
use crate::core::mashup::edit::Placement;
use crate::core::mashup::templates::Visibility;
use crate::core::widgets;
use serde_json::json;
use std::path::Path;

/// Whether a test holds a gate to passing. Not `project`: the fixtures' services have no
/// implementations, which only that gate minds. Not `structure`: it runs only where an export XSD
/// is installed (`TWACO_STRUCTURE_XSD`), so judging it would pass or fail by machine.
fn judged(gate: &crate::core::check::GateResult) -> bool {
    !matches!(gate.name.as_str(), "project" | "structure")
}

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
        from: None,
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
fn copying_a_mashup_keeps_its_sidecars_and_content() {
    let (_dir, solution) = one_project();
    let source = plan_new(&solution, &request("Acme.Reference_MU")).unwrap();
    apply_new(&solution, &source, &lock(&solution)).unwrap();
    let mut copy = request("Acme.Copy_MU");
    copy.from = Some("Acme.Reference_MU".into());
    let plan = plan_new(&solution, &copy).unwrap();
    assert_eq!(plan.assets, source.assets);
    assert!(String::from_utf8_lossy(&plan.entity).contains("name=\"Acme.Copy_MU\""));
    assert!(
        String::from_utf8_lossy(&plan.entity).contains("<RunTimePermissions></RunTimePermissions>")
    );
    apply_new(&solution, &plan, &lock(&solution)).unwrap();
    assert_eq!(
        std::fs::read(source.content_path).unwrap(),
        std::fs::read(plan.content_path).unwrap()
    );
    assert_eq!(
        std::fs::read(source.css_path).unwrap(),
        std::fs::read(plan.css_path).unwrap()
    );
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
    // An editor saved content.json with a byte-order mark: an edit keeps it.
    let content_path = solution
        .src_root()
        .join("Acme.Parameter_MU/mashup/content.json");
    let mut marked = b"\xEF\xBB\xBF".to_vec();
    marked.extend(std::fs::read(&content_path).unwrap());
    std::fs::write(&content_path, marked).unwrap();
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
    assert!(std::fs::read(&content_path)
        .unwrap()
        .starts_with(b"\xEF\xBB\xBF"));
    // As Composer writes it: the default is the root property, not a `DefaultValue`.
    assert_eq!(value["UI"]["Properties"]["header"], "Orders");
    assert!(value["UI"]["Properties"]["_currentParameterDefs"][2]
        .get("DefaultValue")
        .is_none());
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
        .filter(|gate| !gate.ok() && judged(gate))
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
fn a_style_property_takes_a_style_name_or_an_inline_style_object() {
    let shape = widgets::catalog().by_type("shape").unwrap();
    assert_eq!(
        property_value(shape, "ShapeStyle", json!(r#"{"backgroundColor":"red"}"#)).unwrap(),
        json!({"backgroundColor": "red"})
    );
    assert_eq!(
        property_value(shape, "ShapeStyle", json!("Acme.Dot_Style")).unwrap(),
        json!("Acme.Dot_Style")
    );
    assert!(property_value(shape, "ShapeStyle", json!("{not json")).is_err());
}

#[test]
fn prop_parses_the_structured_values_composer_uses() {
    let widget = |name| widgets::catalog().by_type(name).unwrap();
    assert_eq!(
        property_value(
            widget("ptcsdropdown"),
            "ItemMeta",
            json!(r#"{"type":"text"}"#)
        )
        .unwrap(),
        json!({"type": "text"})
    );
    assert_eq!(
        property_value(widget("ptcstabset"), "Items", json!("[]")).unwrap(),
        json!([])
    );
    assert_eq!(
        property_value(
            widget("ptcsgrid"),
            "ColumnFormat",
            json!(r#"{"formatInfo":[]}"#)
        )
        .unwrap(),
        json!({"formatInfo": []})
    );
    assert_eq!(
        property_value(
            widget("ptcskpidial"),
            "TargetFormat",
            json!(r#"{"FieldName":"x"}"#)
        )
        .unwrap(),
        json!({"FieldName": "x"})
    );
    assert_eq!(
        property_value(
            widget("ptcskpidial"),
            "TargetFormat_StateParameters",
            json!("[]")
        )
        .unwrap(),
        json!([])
    );
    assert_eq!(
        property_value(
            widget("ptcsvaluedisplay"),
            "ValueFormat",
            json!(r#"{"FormatString":"0"}"#)
        )
        .unwrap(),
        json!({"FormatString": "0"})
    );
    assert_eq!(
        property_value(widget("ptcskpidial"), "ValueFormat", json!("0000.0")).unwrap(),
        json!("0000.0")
    );
}

#[test]
fn structured_prop_errors_name_the_json_and_shape_needed() {
    let dial = widgets::catalog().by_type("ptcskpidial").unwrap();
    assert_eq!(
        property_value(dial, "TargetFormat", json!("plain"))
            .unwrap_err()
            .to_string(),
        "TargetFormat is structured; --prop TargetFormat=<JSON> needs valid JSON"
    );
    assert_eq!(
        property_value(dial, "TargetFormat_StateParameters", json!(r#"{"x":1}"#))
            .unwrap_err()
            .to_string(),
        "TargetFormat_StateParameters must be a JSON array"
    );
}

#[test]
fn cli_text_follows_the_default_type_when_there_is_no_base_type() {
    let flex = widgets::catalog().by_type("flexcontainer").unwrap();
    assert_eq!(
        property_value(flex, "flex-grow", json!("2")).unwrap(),
        json!(2)
    );
    assert_eq!(
        property_value(flex, "flex-grow", json!("x"))
            .unwrap_err()
            .to_string(),
        "flex-grow must be a JSON number"
    );
    let (name, default) = flex
        .defaults
        .iter()
        .find(|(_, value)| value.is_boolean())
        .expect("a flexcontainer boolean default");
    let flipped = !default.as_bool().unwrap();
    assert_eq!(
        property_value(flex, name, json!(flipped.to_string())).unwrap(),
        json!(flipped)
    );
}

#[test]
fn set_changes_a_flex_container_and_keeps_root_css_in_its_sidecar() {
    let (_dir, solution) = one_project();
    let name = "Acme.Orders_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let set = plan_set(
        &solution,
        &SetRequest {
            mashup: name.into(),
            widget: "flexcontainer-2".into(),
            properties: vec![
                ("flex-grow".into(), json!(2)),
                ("flex-direction".into(), json!("column")),
                ("StyleProperties".into(), json!({"gap": "4px"})),
            ],
            style_from: None,
            styles: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        set.changed.len(),
        2,
        "column is already the blank template's direction"
    );
    apply_edit(&solution, set.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    let root = plan_set(
        &solution,
        &SetRequest {
            mashup: name.into(),
            widget: "mashup-root".into(),
            properties: vec![
                ("CustomClass".into(), json!("app")),
                ("CustomMashupCss".into(), json!(".app { color: red; }")),
            ],
            style_from: None,
            styles: Vec::new(),
        },
    )
    .unwrap();
    apply_edit(&solution, root.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    let directory = solution.src_root().join("Acme.Orders_MU/mashup");
    let content = std::fs::read_to_string(directory.join("content.json")).unwrap();
    assert!(!content.contains("CustomMashupCss"), "{content}");
    assert_eq!(
        std::fs::read_to_string(directory.join("custom.css")).unwrap(),
        ".app { color: red; }\n"
    );
    let entity = std::fs::read(solution.root.join("acme/Mashups/Acme.Orders_MU.xml")).unwrap();
    assert_eq!(
        mashup::extract(&entity).unwrap().css,
        ".app { color: red; }\n"
    );
    let no_change = plan_set(
        &solution,
        &SetRequest {
            mashup: name.into(),
            widget: "mashup-root".into(),
            properties: vec![("CustomClass".into(), json!("app"))],
            style_from: None,
            styles: Vec::new(),
        },
    )
    .unwrap();
    assert!(no_change.changed.is_empty());
    assert!(no_change.edit.is_none());
}

#[test]
fn setting_a_label_type_remeasures_its_fixed_row() {
    let (_dir, solution) = one_project();
    let name = "Acme.Orders_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let first = plan_add(&solution, &add_request(name, "label")).unwrap();
    apply_edit(&solution, &first.edit, &lock(&solution)).unwrap();
    let mut second_request = add_request(name, "label");
    second_request.placement = Some(Placement::Beside {
        anchor: first.placed.id.clone(),
        side: crate::core::mashup::edit::Side::Below,
    });
    let second = plan_add(&solution, &second_request).unwrap();
    apply_edit(&solution, &second.edit, &lock(&solution)).unwrap();
    let before = content_of(&solution, name);
    let document = crate::core::mashup::edit::MashupDocument::new(before).unwrap();
    let first = document.find(&first.placed.id).unwrap();
    let row = &first.path[..first.path.len() - 1];
    assert_eq!(document.at(row)["Properties"]["flex-min-height"], "58px");

    set_and_apply(
        &solution,
        name,
        &first.id,
        vec![("LabelType".into(), json!("large-header"))],
        None,
    )
    .unwrap();
    let after = content_of(&solution, name);
    let document = crate::core::mashup::edit::MashupDocument::new(after).unwrap();
    let first = document.find(&first.id).unwrap();
    let row = &first.path[..first.path.len() - 1];
    assert_eq!(document.at(row)["Properties"]["flex-min-height"], "67px");
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
    // This fixture deliberately models the older direct-child layout.  Its wiring tests name
    // stable ids; placement behaviour itself is covered by the focused layout tests below.
    for widget in ["text-field", "label"] {
        let add = add_request(name, widget);
        let plan = plan_add(&solution, &add).unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    }
    let mut document = crate::core::mashup::edit::parse(&sidecar_content(&solution)).unwrap();
    let center = document.find("center").unwrap();
    let number = document.allocate();
    document.push_child(
        &center.path,
        crate::core::mashup::edit::new_widget(
            widgets::catalog().resolve("grid").unwrap(),
            number,
            false,
        ),
    );
    let content = document.to_content();
    let sidecars = solution.src_root().join("Acme.Orders_MU/mashup");
    std::fs::write(sidecars.join("content.json"), &content).unwrap();
    let entity_path = solution.root.join("acme/Mashups/Acme.Orders_MU.xml");
    let (entity, _) = mashup::sync(
        &std::fs::read(&entity_path).unwrap(),
        &mashup::Assets {
            content,
            css: std::fs::read_to_string(sidecars.join("custom.css")).unwrap(),
        },
    )
    .unwrap();
    std::fs::write(entity_path, entity).unwrap();
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
        .filter(|gate| !gate.ok() && judged(gate))
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
fn unbind_removes_one_property_map_of_an_entry_that_carries_several() {
    let (dir, solution) = bind_fixture();
    let binding = bind(
        &solution,
        "ui:ptcstextfield-3.Text",
        "data:Acme.Orders.Manager.SaveOrder.note",
    )
    .unwrap();
    apply_edit(&solution, &binding.edit, &lock(&solution)).unwrap();
    // Composer writes a second target of the same source into the same entry.
    rewrite_mashup(dir.path(), "Acme.Orders_MU", |value| {
        let maps = value["DataBindings"][0]["PropertyMaps"]
            .as_array_mut()
            .unwrap();
        let mut second = maps[0].clone();
        second["TargetProperty"] = json!("title");
        maps.push(second);
    });
    let unbound = plan_unbind(
        &solution,
        &UnbindRequest {
            mashup: "Acme.Orders_MU".into(),
            from: "ui:ptcstextfield-3.Text".into(),
            to: "data:Acme.Orders.Manager.SaveOrder.note".into(),
        },
    )
    .unwrap();
    let content: Value =
        serde_json::from_slice(&unbound.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    let maps = content["DataBindings"][0]["PropertyMaps"]
        .as_array()
        .unwrap();
    assert_eq!(maps.len(), 1, "{content}");
    assert_eq!(maps[0]["TargetProperty"], "title");
}

#[test]
fn remove_unbind_and_off_are_exact_inverses() {
    let (_dir, solution) = bind_fixture();
    let binding = bind(
        &solution,
        "ui:ptcstextfield-3.Text",
        "data:Acme.Orders.Manager.SaveOrder.note",
    )
    .unwrap();
    apply_edit(&solution, &binding.edit, &lock(&solution)).unwrap();
    let unbound = plan_unbind(
        &solution,
        &UnbindRequest {
            mashup: "Acme.Orders_MU".into(),
            from: "ui:ptcstextfield-3.Text".into(),
            to: "data:Acme.Orders.Manager.SaveOrder.note".into(),
        },
    )
    .unwrap();
    let content: Value =
        serde_json::from_slice(&unbound.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    assert!(content["DataBindings"].as_array().unwrap().is_empty());
    apply_edit(&solution, &unbound.edit, &lock(&solution)).unwrap();

    let button = plan_add(&solution, &add_request("Acme.Orders_MU", "button")).unwrap();
    apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
    let added = on(
        &solution,
        "ui:ptcsbutton-6.Clicked",
        "data:Acme.Orders.Manager.SaveOrder",
    )
    .unwrap();
    apply_edit(&solution, &added.edit, &lock(&solution)).unwrap();
    let off = plan_off(
        &solution,
        &OffRequest {
            mashup: "Acme.Orders_MU".into(),
            event: "ui:ptcsbutton-6.Clicked".into(),
            invoke: "data:Acme.Orders.Manager.SaveOrder".into(),
        },
    )
    .unwrap();
    let content: Value =
        serde_json::from_slice(&off.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    assert!(content["Events"].as_array().unwrap().is_empty());
    apply_edit(&solution, &off.edit, &lock(&solution)).unwrap();

    let event = on(
        &solution,
        "ui:ptcsbutton-6.Clicked",
        "data:Acme.Orders.Manager.SaveOrder",
    )
    .unwrap();
    apply_edit(&solution, &event.edit, &lock(&solution)).unwrap();
    let refused = plan_remove(
        &solution,
        &RemoveRequest {
            mashup: "Acme.Orders_MU".into(),
            widget: "ptcsbutton-6".into(),
            cascade: false,
        },
    );
    assert!(matches!(refused, Err(AuthoringError::Refused(_))));
    let removed = plan_remove(
        &solution,
        &RemoveRequest {
            mashup: "Acme.Orders_MU".into(),
            widget: "ptcsbutton-6".into(),
            cascade: true,
        },
    )
    .unwrap();
    assert_eq!(removed.events_removed, 1);
    apply_edit(&solution, removed.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    let no_op = plan_remove(
        &solution,
        &RemoveRequest {
            mashup: "Acme.Orders_MU".into(),
            widget: "ptcsbutton-6".into(),
            cascade: true,
        },
    )
    .unwrap();
    assert!(no_op.edit.is_none());
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
        .filter(|gate| !gate.ok() && judged(gate))
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
    // A contained mashup can fill, so it gets a container of its own beside the label's.
    assert_eq!(ids, ["flexcontainer-5", "flexcontainer-6"]);
    let row = &value["UI"]["Widgets"][0]["Widgets"];
    assert_eq!(row[0]["Widgets"][0]["Properties"]["Id"], "ptcslabel-3");
    let contained = &row[1]["Widgets"][0]["Properties"];
    assert_eq!(contained["Id"], "mashupcontainer-4");
    assert_eq!(contained["ResponsiveLayout"], true);
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
        .filter(|gate| !gate.ok() && judged(gate))
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
        code("na.me", Some("STRING"), ParameterAction::Add),
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
        .filter(|gate| !gate.ok() && judged(gate))
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
        .filter(|gate| !gate.ok() && judged(gate))
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
}

#[test]
fn function_input_defaults_keep_their_json_types() {
    let (_dir, solution) = bind_fixture();
    let plan = add_function(
        &solution,
        "expression2",
        &[
            "flag:BOOLEAN=true",
            "number:NUMBER=2.5",
            "integer:INTEGER=-2",
            "long:LONG=3",
            "json:JSON={\"key\":true}",
            "text:STRING=a=b",
        ],
        &[],
    )
    .unwrap();
    let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let expression = value["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|widget| widget["Properties"]["Id"] == plan.placed.id)
        .unwrap();
    let defaults: std::collections::BTreeMap<_, _> = expression["Properties"]
        ["_currentParameterDefs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|definition| {
            (
                definition["ParameterName"].as_str().unwrap(),
                definition["DefaultValue"].clone(),
            )
        })
        .collect();
    assert_eq!(defaults["flag"], serde_json::json!(true));
    assert_eq!(defaults["number"], serde_json::json!(2.5));
    assert_eq!(defaults["integer"], serde_json::json!(-2));
    assert_eq!(defaults["long"], serde_json::json!(3));
    assert_eq!(defaults["json"], serde_json::json!({"key": true}));
    assert_eq!(defaults["text"], serde_json::json!("a=b"));
    let error = plan_add(
        &solution,
        &AddRequest {
            inputs: vec!["flag:BOOLEAN=not-a-boolean".into()],
            ..add_request("Acme.Orders_MU", "expression2")
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("BOOLEAN"), "{error}");
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
        .filter(|gate| !gate.ok() && judged(gate))
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
fn reading_a_grids_selection_turns_single_row_selection_on() {
    let (_dir, solution) = recipe_fixture();
    let grid_of = |solution: &Solution| {
        let document = crate::core::mashup::edit::parse(&sidecar_content(solution)).unwrap();
        let grid = document
            .widgets()
            .into_iter()
            .find(|widget| widget.type_id == "ptcsgrid")
            .unwrap();
        (
            grid.id.clone(),
            document.at(&grid.path)["Properties"].clone(),
        )
    };
    let (grid, before) = grid_of(&solution);
    assert_eq!(before["RowSelection"], "none", "Composer's drop default");
    recipe(
        &solution,
        "enable-when",
        RecipeArgs {
            widget: Some("ptcsbutton-6".into()),
            condition: Some("selected".into()),
            inputs: vec![format!("selected=ui:{grid}.RowSelected")],
            ..Default::default()
        },
    )
    .unwrap();
    let (_, after) = grid_of(&solution);
    assert_eq!(after["RowSelection"], "single");
}

#[test]
fn only_selection_outputs_of_a_grid_without_selection_turn_it_on() {
    let (_dir, solution) = recipe_fixture();
    let content = sidecar_content(&solution);
    let fresh = || crate::core::mashup::edit::parse(&content).unwrap();
    let grid = fresh()
        .widgets()
        .into_iter()
        .find(|widget| widget.type_id == "ptcsgrid")
        .unwrap()
        .id;
    let selection = |document: &crate::core::mashup::edit::MashupDocument| {
        let found = document.find(&grid).unwrap();
        document.at(&found.path)["Properties"]["RowSelection"].clone()
    };
    for property in ["SelectedRows", "SelectedRowsIDs"] {
        let mut document = fresh();
        let note = super::bind::enable_grid_selection_for_test(&mut document, &grid, property);
        assert!(
            note.is_some_and(|note| note.contains("single")),
            "{property}"
        );
        assert_eq!(selection(&document), "single");
    }
    let mut document = fresh();
    assert!(super::bind::enable_grid_selection_for_test(&mut document, &grid, "Data").is_none());
    assert_eq!(selection(&document), "none");
    let found = document.find(&grid).unwrap();
    document.set_property(&found.path, "RowSelection", Value::from("multiple"));
    assert!(
        super::bind::enable_grid_selection_for_test(&mut document, &grid, "SelectedRows").is_none()
    );
    assert_eq!(selection(&document), "multiple");
}

#[test]
fn a_new_grid_has_no_default_title() {
    let (_dir, solution) = one_project();
    let name = "Acme.Orders_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let grid = plan_add(&solution, &add_request(name, "grid")).unwrap();
    apply_edit(&solution, &grid.edit, &lock(&solution)).unwrap();
    let document =
        crate::core::mashup::edit::MashupDocument::new(content_of(&solution, name)).unwrap();
    let found = document.find(&grid.placed.id).unwrap();
    assert_eq!(document.at(&found.path)["Properties"]["Label"], "");
}

#[test]
fn a_new_collection_aligns_its_cells_to_the_top() {
    let (_dir, solution) = one_project();
    let name = "Acme.Orders_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let collection = plan_add(&solution, &add_request(name, "ptcscollection")).unwrap();
    apply_edit(&solution, &collection.edit, &lock(&solution)).unwrap();
    let document =
        crate::core::mashup::edit::MashupDocument::new(content_of(&solution, name)).unwrap();
    let found = document.find(&collection.placed.id).unwrap();
    assert_eq!(
        document.at(&found.path)["Properties"]["ContentAlignment"],
        "top"
    );
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
        .filter(|gate| !gate.ok() && judged(gate))
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
fn rerunning_the_combo_recipe_clears_series_it_no_longer_names() {
    let (_dir, solution) = recipe_fixture();
    let plan = plan_add(&solution, &add_request("Acme.Orders_MU", "combo-chart")).unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let combo = plan.placed.id;
    // A series an earlier configuration named, beyond what the result now gives.
    let mut document = crate::core::mashup::edit::parse(&sidecar_content(&solution)).unwrap();
    let found = document.find(&combo).unwrap();
    document.set_property(&found.path, "DataField2", Value::from("old"));
    let content = document.to_content();
    let sidecars = solution.src_root().join("Acme.Orders_MU/mashup");
    std::fs::write(sidecars.join("content.json"), &content).unwrap();
    let entity_path = solution.root.join("acme/Mashups/Acme.Orders_MU.xml");
    let (entity, _) = mashup::sync(
        &std::fs::read(&entity_path).unwrap(),
        &mashup::Assets {
            content,
            css: std::fs::read_to_string(sidecars.join("custom.css")).unwrap(),
        },
    )
    .unwrap();
    std::fs::write(entity_path, entity).unwrap();
    recipe(
        &solution,
        "chart",
        RecipeArgs {
            widget: Some(combo.clone()),
            data: Some("data:Acme.Orders.Manager.GetOrders".into()),
            x: Some("note".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let document = crate::core::mashup::edit::parse(&sidecar_content(&solution)).unwrap();
    let properties = &document.at(&document.find(&combo).unwrap().path)["Properties"];
    assert_eq!(properties["DataField1"], "id");
    assert_eq!(properties["DataField2"], "");
}

#[test]
fn chart_columns_and_picker_recipes_configure_widgets_from_the_result_shape() {
    let (_dir, solution) = recipe_fixture();
    let mut ids = Vec::new();
    for widget in ["bar-chart", "pie-chart", "dropdown", "combo-chart"] {
        let plan = plan_add(&solution, &add_request("Acme.Orders_MU", widget)).unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        ids.push(plan.placed.id);
    }
    let (bar, pie, dropdown, combo) = (&ids[0], &ids[1], &ids[2], &ids[3]);
    let orders = || Some("data:Acme.Orders.Manager.GetOrders".to_string());
    // A combo chart names its series: every numeric field but the x axis.
    recipe(
        &solution,
        "chart",
        RecipeArgs {
            widget: Some(combo.clone()),
            data: orders(),
            x: Some("note".into()),
            ..Default::default()
        },
    )
    .unwrap();
    {
        let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
        let document = crate::core::mashup::edit::MashupDocument::new(value).unwrap();
        let found = document.find(combo).unwrap();
        let properties = &document.at(&found.path)["Properties"];
        assert_eq!(properties["DataField1"], "id");
        assert_eq!(properties["XAxisField"], "note");
    }
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
        .filter(|gate| !gate.ok() && judged(gate))
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

#[test]
fn add_names_an_extension_root_it_cannot_read() {
    let (_dir, solution) = with_config(
        "[[project]]\nname = \"Acme\"\nroot = \"acme\"\n[[extension]]\nroot = \"missing\"\n",
    );
    let name = "Acme.Orders_MU";
    let new = plan_new(&solution, &request(name)).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    let error = plan_add(&solution, &add_request(name, "acmelabel")).unwrap_err();
    assert!(error.to_string().contains("(unread: missing: "), "{error}");
}

#[test]
fn add_places_a_widget_of_an_extension_installed_on_the_server() {
    let (dir, solution) = one_project();
    // What `types --platform` caches: the mashup builder's combined extension code.
    let code = "(function(TW){ const widgetName = 'acmechat'; const config = {\"flags\":{\"name\":\"Acme Chat\"},\"properties\":{\"Limit\":{\"baseType\":\"INTEGER\"},\"Title\":{\"baseType\":\"STRING\",\"defaultValue\":\"Chat\"}}}; TW.Widget.widgetWrapper.config(widgetName, config); }(TW));";
    write(
        dir.path(),
        ".twaco/platform.json",
        &serde_json::to_string(&json!({ "extension_widgets": code })).unwrap(),
    );
    let name = "Acme.Orders_MU";
    let new = plan_new(&solution, &request(name)).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    let mut add = add_request(name, "acmechat");
    add.properties = vec![("Limit".into(), "20".into())];
    let plan = plan_add(&solution, &add).unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let assets = workspace::read_mashup(new.content_path.parent().unwrap())
        .unwrap()
        .unwrap();
    let document = mashup::edit::parse(&assets.content).unwrap();
    let properties = &document.at(&document.find(&plan.placed.id).unwrap().path)["Properties"];
    assert_eq!(properties["Type"], "acmechat");
    assert_eq!(properties["Title"], "Chat");
    // Read as its INTEGER base type, not as text.
    assert_eq!(properties["Limit"], json!(20));
    assert_eq!(properties["UseTheme"], json!(true));

    let mut wrong = add_request(name, "acmechat");
    wrong.properties = vec![("Limit".into(), "many".into())];
    assert!(plan_add(&solution, &wrong).is_err());
}

#[test]
fn parameters_added_to_a_crlf_mashup_keep_it_crlf() {
    let (_dir, solution) = one_project();
    let name = "Acme.Crlf_MU";
    let new = plan_new(&solution, &request(name)).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    let crlf = String::from_utf8(std::fs::read(&new.entity_path).unwrap())
        .unwrap()
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    std::fs::write(&new.entity_path, crlf).unwrap();
    for parameter in ["Beta", "Alpha", "Zed"] {
        let mut add = param_request(name, ParameterAction::Add, parameter);
        add.base_type = Some("STRING".into());
        add.direction = Some("IN".into());
        param(&solution, &add).unwrap();
    }
    let bytes = std::fs::read(&new.entity_path).unwrap();
    let crlf = bytes.windows(2).filter(|pair| pair == b"\r\n").count();
    let lf = bytes.iter().filter(|&&byte| byte == b'\n').count();
    assert_eq!(crlf, lf, "every line still ends CRLF");
}

#[test]
fn a_new_mashup_differing_only_in_case_is_refused() {
    let (_dir, solution) = one_project();
    let new = plan_new(&solution, &request("Acme.Orders_MU")).unwrap();
    apply_new(&solution, &new, &lock(&solution)).unwrap();
    let error = plan_new(&solution, &request("acme.orders_mu")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("already exists as Acme.Orders_MU, which ThingWorx takes for the same name"),
        "{error}"
    );
}

/// `bind_fixture` plus a ThingTemplate with GetLines, and THINGNAME `Manager` and NUMBER `Count`
/// parameters on the mashup.
fn source_fixture() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = bind_fixture();
    write(
        dir.path(),
        "acme/ThingTemplates/Acme.Orders.Line_TT.xml",
        r#"<Entities><ThingTemplates><ThingTemplate name="Acme.Orders.Line_TT" projectName="Acme" baseThingTemplate="GenericThing"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetLines"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></ThingTemplate></ThingTemplates></Entities>"#,
    );
    for (name, base) in [("Manager", "THINGNAME"), ("Count", "NUMBER")] {
        let mut add = param_request("Acme.Orders_MU", ParameterAction::Add, name);
        add.base_type = Some(base.into());
        param(&solution, &add).unwrap();
    }
    (dir, solution)
}

fn source_request(entity: &str, service: &str) -> SourceAddRequest {
    SourceAddRequest {
        mashup: "Acme.Orders_MU".into(),
        entity: entity.into(),
        service: service.into(),
        dynamic: false,
        on_load: false,
        alias: None,
        entity_from: None,
        refresh: None,
        cache: false,
        cache_keys: Vec::new(),
        cache_strategy: None,
        max_result_sets: None,
    }
}

fn add_source(
    solution: &Solution,
    request: &SourceAddRequest,
) -> Result<SourceAddPlan, AuthoringError> {
    let plan = plan_source_add(solution, request)?;
    apply_edit(solution, &plan.edit, &lock(solution))?;
    Ok(plan)
}

fn content_value(solution: &Solution) -> Value {
    serde_json::from_str(&sidecar_content(solution)).unwrap()
}

fn problems(solution: &Solution) -> Vec<String> {
    let mut found = Vec::new();
    crate::core::mashup::validate::check_content(&sidecar_content(solution), &mut found);
    found.into_iter().map(|problem| problem.rule).collect()
}

#[test]
fn source_add_writes_each_kind_as_composer_does() {
    let (_dir, solution) = source_fixture();
    let mut dynamic_template = source_request("Acme.Orders.Line_TT", "GetLines");
    dynamic_template.dynamic = true;
    for (request, key, kind) in [
        (
            source_request("Acme.Orders.Manager", "GetOrders"),
            "Things_Acme.Orders.Manager",
            "Things",
        ),
        (
            source_request("Acme.Orders.Line_TT", "GetLines"),
            "ThingTemplates_Acme.Orders.Line_TT",
            "ThingTemplates",
        ),
        (
            dynamic_template,
            "DynamicThingTemplates_Acme.Orders.Line_TT",
            "DynamicThingTemplates",
        ),
        // A shape's call is dynamic without being asked.
        (
            source_request("Acme.Orders.Audit_TS", "GetLog"),
            "DynamicThingShapes_Acme.Orders.Audit_TS",
            "DynamicThingShapes",
        ),
    ] {
        let plan = add_source(&solution, &request).unwrap();
        assert_eq!(plan.section, key);
        let section = &content_value(&solution)["Data"][key];
        let keys: Vec<&String> = section.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "DataName",
                "EntityName",
                "EntityType",
                "Id",
                "RefreshInterval",
                "Services"
            ]
        );
        assert_eq!(section["DataName"], key);
        // Dynamic sections name their shape or template too, as Composer writes them.
        assert_eq!(section["EntityName"], request.entity.as_str());
        assert_eq!(section["EntityType"], kind);
        let call = &section["Services"][0];
        let keys: Vec<&String> = call.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "APIMethod",
                "Characteristic",
                "Id",
                "Name",
                "Parameters",
                "RefreshInterval",
                "Target"
            ]
        );
        assert_eq!(call["Name"], request.service.as_str());
        assert_eq!(call["Target"], request.service.as_str());
    }
    let mut thing = source_request("Acme.Orders.Manager", "GetOrder");
    thing.dynamic = true;
    let error = plan_source_add(&solution, &thing).unwrap_err().to_string();
    assert!(error.contains("leave out --dynamic"), "{error}");
    let error = plan_source_add(&solution, &source_request("Acme.Orders.Order_DS", "x"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Acme.Orders.Order_DS"), "{error}");
    let error = plan_source_add(&solution, &source_request("Acme.Orders.Manager", "Nope"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("is not a service"), "{error}");
}

#[test]
fn source_add_on_load_writes_the_call_and_event_together() {
    let (_dir, solution) = source_fixture();
    let before = sidecar_content(&solution);
    let mut request = source_request("Acme.Orders.Manager", "GetOrders");
    request.alias = Some("orders".into());
    request.on_load = true;
    let plan = plan_source_add(&solution, &request).unwrap();
    assert_eq!(sidecar_content(&solution), before, "a plan writes nothing");
    assert_eq!(
        plan.event.as_deref(),
        Some("mashup:Loaded -> data:Acme.Orders.Manager.GetOrders[as orders]")
    );
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let content = content_value(&solution);
    assert_eq!(
        content["Data"]["Things_Acme.Orders.Manager"]["Services"][0]["Name"],
        "orders"
    );
    let event = &content["Events"][0];
    assert_eq!(event["EventTriggerArea"], "Mashup");
    assert_eq!(event["EventTriggerEvent"], "Loaded");
    assert_eq!(event["EventHandlerArea"], "Data");
    assert_eq!(event["EventHandlerService"], "orders");
    let error = plan_source_add(&solution, &request)
        .unwrap_err()
        .to_string();
    assert!(error.contains("already has a call"), "{error}");
}

#[test]
fn source_retarget_rewrites_data_bindings_and_events() {
    let (dir, solution) = source_fixture();
    write(
        dir.path(),
        "acme/Things/Acme.Orders.Replacement.xml",
        r#"<Entities><Things><Thing name="Acme.Orders.Replacement" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#,
    );
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap();
    let binding = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders",
        "ui:ptcsgrid-5.Data",
    )
    .unwrap();
    apply_edit(&solution, &binding.edit, &lock(&solution)).unwrap();
    let event = on(
        &solution,
        "mashup:Loaded",
        "data:Acme.Orders.Manager.GetOrders",
    )
    .unwrap();
    apply_edit(&solution, &event.edit, &lock(&solution)).unwrap();
    let plan = plan_source_retarget(
        &solution,
        &SourceRetargetRequest {
            mashup: "Acme.Orders_MU".into(),
            from: "Acme.Orders.Manager".into(),
            to: "Acme.Orders.Replacement".into(),
        },
    )
    .unwrap();
    assert_eq!(plan.bindings_rewritten, 1);
    assert_eq!(plan.events_rewritten, 1);
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let value = content_value(&solution);
    let key = "Things_Acme.Orders.Replacement";
    assert_eq!(value["Data"][key]["DataName"], key);
    assert_eq!(value["Data"][key]["EntityName"], "Acme.Orders.Replacement");
    assert_eq!(value["DataBindings"][0]["SourceSection"], key);
    assert_eq!(value["Events"][0]["EventHandlerId"], key);
}

#[test]
fn entity_from_writes_the_call_and_its_entity_binding_in_one_edit() {
    let (_dir, solution) = source_fixture();
    let mut request = source_request("Acme.Orders.Audit_TS", "GetLog");
    request.entity_from = Some("mashup:Missing".into());
    let error = plan_source_add(&solution, &request)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("twaco mashup param add Acme.Orders_MU Missing --type THINGNAME"),
        "{error}"
    );
    request.entity_from = Some("mashup:Count".into());
    let error = plan_source_add(&solution, &request)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("is NUMBER; a dynamic source entity needs THINGNAME or STRING"),
        "{error}"
    );
    let mut fixed = source_request("Acme.Orders.Manager", "GetOrders");
    fixed.entity_from = Some("mashup:Manager".into());
    let error = plan_source_add(&solution, &fixed).unwrap_err().to_string();
    assert!(error.contains("runs on itself"), "{error}");
    request.entity_from = Some("mashup:Manager".into());
    add_source(&solution, &request).unwrap();
    let value = content_value(&solution);
    let binding = &value["DataBindings"][0];
    assert_eq!(binding["SourceArea"], "Mashup");
    assert_eq!(binding["SourceId"], "mashup-root");
    assert_eq!(binding["TargetArea"], "Data");
    assert_eq!(binding["TargetId"], "EntityName");
    assert_eq!(
        binding["TargetSection"],
        "DynamicThingShapes_Acme.Orders.Audit_TS"
    );
    assert_eq!(
        binding["PropertyMaps"][0],
        json!({"SourceProperty": "Manager", "SourcePropertyBaseType": "THINGNAME",
               "SourcePropertyType": "Property", "TargetProperty": "value",
               "TargetPropertyBaseType": "THINGNAME", "TargetPropertyType": "Entity"})
    );
    // describe prints the address bind and unbind take, and the mashup is valid.
    let described = crate::core::mashup::describe::describe(&solution, "Acme.Orders_MU").unwrap();
    let rendered = crate::core::mashup::describe::render(&described);
    assert!(
        rendered.contains("mashup:Manager -> data:Acme.Orders.Audit_TS[EntityName]"),
        "{rendered}"
    );
    assert!(problems(&solution).is_empty(), "{:?}", problems(&solution));
    // One entity per section.
    let error = bind(
        &solution,
        "mashup:Manager",
        "data:Acme.Orders.Audit_TS[EntityName]",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("already takes its entity"), "{error}");
}

#[test]
fn calls_append_name_and_cache_as_composer_does() {
    let (_dir, solution) = source_fixture();
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap();
    let second = plan_source_add(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap()
    .call;
    assert_eq!(
        second, "GetOrders_2",
        "a second call of one service gets the next name"
    );
    let mut named = source_request("Acme.Orders.Manager", "GetOrder");
    named.alias = Some("GetOrders".into());
    assert_eq!(
        plan_source_add(&solution, &named).unwrap_err().code(),
        ErrorCode::AlreadyExists
    );
    let mut cached = source_request("Acme.Orders.Manager", "GetOrder");
    cached.cache_keys = vec!["id".into()];
    assert_eq!(
        plan_source_add(&solution, &cached).unwrap_err().to_string(),
        "cache options need --cache"
    );
    cached.cache = true;
    cached.cache_keys = vec!["nope".into()];
    let error = plan_source_add(&solution, &cached).unwrap_err().to_string();
    assert!(error.contains("no parameter nope"), "{error}");
    cached.cache_keys = vec!["id".into()];
    cached.refresh = Some(30);
    add_source(&solution, &cached).unwrap();
    let value = content_value(&solution);
    let section = &value["Data"]["Things_Acme.Orders.Manager"];
    let calls = section["Services"].as_array().unwrap();
    assert_eq!(calls.len(), 2, "one section, two calls");
    assert_eq!(
        calls[1]["Caching"],
        json!({"cache": true, "cacheKeyParams": ["id"], "cacheStrategy": "INSTANCE", "maxResultSets": "10"})
    );
    assert_eq!(calls[1]["RefreshInterval"], 30);
    assert_eq!(section["RefreshInterval"], 0);
}

#[test]
fn dynamic_calls_need_their_entity_to_run() {
    let (_dir, solution) = source_fixture();
    // Without a call, bind points at source add.
    let error = bind(
        &solution,
        "data:Acme.Orders.Audit_TS.GetLog",
        "ui:ptcsgrid-5.Data",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("add its call with twaco mashup source add"),
        "{error}"
    );
    add_source(&solution, &source_request("Acme.Orders.Audit_TS", "GetLog")).unwrap();
    let plan = bind(
        &solution,
        "data:Acme.Orders.Audit_TS.GetLog",
        "ui:ptcsgrid-5.Data",
    )
    .unwrap();
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.contains("has no EntityName binding")),
        "{:?}",
        plan.warnings
    );
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    assert_eq!(problems(&solution), ["unbound-dynamic-entity"]);
    let error = on(
        &solution,
        "mashup:Loaded",
        "data:Acme.Orders.Audit_TS.GetLog",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("has no EntityName binding"), "{error}");
    // A static section's entity is fixed.
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap();
    let error = bind(
        &solution,
        "mashup:Manager",
        "data:Acme.Orders.Manager[EntityName]",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("is static"), "{error}");
    let plan = bind(
        &solution,
        "mashup:Manager",
        "data:Acme.Orders.Audit_TS[EntityName]",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let plan = on(
        &solution,
        "mashup:Loaded",
        "data:Acme.Orders.Audit_TS.GetLog",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    assert!(problems(&solution).is_empty(), "{:?}", problems(&solution));
}

#[test]
fn an_entity_comes_from_a_result_field_and_an_alias_picks_the_section() {
    let (_dir, solution) = source_fixture();
    let mut dynamic = source_request("Acme.Orders.Line_TT", "GetLines");
    dynamic.dynamic = true;
    dynamic.alias = Some("lines".into());
    add_source(&solution, &dynamic).unwrap();
    let mut other = source_request("Acme.Orders.Line_TT", "GetLines");
    other.dynamic = true;
    other.alias = Some("more".into());
    add_source(&solution, &other).unwrap();
    // Two calls in one dynamic section: one section, so no [as] is needed for its entity.
    let plan = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders.note",
        "data:Acme.Orders.Line_TT[EntityName]",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let value = content_value(&solution);
    let binding = value["DataBindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["TargetId"] == "EntityName")
        .unwrap()
        .clone();
    assert_eq!(
        binding["TargetSection"],
        "DynamicThingTemplates_Acme.Orders.Line_TT"
    );
    assert_eq!(binding["SourceArea"], "Data");
    assert_eq!(binding["SourceSection"], "Things_Acme.Orders.Manager");
    assert_eq!(
        binding["PropertyMaps"][0]["SourcePropertyBaseType"],
        "STRING"
    );
    // With a ThingShape's dynamic section and another for the same name absent, a wrong alias
    // names no call.
    let error = bind(
        &solution,
        "mashup:Manager",
        "data:Acme.Orders.Audit_TS[as nothing][EntityName]",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("no dynamic source"), "{error}");
}

#[test]
fn source_remove_takes_the_entity_binding_with_the_last_call_only() {
    let (_dir, solution) = source_fixture();
    let mut request = source_request("Acme.Orders.Audit_TS", "GetLog");
    request.entity_from = Some("mashup:Manager".into());
    add_source(&solution, &request).unwrap();
    request.entity_from = None;
    request.alias = Some("log2".into());
    add_source(&solution, &request).unwrap();
    let plan = bind(
        &solution,
        "data:Acme.Orders.Audit_TS.GetLog[as log2]",
        "ui:ptcsgrid-5.Data",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let remove = |address: &str, cascade: bool| {
        plan_source_remove(
            &solution,
            &SourceRemoveRequest {
                mashup: "Acme.Orders_MU".into(),
                address: address.into(),
                cascade,
            },
        )
    };
    // GetLog has nothing of its own: removing it keeps the entity the other call needs.
    let plan = remove("data:Acme.Orders.Audit_TS.GetLog[as GetLog]", false).unwrap();
    assert_eq!((plan.bindings_removed, plan.events_removed), (0, 0));
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    assert_eq!(
        content_value(&solution)["DataBindings"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let error = remove("data:Acme.Orders.Audit_TS.GetLog", false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("use --cascade"), "{error}");
    let plan = remove("data:Acme.Orders.Audit_TS.GetLog", true).unwrap();
    assert_eq!(
        plan.bindings_removed, 2,
        "its grid binding and the section's entity binding"
    );
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let value = content_value(&solution);
    assert!(value["Data"]
        .get("DynamicThingShapes_Acme.Orders.Audit_TS")
        .is_none());
    assert_eq!(value["DataBindings"], json!([]));
}

#[test]
fn source_list_says_what_sets_each_entity_and_a_plan_writes_nothing() {
    let (_dir, solution) = source_fixture();
    let before = sidecar_content(&solution);
    plan_source_add(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap();
    assert_eq!(sidecar_content(&solution), before, "a plan writes nothing");
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap();
    add_source(&solution, &source_request("Acme.Orders.Audit_TS", "GetLog")).unwrap();
    let mut template = source_request("Acme.Orders.Line_TT", "GetLines");
    template.dynamic = true;
    template.entity_from = Some("mashup:Manager".into());
    add_source(&solution, &template).unwrap();
    let mut rows: Vec<(String, String, String)> = list_sources(&solution, "Acme.Orders_MU")
        .unwrap()
        .into_iter()
        .map(|row| (row.kind, row.alias, row.entity_from))
        .collect();
    rows.sort();
    let expected: Vec<(String, String, String)> = [
        ("dynamic-shape", "GetLog", "UNBOUND"),
        ("dynamic-template", "GetLines", "mashup:Manager"),
        ("things", "GetOrders", "fixed"),
    ]
    .into_iter()
    .map(|(a, b, c)| (a.into(), b.into(), c.into()))
    .collect();
    assert_eq!(rows, expected);
}

#[test]
fn chart_series_fields_root_properties_and_fixed_icons_are_settable() {
    let (_dir, solution) = one_project();
    let name = "Acme.Orders_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let mut add = add_request(name, "ptcschartcombo");
    add.properties = vec![("DataField2".into(), "Speed".into())];
    let chart = plan_add(&solution, &add).unwrap();
    apply_edit(&solution, &chart.edit, &lock(&solution)).unwrap();
    let id = chart.placed.id.clone();
    let set = |widget: &str, properties: Vec<(&str, Value)>| {
        plan_set(
            &solution,
            &SetRequest {
                mashup: name.into(),
                widget: widget.into(),
                properties: properties
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), value))
                    .collect(),
                style_from: None,
                styles: Vec::new(),
            },
        )
    };
    // A series field exists for every series the chart has a label for, and no further.
    let plan = set(&id, vec![("DataField3", json!("Battery"))]).unwrap();
    assert_eq!(plan.changed[0].after, json!("Battery"));
    let error = set(&id, vec![("DataField", json!("x"))])
        .unwrap_err()
        .to_string();
    assert!(error.contains("has no property DataField"), "{error}");
    let error = set(&id, vec![("DataField999", json!("x"))])
        .unwrap_err()
        .to_string();
    assert!(error.contains("has no property DataField999"), "{error}");
    // Fixed axis bounds are numbers, for the axes the chart has.
    let plan = set(&id, vec![("YAxis1MinimumValue", json!("0"))]).unwrap();
    assert_eq!(plan.changed[0].after, json!(0));
    let error = set(&id, vec![("YAxis1MaximumValue", json!("high"))])
        .unwrap_err()
        .to_string();
    assert!(error.contains("must be a JSON number"), "{error}");
    let content: Value =
        serde_json::from_slice(&chart.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    let document = mashup::edit::parse(&content.to_string()).unwrap();
    let path = document.find(&id).unwrap().path;
    assert_eq!(document.at(&path)["Properties"]["DataField2"], "Speed");
    // The root takes Master and the scalar properties it has; parameters and twaco's keys no.
    let plan = set(
        "mashup-root",
        vec![
            ("Master", json!("Acme.Main_MM")),
            ("Title", json!("Orders")),
        ],
    )
    .unwrap();
    assert_eq!(plan.changed.len(), 2);
    let error = set("mashup-root", vec![("id_index", json!(9))])
        .unwrap_err()
        .to_string();
    assert!(error.contains("not id_index"), "{error}");
    let error = set("mashup-root", vec![("NoSuchThing", json!("x"))])
        .unwrap_err()
        .to_string();
    assert!(error.contains("not NoSuchThing"), "{error}");
    // A fixed container gets Composer's fixed icon, and loses it again.
    let plan = set("flexcontainer-2", vec![("flex-size", json!("fixed"))]).unwrap();
    let icon = plan
        .changed
        .iter()
        .find(|change| change.property == "iconClass")
        .unwrap();
    assert_eq!(icon.after, "widgets-flexcontainer-fixed");
    apply_edit(&solution, plan.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    let plan = set("flexcontainer-2", vec![("flex-size", json!("default"))]).unwrap();
    let icon = plan
        .changed
        .iter()
        .find(|change| change.property == "iconClass")
        .unwrap();
    assert_eq!(icon.after, "widgets-flexcontainer");
}

#[test]
fn root_parameter_defaults_are_set_through_mashup_param() {
    let (_dir, solution) = source_fixture();
    let error = plan_set(
        &solution,
        &SetRequest {
            mashup: "Acme.Orders_MU".into(),
            widget: "mashup-root".into(),
            properties: vec![("Manager".into(), json!("Acme.Orders.Manager"))],
            style_from: None,
            styles: Vec::new(),
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("use mashup param set --default"), "{error}");
}

#[test]
fn a_scalar_result_binds_as_its_result_field() {
    let (dir, solution) = bind_fixture();
    write(
        dir.path(),
        "acme/Things/Acme.Orders.Titles.xml",
        r#"<Entities><Things><Thing name="Acme.Orders.Titles" projectName="Acme"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetTitle"><ResultType baseType="STRING"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    );
    let plan = bind(
        &solution,
        "data:Acme.Orders.Titles.GetTitle.result",
        "ui:ptcslabel-4.LabelText",
    )
    .unwrap();
    let value: Value =
        serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    let binding = &value["DataBindings"][0];
    assert_eq!(binding["SourceDetails"], "AllData");
    assert_eq!(
        binding["PropertyMaps"][0],
        json!({"SourceProperty": "result", "SourcePropertyBaseType": "STRING",
               "SourcePropertyType": "Field", "TargetProperty": "LabelText",
               "TargetPropertyBaseType": "STRING", "TargetPropertyType": "property"})
    );
    // An infotable result's fields come from its DataShape; `result` is not one of them.
    let error = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders.result",
        "ui:ptcslabel-4.LabelText",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("result"), "{error}");
}

#[test]
fn strings_bind_into_entity_names_and_numbers_into_numbers() {
    let (_dir, solution) = source_fixture();
    // STRING into THINGNAME, NUMBER into LONG: ThingWorx carries both.
    bind(&solution, "ui:ptcstextfield-3.Text", "mashup:Manager").unwrap();
    bind(
        &solution,
        "mashup:Count",
        "data:Acme.Orders.Manager.GetOrder.id",
    )
    .unwrap();
    // Text is no number.
    let error = bind(
        &solution,
        "ui:ptcstextfield-3.Text",
        "data:Acme.Orders.Manager.SaveOrder.id",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("is STRING but"), "{error}");
}

#[test]
fn cache_strategies_are_composers_three() {
    let (_dir, solution) = source_fixture();
    let mut cached = source_request("Acme.Orders.Manager", "GetOrders");
    cached.cache = true;
    cached.cache_strategy = Some("INVALID".into());
    let error = plan_source_add(&solution, &cached).unwrap_err().to_string();
    assert!(
        error.contains("INSTANCE, SESSION or USER, not INVALID"),
        "{error}"
    );
    cached.cache_strategy = Some("SESSION".into());
    let plan = plan_source_add(&solution, &cached).unwrap();
    let value: Value =
        serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
    assert_eq!(
        value["Data"]["Things_Acme.Orders.Manager"]["Services"][0]["Caching"]["cacheStrategy"],
        "SESSION"
    );
}

#[test]
fn source_remove_takes_only_a_call_address() {
    let (_dir, solution) = source_fixture();
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap();
    for address in [
        "data:Acme.Orders.Manager.GetOrders[SelectedRows].note",
        "data:Acme.Orders.Manager.GetOrders.note",
        "data:Acme.Orders.Manager[EntityName]",
    ] {
        let error = plan_source_remove(
            &solution,
            &SourceRemoveRequest {
                mashup: "Acme.Orders_MU".into(),
                address: address.into(),
                cascade: true,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("takes a data call address"),
            "{address}: {error}"
        );
    }
}

#[test]
fn an_entity_from_a_source_that_cannot_run_is_warned_or_refused() {
    let (dir, solution) = source_fixture();
    write(
        dir.path(),
        "acme/ThingShapes/Acme.Orders.Pick_TS.xml",
        r#"<Entities><ThingShapes><ThingShape name="Acme.Orders.Pick_TS" projectName="Acme"><ServiceDefinitions>
            <ServiceDefinition name="GetLine"><ResultType baseType="THINGNAME"/></ServiceDefinition>
            </ServiceDefinitions></ThingShape></ThingShapes></Entities>"#,
    );
    // An unbound dynamic call's result naming another dynamic call's Thing.
    add_source(&solution, &source_request("Acme.Orders.Pick_TS", "GetLine")).unwrap();
    add_source(&solution, &source_request("Acme.Orders.Audit_TS", "GetLog")).unwrap();
    let plan = bind(
        &solution,
        "data:Acme.Orders.Pick_TS.GetLine.result",
        "data:Acme.Orders.Audit_TS[EntityName]",
    )
    .unwrap();
    assert!(
        plan.warnings.iter().any(|warning| warning
            .contains("DynamicThingShapes_Acme.Orders.Pick_TS has no EntityName binding")),
        "{:?}",
        plan.warnings
    );
    let mut request = source_request("Acme.Orders.Line_TT", "GetLines");
    request.dynamic = true;
    request.entity_from = Some("data:Acme.Orders.Pick_TS.GetLine.result".into());
    let error = plan_source_add(&solution, &request)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot run until one is added"), "{error}");
}

#[test]
fn a_copy_changes_only_identity_and_drops_permissions() {
    let (dir, solution) = one_project();
    let source = plan_new(&solution, &request("Acme.Reference_MU")).unwrap();
    apply_new(&solution, &source, &lock(&solution)).unwrap();
    // The reference carries a description and run-time permissions.
    let path = source.entity_path.clone();
    let xml = std::fs::read_to_string(&path).unwrap();
    let described = xml.replacen("description=\"\"", "description=\"Orders board\"", 1);
    let with_permissions = described.replacen(
        "<RunTimePermissions></RunTimePermissions>",
        "<RunTimePermissions><Permissions resourceName=\"*\"><PropertyRead><Principal isPermitted=\"true\" name=\"Acme.Users\" type=\"Group\"></Principal></PropertyRead></Permissions></RunTimePermissions>",
        1,
    );
    assert_ne!(with_permissions, xml, "the fixture edits applied");
    std::fs::write(&path, &with_permissions).unwrap();
    let _ = dir;
    let mut copy = request("Acme.Copy_MU");
    copy.from = Some("Acme.Reference_MU".into());
    let plan = plan_new(&solution, &copy).unwrap();
    let summary = plan.copy_summary.as_ref().expect("copy summary");
    assert_eq!(summary.widgets, plan.widgets.len().saturating_sub(1));
    let content: Value = serde_json::from_str(&plan.assets.content).unwrap();
    assert_eq!(
        summary.data_calls,
        content["Data"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, section)| section["Services"].as_array().map_or(0, Vec::len))
            .sum::<usize>()
    );
    assert_eq!(
        summary.bindings,
        content["DataBindings"].as_array().map_or(0, Vec::len)
    );
    assert_eq!(
        summary.events,
        content["Events"].as_array().map_or(0, Vec::len)
    );
    assert_eq!(
        summary.parameters,
        crate::core::mashup::describe::mashup_parameters(&plan.entity)
            .unwrap()
            .len()
    );
    let text = copy_summary_lines(&solution, &plan, false);
    assert_eq!(
        text[0],
        "would create mashup Acme.Copy_MU from Acme.Reference_MU"
    );
    assert_eq!(
        text[1],
        format!(
            "  widgets: {}; data calls: {}; bindings: {}; events: {}; parameters: {}",
            summary.widgets,
            summary.data_calls,
            summary.bindings,
            summary.events,
            summary.parameters
        )
    );
    assert_eq!(text.len(), 5);
    let copied = String::from_utf8(plan.entity.clone()).unwrap();
    assert!(
        !copied.contains("Acme.Users"),
        "what the reference granted is gone"
    );
    assert!(copied.contains("<RunTimePermissions><Permissions resourceName=\"*\"><PropertyRead></PropertyRead></Permissions></RunTimePermissions>"));
    assert!(!copied.contains("Orders board"));
    // Undo the identity change: what is left is the reference without its permissions.
    let expected = with_permissions
        .replacen("name=\"Acme.Reference_MU\"", "name=\"Acme.Copy_MU\"", 1)
        .replacen("description=\"Orders board\"", "description=\"\"", 1)
        .replacen(
            "<Principal isPermitted=\"true\" name=\"Acme.Users\" type=\"Group\"></Principal>",
            "",
            1,
        );
    assert_eq!(copied, expected);
    // A plan writes nothing.
    assert!(!plan.entity_path.exists());
    apply_new(&solution, &plan, &lock(&solution)).unwrap();
    let diff = crate::core::mashup::diff::diff(
        &solution,
        "Acme.Reference_MU",
        "Acme.Copy_MU",
        crate::core::mashup::diff::Tier::All,
    )
    .unwrap();
    assert!(diff.equal, "{:?}", diff.differences);
    // The name is now taken.
    let error = plan_new(&solution, &copy).unwrap_err();
    assert_eq!(error.code(), ErrorCode::AlreadyExists, "{error}");
}

#[test]
fn a_copy_refuses_flags_that_shape_a_new_mashup() {
    let (_dir, solution) = one_project();
    let source = plan_new(&solution, &request("Acme.Reference_MU")).unwrap();
    apply_new(&solution, &source, &lock(&solution)).unwrap();
    let mut copy = request("Acme.Copy_MU");
    copy.from = Some("Acme.Reference_MU".into());
    copy.params = vec!["id:LONG".into()];
    let error = plan_new(&solution, &copy).unwrap_err().to_string();
    assert!(error.contains("--param cannot be combined"), "{error}");
    copy.params.clear();
    copy.template = "sidebar".into();
    let error = plan_new(&solution, &copy).unwrap_err().to_string();
    assert!(error.contains("--template cannot be combined"), "{error}");
    copy.template = "blank".into();
    copy.from = Some("Acme.Missing_MU".into());
    assert!(plan_new(&solution, &copy).is_err());
}

#[test]
fn retarget_keeps_a_dynamic_entity_and_refuses_what_the_new_entity_lacks() {
    let (dir, solution) = source_fixture();
    write(
        dir.path(),
        "acme/ThingShapes/Acme.Orders.Other_TS.xml",
        r#"<Entities><ThingShapes><ThingShape name="Acme.Orders.Other_TS" projectName="Acme"><ServiceDefinitions>
            <ServiceDefinition name="GetLog"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            </ServiceDefinitions></ThingShape></ThingShapes></Entities>"#,
    );
    write(
        dir.path(),
        "acme/ThingShapes/Acme.Orders.Bare_TS.xml",
        r#"<Entities><ThingShapes><ThingShape name="Acme.Orders.Bare_TS" projectName="Acme"><ServiceDefinitions>
            </ServiceDefinitions></ThingShape></ThingShapes></Entities>"#,
    );
    let mut request = source_request("Acme.Orders.Audit_TS", "GetLog");
    request.entity_from = Some("mashup:Manager".into());
    add_source(&solution, &request).unwrap();
    let retarget = |from: &str, to: &str| {
        plan_source_retarget(
            &solution,
            &SourceRetargetRequest {
                mashup: "Acme.Orders_MU".into(),
                from: from.into(),
                to: to.into(),
            },
        )
    };
    let error = retarget("Acme.Orders.Audit_TS", "Acme.Orders.Bare_TS")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Acme.Orders.Bare_TS has no service GetLog"),
        "{error}"
    );
    let error = retarget("Acme.Orders.Audit_TS", "Acme.Orders.Manager")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("matching Thing, ThingTemplate or ThingShape"),
        "{error}"
    );
    let before = sidecar_content(&solution);
    let plan = retarget("Acme.Orders.Audit_TS", "Acme.Orders.Other_TS").unwrap();
    assert!(plan.dynamic);
    assert_eq!(sidecar_content(&solution), before, "a plan writes nothing");
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let value = content_value(&solution);
    let key = "DynamicThingShapes_Acme.Orders.Other_TS";
    assert_eq!(value["Data"][key]["EntityName"], "Acme.Orders.Other_TS");
    assert_eq!(
        value["DataBindings"][0]["TargetSection"], key,
        "the Manager still names its Thing"
    );
    assert!(problems(&solution).is_empty(), "{:?}", problems(&solution));
    // Back again would collide with nothing; onto an existing section is refused.
    add_source(&solution, &source_request("Acme.Orders.Audit_TS", "GetLog")).unwrap();
    let error = retarget("Acme.Orders.Audit_TS", "Acme.Orders.Other_TS")
        .unwrap_err()
        .to_string();
    assert!(error.contains("already exists"), "{error}");
}

#[test]
fn retarget_refuses_a_parameter_of_another_type() {
    let (dir, solution) = source_fixture();
    write(
        dir.path(),
        "acme/Things/Acme.Orders.Textual.xml",
        r#"<Entities><Things><Thing name="Acme.Orders.Textual" projectName="Acme"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetOrder"><ParameterDefinitions><FieldDefinition name="id" baseType="BOOLEAN"/></ParameterDefinitions><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    );
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrder"),
    )
    .unwrap();
    let plan = bind(
        &solution,
        "mashup:Count",
        "data:Acme.Orders.Manager.GetOrder.id",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let error = plan_source_retarget(
        &solution,
        &SourceRetargetRequest {
            mashup: "Acme.Orders_MU".into(),
            from: "Acme.Orders.Manager".into(),
            to: "Acme.Orders.Textual".into(),
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("parameter id is BOOLEAN, not NUMBER"),
        "{error}"
    );
}

#[test]
fn retarget_checks_consumed_result_fields_before_planning_an_edit() {
    let (dir, solution) = source_fixture();
    for (name, shape) in [
        (
            "Acme.Orders.Missing",
            r#"<DataShape name="Acme.Orders.Missing_DS"><FieldDefinitions><FieldDefinition name="id" baseType="LONG"/></FieldDefinitions></DataShape>"#,
        ),
        (
            "Acme.Orders.Changed",
            r#"<DataShape name="Acme.Orders.Changed_DS"><FieldDefinitions><FieldDefinition name="id" baseType="STRING"/><FieldDefinition name="note" baseType="STRING"/></FieldDefinitions></DataShape>"#,
        ),
        ("Acme.Orders.Unknown", ""),
    ] {
        if !shape.is_empty() {
            write(
                dir.path(),
                &format!("acme/DataShapes/{name}_DS.xml"),
                &format!("<Entities><DataShapes>{shape}</DataShapes></Entities>"),
            );
        }
        let data_shape = if shape.is_empty() {
            "Acme.Orders.NotHere_DS".to_string()
        } else {
            format!("{name}_DS")
        };
        write(
            dir.path(),
            &format!("acme/Things/{name}.xml"),
            &format!(
                r#"<Entities><Things><Thing name="{name}" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE" aspect.dataShape="{data_shape}"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#
            ),
        );
    }
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap();
    let plan = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders",
        "ui:ptcsgrid-5.Data",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let content_path = solution
        .src_root()
        .join("Acme.Orders_MU/mashup/content.json");
    let mut value = content_value(&solution);
    let grid = value["UI"]["Widgets"][0]["Widgets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|widget| widget["Properties"]["Id"] == "ptcsgrid-5")
        .unwrap();
    grid["Properties"]["ColumnFormat"] =
        json!({"formatInfo": [{"FieldName": "id"}, {"FieldName": "note"}]});
    std::fs::write(&content_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    let entity_path = dir.path().join("acme/Mashups/Acme.Orders_MU.xml");
    let assets = workspace::read_mashup(&solution.src_root().join("Acme.Orders_MU/mashup"))
        .unwrap()
        .unwrap();
    let (synced, _) = mashup::sync(&std::fs::read(&entity_path).unwrap(), &assets).unwrap();
    std::fs::write(&entity_path, synced).unwrap();
    let retarget = |to: &str| {
        plan_source_retarget(
            &solution,
            &SourceRetargetRequest {
                mashup: "Acme.Orders_MU".into(),
                from: "Acme.Orders.Manager".into(),
                to: to.into(),
            },
        )
    };
    let before = sidecar_content(&solution);
    let error = retarget("Acme.Orders.Missing").unwrap_err().to_string();
    assert!(
        error.contains("result field note") && error.contains("ptcsgrid-5.ColumnFormat"),
        "{error}"
    );
    assert_eq!(
        sidecar_content(&solution),
        before,
        "a refused plan writes nothing"
    );
    let error = retarget("Acme.Orders.Changed").unwrap_err().to_string();
    assert!(
        error.contains("result field id") && error.contains("STRING instead of LONG"),
        "{error}"
    );
    assert_eq!(
        sidecar_content(&solution),
        before,
        "a refused plan writes nothing"
    );
    // A missing replacement DataShape is deliberately not guessed at.
    retarget("Acme.Orders.Unknown").unwrap();
}

#[test]
fn retarget_notes_when_a_string_chart_axis_depends_on_its_values() {
    let (dir, solution) = source_fixture();
    write(
        dir.path(),
        "acme/Things/Acme.Orders.Axis.xml",
        r#"<Entities><Things><Thing name="Acme.Orders.Axis" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    );
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap();
    let mut add = add_request("Acme.Orders_MU", "ptcschartline");
    add.properties = vec![
        ("XAxisField".into(), "note".into()),
        ("XAxisType".into(), "number".into()),
    ];
    let added = plan_add(&solution, &add).unwrap();
    let chart = added.widget.clone();
    apply_edit(&solution, &added.edit, &lock(&solution)).unwrap();
    let plan = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders",
        &format!("ui:{chart}.Data"),
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let plan = plan_source_retarget(
        &solution,
        &SourceRetargetRequest {
            mashup: "Acme.Orders_MU".into(),
            from: "Acme.Orders.Manager".into(),
            to: "Acme.Orders.Axis".into(),
        },
    )
    .unwrap();
    assert!(
        plan.notes
            .iter()
            .any(|note| note.contains("values that read as numbers")
                && note.contains("XAxisType=label")),
        "{:?}",
        plan.notes
    );
}

#[test]
fn retarget_accepts_what_bind_accepts_and_checks_values_the_call_sets() {
    let (dir, solution) = source_fixture();
    // GetOrder(id LONG) moves to a Thing whose GetOrder takes a NUMBER id: a number fits.
    write(
        dir.path(),
        "acme/Things/Acme.Orders.Numeric.xml",
        r#"<Entities><Things><Thing name="Acme.Orders.Numeric" projectName="Acme"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetOrder"><ParameterDefinitions><FieldDefinition name="id" baseType="NUMBER"/></ParameterDefinitions><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
    );
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrder"),
    )
    .unwrap();
    let plan = bind(
        &solution,
        "mashup:Count",
        "data:Acme.Orders.Manager.GetOrder.id",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let retarget = |to: &str| {
        plan_source_retarget(
            &solution,
            &SourceRetargetRequest {
                mashup: "Acme.Orders_MU".into(),
                from: "Acme.Orders.Manager".into(),
                to: to.into(),
            },
        )
    };
    retarget("Acme.Orders.Numeric").unwrap();
    // A value the call sets itself must suit the new type.
    let content_path = solution
        .src_root()
        .join("Acme.Orders_MU/mashup/content.json");
    let mut value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    value["Data"]["Things_Acme.Orders.Manager"]["Services"][0]["Parameters"] = json!({"id": "abc"});
    std::fs::write(&content_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    // As `twaco sync` would: splice the edited content into the entity.
    let entity_path = dir.path().join("acme/Mashups/Acme.Orders_MU.xml");
    let assets = workspace::read_mashup(&solution.src_root().join("Acme.Orders_MU/mashup"))
        .unwrap()
        .unwrap();
    let (synced, _) =
        crate::core::mashup::sync(&std::fs::read(&entity_path).unwrap(), &assets).unwrap();
    std::fs::write(&entity_path, synced).unwrap();
    let error = retarget("Acme.Orders.Numeric").unwrap_err().to_string();
    assert!(
        error.contains("parameter id is NUMBER; the call sets it to \"abc\""),
        "{error}"
    );
}

#[test]
fn a_copy_escapes_its_names_and_keeps_tables_that_only_mention_principals() {
    let (_dir, solution) = one_project();
    let source = plan_new(&solution, &request("Acme.Reference_MU")).unwrap();
    apply_new(&solution, &source, &lock(&solution)).unwrap();
    // A table whose text mentions a Principal grants nothing.
    let xml = std::fs::read_to_string(&source.entity_path).unwrap();
    let mentioned = xml.replacen(
        "<iosShortcutIconTitle></iosShortcutIconTitle>",
        "<iosShortcutIconTitle><![CDATA[<Principal> docs]]></iosShortcutIconTitle>",
        1,
    );
    assert_ne!(mentioned, xml);
    std::fs::write(&source.entity_path, &mentioned).unwrap();
    let mut copy = request("Acme.R&D_MU");
    copy.from = Some("Acme.Reference_MU".into());
    let plan = plan_new(&solution, &copy).unwrap();
    let copied = String::from_utf8(plan.entity.clone()).unwrap();
    assert!(
        copied.contains("name=\"Acme.R&amp;D_MU\""),
        "the name is escaped"
    );
    assert!(
        copied.contains("name=\"MobileSettings\""),
        "the table stays"
    );
    assert!(copied.contains("<Principal> docs"));
}

/// The bind fixture, a call of `GetOrders` in its mashup, and a cell mashup with an input per
/// field and an output.
fn collection_fixture() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = bind_fixture();
    let plan = bind(
        &solution,
        "data:Acme.Orders.Manager.GetOrders",
        "ui:ptcsgrid-5.Data",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let mut cell = request("Acme.OrderCell_MU");
    cell.params = vec![
        "Id:LONG:IN".into(),
        "Note:STRING".into(),
        "Picked:BOOLEAN:OUT".into(),
    ];
    let plan = plan_new(&solution, &cell).unwrap();
    apply_new(&solution, &plan, &lock(&solution)).unwrap();
    (dir, solution)
}

fn collection_request(mappings: &[(&str, &str)]) -> CollectionAddRequest {
    CollectionAddRequest {
        mashup: "Acme.Orders_MU".into(),
        cell: "Acme.OrderCell_MU".into(),
        data: "data:Acme.Orders.Manager.GetOrders".into(),
        mappings: mappings
            .iter()
            .map(|(field, parameter)| (field.to_string(), parameter.to_string()))
            .collect(),
        uid: Some("id".into()),
        width: Some(200),
        height: Some(120),
        placement: None,
    }
}

fn tree_bytes(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, files);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    walk(dir, &mut files);
    files
}

fn collection_properties(solution: &Solution, id: &str) -> Value {
    fn find(widget: &Value, id: &str) -> Option<Value> {
        if widget["Properties"]["Id"] == id {
            return Some(widget["Properties"].clone());
        }
        widget["Widgets"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|child| find(child, id))
    }
    let content: Value = serde_json::from_str(&sidecar_content(solution)).unwrap();
    find(&content["UI"], id).expect("the collection is in the layout")
}

#[test]
fn collection_add_writes_the_composer_contract_in_one_edit() {
    let (dir, solution) = collection_fixture();
    let before = tree_bytes(dir.path());
    let plan = plan_collection_add(
        &solution,
        &collection_request(&[("id", "Id"), ("note", "Note")]),
    )
    .unwrap();
    assert!(before == tree_bytes(dir.path()), "planning changed a file");
    assert_eq!(
        plan.notes.len(),
        1,
        "no event runs the call: {:?}",
        plan.notes
    );
    assert!(plan.notes[0].contains("--event mashup:Loaded"));
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();

    let properties = collection_properties(&solution, &plan.id);
    assert_eq!(properties["Type"], "ptcscollection");
    assert_eq!(properties["Mashup"], "Acme.OrderCell_MU");
    // An object, as Composer stores it, not a JSON string.
    assert_eq!(
        properties["MashupPropertyBinding"],
        json!({"id": "Id", "note": "Note"})
    );
    assert_eq!(properties["UIDField"], "id");
    assert_eq!(properties["CellWidth"], json!(200));
    assert_eq!(properties["CellHeight"], json!(120));
    let content: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
    let bound = crate::core::bindings::whole_data_bindings(&content);
    assert!(
        bound.iter().any(|binding| binding.widget == plan.id),
        "{bound:?}"
    );
    let report =
        crate::core::mashup::validate::validate(&solution, Some("Acme.Orders_MU")).unwrap();
    assert!(report[0].problems.is_empty(), "{:?}", report[0].problems);
}

#[test]
fn collection_add_notes_nothing_when_an_event_runs_the_call() {
    let (_dir, solution) = collection_fixture();
    let plan = on(
        &solution,
        "mashup:Loaded",
        "data:Acme.Orders.Manager.GetOrders",
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let plan = plan_collection_add(&solution, &collection_request(&[("id", "Id")])).unwrap();
    assert!(plan.notes.is_empty(), "{:?}", plan.notes);
}

#[test]
fn collection_add_refuses_what_the_cell_or_the_result_lacks_and_writes_nothing() {
    let (dir, solution) = collection_fixture();
    let before = tree_bytes(dir.path());
    let refused = |request: CollectionAddRequest, expected: &str| {
        let error = plan_collection_add(&solution, &request)
            .err()
            .unwrap_or_else(|| panic!("{expected}: not refused"))
            .to_string();
        assert!(error.contains(expected), "{expected}: {error}");
    };
    refused(
        collection_request(&[("id", "Missing")]),
        "has no parameter Missing",
    );
    refused(collection_request(&[("id", "Picked")]), "is an output");
    refused(collection_request(&[("gone", "Id")]), "has no field gone");
    refused(
        collection_request(&[("note", "Id")]),
        "field note is STRING",
    );
    refused(collection_request(&[]), "at least one --map");
    refused(
        collection_request(&[("id", "Id"), ("id", "Note")]),
        "row field id is mapped twice",
    );
    refused(
        collection_request(&[("id", "Id"), ("note", "Id")]),
        "cell parameter Id is mapped twice",
    );
    let mut uid = collection_request(&[("id", "Id")]);
    uid.uid = Some("gone".into());
    refused(uid, "has no field gone");
    let mut size = collection_request(&[("id", "Id")]);
    size.width = Some(0);
    refused(size, "positive");
    let mut itself = collection_request(&[("id", "Id")]);
    itself.cell = "Acme.Orders_MU".into();
    refused(itself, "cannot use itself");
    let mut absent = collection_request(&[("id", "Id")]);
    absent.cell = "Acme.Nowhere_MU".into();
    assert!(plan_collection_add(&solution, &absent).is_err());
    let mut field = collection_request(&[("id", "Id")]);
    field.data = "data:Acme.Orders.Manager.GetOrders.id".into();
    refused(field, "INFOTABLE");
    let mut uncalled = collection_request(&[("id", "Id")]);
    uncalled.data = "data:Acme.Orders.Manager.GetOrder".into();
    refused(uncalled, "mashup source add");
    assert!(before == tree_bytes(dir.path()), "a refusal changed a file");
}

#[test]
fn collection_add_checks_fields_of_an_aliased_call_by_its_service() {
    let (dir, solution) = collection_fixture();
    // Rename the call: its name no longer matches the service it runs.
    rewrite_content(dir.path(), |content| {
        for section in content["Data"].as_object_mut().unwrap().values_mut() {
            for call in section["Services"].as_array_mut().into_iter().flatten() {
                if call["Name"] == "GetOrders" {
                    call["Name"] = json!("Orders");
                    call["Target"] = json!("GetOrders");
                }
            }
        }
        for binding in content["DataBindings"].as_array_mut().into_iter().flatten() {
            if binding["SourceId"] == "GetOrders" {
                binding["SourceId"] = json!("Orders");
            }
        }
    });
    let mut request = collection_request(&[("gone", "Id")]);
    request.data = "data:Acme.Orders.Manager.GetOrders[as Orders]".into();
    let error = plan_collection_add(&solution, &request)
        .err()
        .map(|e| e.to_string());
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("has no field gone")),
        "{error:?}"
    );
}

#[test]
fn validate_reports_collection_maps_the_result_or_the_cell_lacks() {
    let (dir, solution) = collection_fixture();
    let plan = plan_collection_add(&solution, &collection_request(&[("id", "Id")])).unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let id = plan.id.clone();
    rewrite_content(dir.path(), |content| {
        fn walk(widget: &mut Value, id: &str) {
            if widget["Properties"]["Id"] == id {
                widget["Properties"]["MashupPropertyBinding"] =
                    json!({"id": "Id", "gone": "Note", "note": "NoSuchParameter"});
            }
            for child in widget["Widgets"].as_array_mut().into_iter().flatten() {
                walk(child, id);
            }
        }
        walk(&mut content["UI"], &id);
    });
    let report =
        crate::core::mashup::validate::validate(&solution, Some("Acme.Orders_MU")).unwrap();
    let found: Vec<(&str, &str)> = report[0]
        .problems
        .iter()
        .map(|p| (p.rule.as_str(), p.message.as_str()))
        .collect();
    assert!(
        found
            .iter()
            .any(|(rule, message)| *rule == "unknown-field-property" && message.contains("gone")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|(rule, message)| *rule == "unknown-cell-parameter"
                && message.contains("NoSuchParameter")),
        "{found:?}"
    );
    // Advisory: the bindings gate, which check runs, does not report a cell parameter.
    assert!(!crate::core::bindings::run(&solution)
        .problems
        .iter()
        .any(|p| p.rule == "unknown-cell-parameter"));
}

/// Two mashups, each a label in the center; the reference styles its label and its container.
fn style_fixture() -> (tempfile::TempDir, Solution) {
    let (dir, solution) = one_project();
    for name in ["Acme.Ref_MU", "Acme.Target_MU"] {
        let plan = plan_new(&solution, &request(name)).unwrap();
        apply_new(&solution, &plan, &lock(&solution)).unwrap();
        let plan = plan_add(&solution, &add_request(name, "label")).unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    }
    let styles = json!({
        "meta": {"type": "WidgetStyler", "version": "1.0"},
        "data": {
            "ptcslabel-3": {"parts": {"$": {"$": {"color": "rgba(1,2,3,1)"}}}, "widget": "ptcs-label"},
            "flexcontainer-2": {"parts": {"$": {"$": {"background": "inherit"}}}, "widget": "flex-container"}
        }
    });
    set_and_apply(
        &solution,
        "Acme.Ref_MU",
        "mashup-root",
        vec![("StyleProperties".into(), styles)],
        None,
    )
    .unwrap();
    (dir, solution)
}

fn set_and_apply(
    solution: &Solution,
    mashup: &str,
    widget: &str,
    properties: Vec<(String, Value)>,
    style_from: Option<&str>,
) -> Result<SetPlan, AuthoringError> {
    let plan = plan_set(
        solution,
        &SetRequest {
            mashup: mashup.into(),
            widget: widget.into(),
            properties,
            style_from: style_from.map(str::to_string),
            styles: Vec::new(),
        },
    )?;
    if let Some(edit) = &plan.edit {
        apply_edit(solution, edit, &lock(solution)).unwrap();
    }
    Ok(plan)
}

fn root_styles(solution: &Solution, mashup: &str) -> Value {
    content_of(solution, mashup)["UI"]["Properties"]["StyleProperties"].clone()
}

fn style(
    solution: &Solution,
    mashup: &str,
    widget: &str,
    styles: &[&str],
    from: Option<&str>,
) -> Result<SetPlan, AuthoringError> {
    let plan = plan_set(
        solution,
        &SetRequest {
            mashup: mashup.into(),
            widget: widget.into(),
            properties: Vec::new(),
            style_from: from.map(str::to_string),
            styles: styles.iter().map(|text| (*text).to_string()).collect(),
        },
    )?;
    if let Some(edit) = &plan.edit {
        apply_edit(solution, edit, &lock(solution)).unwrap();
    }
    Ok(plan)
}

#[test]
fn style_writes_a_new_entry_with_its_styler_and_sorted_parts() {
    let (_dir, solution) = style_fixture();
    let plan = style(
        &solution,
        "Acme.Target_MU",
        "flexcontainer-2",
        &[
            "border-width=1px",
            "background=#ffffff",
            "border-style=solid",
            "border-color=#BEC6CA",
        ],
        None,
    )
    .unwrap();
    assert_eq!(plan.changed.len(), 1, "{:?}", plan.changed);
    let styles = root_styles(&solution, "Acme.Target_MU");
    assert_eq!(styles["meta"]["type"], "WidgetStyler");
    let entry = &styles["data"]["flexcontainer-2"];
    assert_eq!(entry["widget"], "flex-container");
    let css = entry["parts"]["$"]["$"].as_object().unwrap();
    assert_eq!(
        css.keys().collect::<Vec<_>>(),
        ["background", "border-color", "border-style", "border-width"]
    );
    // The same settings again change nothing.
    let again = style(
        &solution,
        "Acme.Target_MU",
        "flexcontainer-2",
        &["background=#ffffff"],
        None,
    )
    .unwrap();
    assert!(again.edit.is_none() && again.changed.is_empty());
}

#[test]
fn style_refines_a_part_and_state_and_an_empty_value_removes_it() {
    let (_dir, solution) = style_fixture();
    style(
        &solution,
        "Acme.Ref_MU",
        "ptcslabel-3",
        &["label@hover:color=#000000"],
        None,
    )
    .unwrap();
    let entry = root_styles(&solution, "Acme.Ref_MU")["data"]["ptcslabel-3"].clone();
    assert_eq!(
        entry["parts"]["$"]["$"]["color"], "rgba(1,2,3,1)",
        "the existing part stays"
    );
    assert_eq!(entry["parts"]["label"]["hover"]["color"], "#000000");
    style(
        &solution,
        "Acme.Ref_MU",
        "ptcslabel-3",
        &["label@hover:color="],
        None,
    )
    .unwrap();
    let entry = root_styles(&solution, "Acme.Ref_MU")["data"]["ptcslabel-3"].clone();
    assert!(entry["parts"].get("label").is_none(), "{entry}");
    // Removing the last property removes the widget's entry, and the others stay.
    let plan = style(&solution, "Acme.Ref_MU", "ptcslabel-3", &["color="], None).unwrap();
    assert_eq!(plan.changed.len(), 1);
    assert!(plan.changed[0].after.is_null());
    let data = root_styles(&solution, "Acme.Ref_MU")["data"].clone();
    assert!(data.get("ptcslabel-3").is_none(), "{data}");
    assert!(data.get("flexcontainer-2").is_some(), "{data}");
}

#[test]
fn style_refines_a_copy_from_style_from_in_one_change() {
    let (_dir, solution) = style_fixture();
    let plan = style(
        &solution,
        "Acme.Target_MU",
        "ptcslabel-3",
        &["font-weight=bold"],
        Some("Acme.Ref_MU:ptcslabel-3"),
    )
    .unwrap();
    assert_eq!(plan.changed.len(), 1, "{:?}", plan.changed);
    assert!(plan.changed[0].before.is_none());
    let css = &root_styles(&solution, "Acme.Target_MU")["data"]["ptcslabel-3"]["parts"]["$"]["$"];
    assert_eq!(css["color"], "rgba(1,2,3,1)");
    assert_eq!(css["font-weight"], "bold");
}

#[test]
fn a_copy_whose_only_property_is_removed_changes_nothing() {
    let (dir, solution) = style_fixture();
    let before = tree_bytes(dir.path());
    // The reference label's entry holds only `color`: copying it and removing that nets nothing.
    let plan = style(
        &solution,
        "Acme.Target_MU",
        "ptcslabel-3",
        &["color="],
        Some("Acme.Ref_MU:ptcslabel-3"),
    )
    .unwrap();
    assert!(plan.changed.is_empty(), "{:?}", plan.changed);
    assert!(plan.edit.is_none());
    assert_eq!(tree_bytes(dir.path()), before);
}

#[test]
fn style_refuses_bad_settings_and_the_mashup_root() {
    let (dir, solution) = style_fixture();
    let before = tree_bytes(dir.path());
    for bad in ["background", "=red", "a b=c", "part@:x=1", ":x=1"] {
        assert!(
            matches!(
                style(&solution, "Acme.Target_MU", "ptcslabel-3", &[bad], None),
                Err(AuthoringError::Invalid(_))
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        style(
            &solution,
            "Acme.Target_MU",
            "mashup-root",
            &["background=red"],
            None
        ),
        Err(AuthoringError::Refused(_))
    ));
    assert_eq!(tree_bytes(dir.path()), before);
}

#[test]
fn style_from_copies_one_entry_and_keeps_the_rest() {
    let (_dir, solution) = style_fixture();
    // The target already styles its container: that entry and its meta stay.
    let own = json!({
        "meta": {"type": "WidgetStyler", "version": "1.0"},
        "data": {"flexcontainer-2": {"parts": {"$": {"$": {"background": "red"}}}, "widget": "flex-container"}}
    });
    set_and_apply(
        &solution,
        "Acme.Target_MU",
        "mashup-root",
        vec![("StyleProperties".into(), own)],
        None,
    )
    .unwrap();
    let plan = set_and_apply(
        &solution,
        "Acme.Target_MU",
        "ptcslabel-3",
        Vec::new(),
        Some("Acme.Ref_MU:ptcslabel-3"),
    )
    .unwrap();
    assert!(plan.style_copy.as_ref().is_some_and(|copy| copy.copied));
    let styles = root_styles(&solution, "Acme.Target_MU");
    assert_eq!(
        styles["data"]["ptcslabel-3"],
        root_styles(&solution, "Acme.Ref_MU")["data"]["ptcslabel-3"]
    );
    assert_eq!(
        styles["data"]["flexcontainer-2"]["parts"]["$"]["$"]["background"],
        "red"
    );
    assert_eq!(styles["meta"]["type"], "WidgetStyler");

    // The same copy again changes nothing and writes nothing.
    let again = plan_set(
        &solution,
        &SetRequest {
            mashup: "Acme.Target_MU".into(),
            widget: "ptcslabel-3".into(),
            properties: Vec::new(),
            style_from: Some("Acme.Ref_MU:ptcslabel-3".into()),
            styles: Vec::new(),
        },
    )
    .unwrap();
    assert!(again.edit.is_none() && again.changed.is_empty());
}

#[test]
fn style_from_creates_the_style_map_and_combines_with_props_in_one_edit() {
    let (_dir, solution) = style_fixture();
    assert!(root_styles(&solution, "Acme.Target_MU")
        .get("data")
        .is_none());
    let plan = set_and_apply(
        &solution,
        "Acme.Target_MU",
        "ptcslabel-3",
        vec![("LabelType".into(), json!("header"))],
        Some("Acme.Ref_MU:ptcslabel-3"),
    )
    .unwrap();
    assert_eq!(plan.changed.len(), 2, "{:?}", plan.changed);
    let styles = root_styles(&solution, "Acme.Target_MU");
    assert_eq!(
        styles["meta"],
        json!({"type": "WidgetStyler", "version": "1.0"})
    );
    assert_eq!(styles["data"]["ptcslabel-3"]["widget"], "ptcs-label");
    let content = content_of(&solution, "Acme.Target_MU");
    assert!(content.to_string().contains("\"LabelType\":\"header\""));
}

#[test]
fn style_from_refuses_what_it_cannot_copy_and_copies_nothing_from_an_unstyled_widget() {
    let (dir, solution) = style_fixture();
    let before = tree_bytes(dir.path());
    let attempt = |widget: &str, reference: &str| {
        plan_set(
            &solution,
            &SetRequest {
                mashup: "Acme.Target_MU".into(),
                widget: widget.into(),
                properties: Vec::new(),
                style_from: Some(reference.into()),
                styles: Vec::new(),
            },
        )
    };
    for (widget, reference, expected) in [
        (
            "ptcslabel-3",
            "Acme.Ref_MU:flexcontainer-2",
            "cannot copy style",
        ),
        (
            "ptcslabel-3",
            "Acme.Nowhere_MU:ptcslabel-3",
            "Acme.Nowhere_MU",
        ),
        ("ptcslabel-3", "Acme.Ref_MU:ptcslabel-99", "ptcslabel-99"),
        ("ptcslabel-3", "Acme.Ref_MU", "<mashup>:<widget>"),
    ] {
        let error = attempt(widget, reference)
            .err()
            .unwrap_or_else(|| panic!("{reference}: not refused"))
            .to_string();
        assert!(error.contains(expected), "{reference}: {error}");
    }
    // The reference's label has a style; the target's label, copied back to the reference,
    // has none: nothing to copy, nothing written.
    let plan = plan_set(
        &solution,
        &SetRequest {
            mashup: "Acme.Ref_MU".into(),
            widget: "ptcslabel-3".into(),
            properties: Vec::new(),
            style_from: Some("Acme.Target_MU:ptcslabel-3".into()),
            styles: Vec::new(),
        },
    )
    .unwrap();
    assert!(plan.style_copy.as_ref().is_some_and(|copy| !copy.copied));
    assert!(plan.edit.is_none());
    assert!(
        before == tree_bytes(dir.path()),
        "a refusal or a no-op changed a file"
    );
}

#[test]
fn style_from_refuses_a_malformed_style_map() {
    let (dir, solution) = style_fixture();
    let path = dir.path().join("src/Acme.Target_MU/mashup/content.json");
    let mut content: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    content["UI"]["Properties"]["StyleProperties"] = json!({"data": "not a map"});
    let text = mashup::edit::parse(&content.to_string())
        .unwrap()
        .to_content();
    std::fs::write(&path, &text).unwrap();
    let entity = dir.path().join("acme/Mashups/Acme.Target_MU.xml");
    let css =
        std::fs::read_to_string(dir.path().join("src/Acme.Target_MU/mashup/custom.css")).unwrap();
    let (xml, _) = mashup::sync(
        &std::fs::read(&entity).unwrap(),
        &mashup::Assets { content: text, css },
    )
    .unwrap();
    std::fs::write(&entity, xml).unwrap();
    let error = plan_set(
        &solution,
        &SetRequest {
            mashup: "Acme.Target_MU".into(),
            widget: "ptcslabel-3".into(),
            properties: Vec::new(),
            style_from: Some("Acme.Ref_MU:ptcslabel-3".into()),
            styles: Vec::new(),
        },
    )
    .err()
    .map(|e| e.to_string());
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("StyleProperties.data must be an object")),
        "{error:?}"
    );
}

#[test]
fn setting_the_root_theme_takes_the_default_or_a_solution_theme_only() {
    let (dir, solution) = style_fixture();
    let theme = |name: &str| {
        plan_set(
            &solution,
            &SetRequest {
                mashup: "Acme.Target_MU".into(),
                widget: "mashup-root".into(),
                properties: vec![("StyleTheme".into(), json!(name))],
                style_from: None,
                styles: Vec::new(),
            },
        )
    };
    assert!(theme(super::new::DEFAULT_THEME).is_ok());
    let error = theme("Acme.Missing").err().map(|e| e.to_string());
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("no StyleTheme Acme.Missing")),
        "{error:?}"
    );
    write(
        dir.path(),
        "acme/StyleThemes/Acme.Theme.xml",
        "<Entities><StyleThemes><StyleTheme name=\"Acme.Theme\" projectName=\"Acme\"></StyleTheme></StyleThemes></Entities>",
    );
    assert!(theme("Acme.Theme").is_ok());
}

/// A mashup with a NUMBER parameter and a shape added as Composer adds one.
fn shape_fixture() -> (tempfile::TempDir, Solution, String) {
    let (dir, solution) = one_project();
    let mut new = request("Acme.Shape_MU");
    new.params = vec!["Level:NUMBER".into()];
    let plan = plan_new(&solution, &new).unwrap();
    apply_new(&solution, &plan, &lock(&solution)).unwrap();
    let mut add = add_request("Acme.Shape_MU", "shape");
    add.properties = vec![("ShapeType".into(), "circle".into())];
    let plan = plan_add(&solution, &add).unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    (dir, solution, plan.placed.id)
}

fn widget_properties(solution: &Solution, mashup: &str, id: &str) -> Value {
    fn find(widget: &Value, id: &str) -> Option<Value> {
        if widget["Properties"]["Id"] == id {
            return Some(widget["Properties"].clone());
        }
        widget["Widgets"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|child| find(child, id))
    }
    find(&content_of(solution, mashup)["UI"], id).expect("the widget is in the layout")
}

#[test]
fn a_shape_is_added_as_composer_writes_one() {
    let (_dir, solution, id) = shape_fixture();
    assert!(id.starts_with("shape-"), "{id}");
    let properties = widget_properties(&solution, "Acme.Shape_MU", &id);
    assert_eq!(properties["Type"], "shape");
    assert_eq!(properties["ShapeType"], "circle");
    assert_eq!(properties["DisplayName"], json!(id));
    assert_eq!(properties["__TypeDisplayName"], "Shape");
    assert_eq!(properties["ShapeFormatter_StateParameters"], json!([]));
    assert_eq!(properties["ShapeStyle"], "DefaultChartStyle");
    // Not a PTCS widget: Composer gives it none of what it adds to a dropped PTCS widget.
    assert!(properties.get("UseTheme").is_none(), "{properties}");
    let report = crate::core::mashup::validate::validate(&solution, Some("Acme.Shape_MU")).unwrap();
    assert!(report[0].problems.is_empty(), "{:?}", report[0].problems);
}

#[test]
fn a_shape_is_wired_set_and_removed_through_its_contract() {
    let (_dir, solution, id) = shape_fixture();
    let plan = plan_bind(
        &solution,
        &BindRequest {
            mashup: "Acme.Shape_MU".into(),
            from: "mashup:Level".into(),
            to: format!("ui:{id}.Data"),
        },
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let plan = plan_bind(
        &solution,
        &BindRequest {
            mashup: "Acme.Shape_MU".into(),
            from: "mashup:Level".into(),
            to: format!("ui:{id}.DynamicRangeMaximum"),
        },
    )
    .unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let set = |name: &str, value: Value| {
        plan_set(
            &solution,
            &SetRequest {
                mashup: "Acme.Shape_MU".into(),
                widget: id.clone(),
                properties: vec![(name.into(), value)],
                style_from: None,
                styles: Vec::new(),
            },
        )
    };
    assert!(set("ShapeType", json!("roundrect")).is_ok());
    assert!(set("ShapeType", json!("hexagon")).is_err());
    assert!(set("EnableDynamicFill", json!(true)).is_ok());

    // Its DisplayName is its Id; that is no reference that stops its removal.
    let plan = plan_remove(
        &solution,
        &RemoveRequest {
            mashup: "Acme.Shape_MU".into(),
            widget: id.clone(),
            cascade: true,
        },
    )
    .unwrap();
    assert_eq!(plan.bindings_removed, 2);
    apply_edit(&solution, plan.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    assert!(!content_of(&solution, "Acme.Shape_MU")
        .to_string()
        .contains(&id));
}

#[test]
fn source_retarget_reads_a_call_without_a_target_as_its_name() {
    let (dir, solution) = source_fixture();
    write(
        dir.path(),
        "acme/Things/Acme.Orders.Replacement.xml",
        r#"<Entities><Things><Thing name="Acme.Orders.Replacement" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#,
    );
    add_source(
        &solution,
        &source_request("Acme.Orders.Manager", "GetOrders"),
    )
    .unwrap();
    // A call Composer wrote without a Target runs the service its Name names.
    rewrite_content(dir.path(), |content| {
        for section in content["Data"].as_object_mut().unwrap().values_mut() {
            for call in section["Services"].as_array_mut().into_iter().flatten() {
                call.as_object_mut().unwrap().remove("Target");
            }
        }
    });
    let plan = plan_source_retarget(
        &solution,
        &SourceRetargetRequest {
            mashup: "Acme.Orders_MU".into(),
            from: "Acme.Orders.Manager".into(),
            to: "Acme.Orders.Replacement".into(),
        },
    );
    assert!(plan.is_ok(), "{:?}", plan.err().map(|e| e.to_string()));
}

fn state_format_definition(dir: &Path, kind: &str) {
    let definition = json!({"stateType":kind,"stateDefinitions":[
        {"comparator":"<","defaultValue":"25","displayString":"Poor","name":"internal", "defaultStyleDefinition":{"backgroundColor":"rgba(238,134,134,1)","displayString":"","fontEmphasisBold":false,"fontEmphasisItalic":false,"fontEmphasisUnderline":false,"foregroundColor":"rgba(238,134,134,1)","image":"Acme.Poor","lineColor":"","lineStyle":"solid","lineThickness":1,"secondaryBackgroundColor":"","textSize":"normal"}},
        {"comparator":"<","defaultValue":"50","displayString":"Fair", "defaultStyleDefinition":"Acme.FairStyle"},
        {"defaultValue":"","displayString":"Good", "defaultStyleDefinition":""}
    ]});
    write(
        dir,
        "acme/StateDefinitions/Acme.Level_ST.xml",
        &format!(
            r#"<Entities><StateDefinitions><StateDefinition name="Acme.Level_ST" projectName="Acme"><content><![CDATA[{definition}]]></content></StateDefinition></StateDefinitions></Entities>"#
        ),
    );
}

fn state_format_request(widget: &str) -> StateFormatSetRequest {
    StateFormatSetRequest {
        mashup: "Acme.Orders_MU".into(),
        widget: widget.into(),
        state: "Acme.Level_ST".into(),
        property: Some("TargetFormat".into()),
        column: None,
        field: None,
        icon_only: false,
    }
}

fn state_format_fixture() -> (tempfile::TempDir, Solution, String) {
    let (dir, solution) = collection_fixture();
    state_format_definition(dir.path(), "numeric");
    let mut request = add_request("Acme.Orders_MU", "ptcskpidial");
    request.placement = Some(Placement::Beside {
        anchor: "ptcsgrid-5".into(),
        side: crate::core::mashup::edit::Side::Right,
    });
    let plan = plan_add(&solution, &request).unwrap();
    let id = plan.placed.id.clone();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    (dir, solution, id)
}

#[test]
fn state_format_numeric_contract_plan_apply_and_identity() {
    use crate::core::commands::{self, Mode};
    let (dir, solution, id) = state_format_fixture();
    let before = tree_bytes(dir.path());
    let mut request = commands::mashup::MashupStateFormatSetRequest {
        request: state_format_request(&id),
        mode: Mode::Plan,
        lock_label: "test state formatting",
    };
    let mut notices = commands::Notices::default();
    let planned = commands::mashup::state_format_set(&solution, &request, &mut notices).unwrap();
    assert!(!planned.applied);
    assert_eq!(before, tree_bytes(dir.path()));
    request.mode = Mode::Apply;
    let applied = commands::mashup::state_format_set(&solution, &request, &mut notices).unwrap();
    let properties = collection_properties(&solution, &id);
    let expected = json!({"FieldName":"Value","StateDefinition":"Acme.Level_ST","StateDefinitionType":"numeric","StateFormats":[
        {"comparator":"<","defaultValue":"25","value":"25","stateName":"Poor","state":{"backgroundColor":"rgba(238,134,134,1)","displayString":"","fontEmphasisBold":false,"fontEmphasisItalic":false,"fontEmphasisUnderline":false,"foregroundColor":"rgba(238,134,134,1)","image":"Acme.Poor","lineColor":"","lineStyle":"solid","lineThickness":1,"secondaryBackgroundColor":"","textSize":"normal"}},
        {"comparator":"<","defaultValue":"50","value":"50","stateName":"Fair","state":"Acme.FairStyle"},
        {"defaultValue":"","stateName":"Good","state":{
            "backgroundColor":"","displayString":"","fontEmphasisBold":false,"fontEmphasisItalic":false,"fontEmphasisUnderline":false,
            "foregroundColor":"","image":"","lineColor":"","lineStyle":"solid","lineThickness":1,"secondaryBackgroundColor":"","textSize":"normal"
        }}
    ]});
    assert_eq!(properties["TargetFormat"], expected);
    assert_eq!(properties["TargetFormat_StateParameters"], json!([]));
    let report = set_plan_json(&solution, &applied.plan, true);
    assert_eq!(report["files"].as_array().unwrap().len(), 2);
    let entity = std::fs::read(dir.path().join("acme/Mashups/Acme.Orders_MU.xml")).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&mashup::extract(&entity).unwrap().content).unwrap(),
        serde_json::from_str::<Value>(&sidecar_content(&solution)).unwrap()
    );
    // Applying takes the workspace lock even for a no-op, refreshing its holder timestamp.
    let content_bytes = || {
        let mut files = tree_bytes(dir.path());
        files.remove(&dir.path().join(".twaco/lock.holder"));
        files
    };
    let before = content_bytes();
    let again = commands::mashup::state_format_set(&solution, &request, &mut notices).unwrap();
    assert!(again.plan.edit.is_none());
    assert!(again.plan.changed.is_empty());
    assert!(
        before == content_bytes(),
        "an identical snapshot changed workspace content"
    );
}

fn state_format_grid_columns(solution: &Solution) {
    let plan = plan_set(solution, &SetRequest { mashup:"Acme.Orders_MU".into(), widget:"ptcsgrid-5".into(), style_from:None, styles:Vec::new(), properties:vec![("ColumnFormat".into(),json!({"formatInfo":[
        {"FieldName":"note","Title":"Status","FormatOptions":{"FormatString":"full","renderer":"STRING","formatInfo":{"StateFormats":[]}}},
        {"FieldName":"id","Title":"Id","FormatOptions":{"FormatString":"0","renderer":"NUMBER"}}
    ]}))] }).unwrap();
    apply_edit(solution, plan.edit.as_ref().unwrap(), &lock(solution)).unwrap();
}

#[test]
fn state_format_string_grid_icons_keep_other_columns_and_options() {
    let (dir, solution) = collection_fixture();
    state_format_definition(dir.path(), "string");
    state_format_grid_columns(&solution);
    let before = collection_properties(&solution, "ptcsgrid-5")["ColumnFormat"].clone();
    let mut request = state_format_request("ptcsgrid-5");
    request.property = None;
    request.column = Some("note".into());
    request.icon_only = true;
    let plan = plan_state_format_set(&solution, &request).unwrap();
    apply_edit(&solution, plan.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    let format = collection_properties(&solution, "ptcsgrid-5")["ColumnFormat"].clone();
    assert_eq!(format["formatInfo"][1], before["formatInfo"][1]);
    let options = &format["formatInfo"][0]["FormatOptions"];
    assert_eq!(options["renderer"], "STRING");
    assert_eq!(options["FormatString"], "notext");
    assert_eq!(options["formatInfo"]["FieldName"], "note");
    assert_eq!(options["formatInfo"]["StateDefinitionType"], "string");
    assert_eq!(options["formatInfo"]["StateFormats"][0]["comparator"], "==");
    assert!(options["formatInfo"]["StateFormats"][2]
        .get("value")
        .is_none());
    assert!(plan_state_format_set(&solution, &request)
        .unwrap()
        .edit
        .is_none());
    request.field = Some("id".into());
    assert!(
        plan_state_format_set(&solution, &request).is_ok(),
        "a string definition also accepts a numeric field"
    );
}

#[test]
fn state_format_refusals_leave_every_byte_alone() {
    let (dir, solution, id) = state_format_fixture();
    state_format_grid_columns(&solution);
    let before = tree_bytes(dir.path());
    let valid = state_format_request(&id);
    let refuse = |request: StateFormatSetRequest, message: &str| {
        let error = plan_state_format_set(&solution, &request)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{error} did not contain {message}");
        assert_eq!(before, tree_bytes(dir.path()));
    };
    let mut request = valid.clone();
    request.state = "Missing".into();
    refuse(request, "no StateDefinition Missing");
    let mut request = valid.clone();
    request.widget = "absent".into();
    refuse(request, "absent");
    for property in ["Absent", "Value"] {
        let mut request = valid.clone();
        request.property = Some(property.into());
        refuse(request, "STATEFORMATTING properties: TargetFormat");
    }
    let mut request = valid.clone();
    request.icon_only = true;
    refuse(request, "--icon-only requires --column");
    let mut request = valid.clone();
    request.column = Some("id".into());
    refuse(request, "exactly one");
    let mut request = valid.clone();
    request.property = None;
    refuse(request, "exactly one");
    let mut grid = valid.clone();
    grid.widget = "ptcsgrid-5".into();
    grid.property = None;
    grid.column = Some("missing".into());
    refuse(grid.clone(), "no column missing");
    grid.column = Some("note".into());
    refuse(grid.clone(), "field note is STRING");
    grid.field = Some("absent".into());
    refuse(grid.clone(), "has no field absent");
    grid.field = Some("id".into());
    let override_plan = plan_state_format_set(&solution, &grid).unwrap();
    assert_eq!(
        override_plan.changed[0].after["formatInfo"][0]["FormatOptions"]["formatInfo"]["FieldName"],
        "id"
    );
    assert_eq!(
        override_plan.changed[0].after["formatInfo"][0]["FormatOptions"]["FormatString"],
        "full"
    );
    grid.column = Some("id".into());
    grid.field = None;
    assert!(plan_state_format_set(&solution, &grid).is_ok());
    let mut request = valid;
    request.property = None;
    request.column = Some("id".into());
    refuse(request, "not a ptcsgrid");
}

#[test]
fn state_format_requires_explicit_field_for_other_widgets_and_checks_data_binding() {
    let (dir, solution) = collection_fixture();
    state_format_definition(dir.path(), "numeric");
    let plan = plan_add(&solution, &add_request("Acme.Orders_MU", "shape")).unwrap();
    let id = plan.placed.id.clone();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let mut request = state_format_request(&id);
    request.property = Some("ShapeFormatter".into());
    assert!(plan_state_format_set(&solution, &request)
        .unwrap_err()
        .to_string()
        .contains("give --field"));
    request.field = Some("Status".into());
    assert!(plan_state_format_set(&solution, &request).is_ok());
    // A whole Data binding also checks a widget property; aliases must resolve to the service target.
    rewrite_content(dir.path(), |content| {
        for section in content["Data"].as_object_mut().unwrap().values_mut() {
            for call in section["Services"].as_array_mut().into_iter().flatten() {
                if call["Name"] == "GetOrders" {
                    call["Name"] = json!("Orders");
                    call["Target"] = json!("GetOrders");
                }
            }
        }
        for binding in content["DataBindings"].as_array_mut().into_iter().flatten() {
            if binding["SourceId"] == "GetOrders" {
                binding["SourceId"] = json!("Orders");
                binding["TargetId"] = json!(id);
            }
        }
    });
    request.field = Some("note".into());
    assert!(plan_state_format_set(&solution, &request)
        .unwrap_err()
        .to_string()
        .contains("field note is STRING"));
    request.field = Some("absent".into());
    assert!(plan_state_format_set(&solution, &request)
        .unwrap_err()
        .to_string()
        .contains("has no field absent"));
    request.field = Some("id".into());
    assert!(plan_state_format_set(&solution, &request).is_ok());
}

#[test]
fn state_format_validate_unknown_and_drift_are_separate_from_check() {
    use crate::core::mashup::validate::{validate, Severity};
    let (dir, solution, id) = state_format_fixture();
    let plan = plan_state_format_set(&solution, &state_format_request(&id)).unwrap();
    apply_edit(&solution, plan.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    let problems = validate(&solution, Some("Acme.Orders_MU"))
        .unwrap()
        .remove(0)
        .problems;
    assert!(!problems
        .iter()
        .any(|p| p.rule == "state-format-drift" || p.rule == "unknown-state-definition"));
    state_format_grid_columns(&solution);
    let plan = plan_set(&solution, &SetRequest { mashup:"Acme.Orders_MU".into(),widget:"ptcsgrid-5".into(),style_from:None,styles:Vec::new(),properties:vec![("ColumnFormat".into(),json!({"formatInfo":[{"FieldName":"note","FormatOptions":{"formatInfo":{"StateDefinition":"Missing"}}}]}))] }).unwrap();
    apply_edit(&solution, plan.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    let state_path = dir.path().join("acme/StateDefinitions/Acme.Level_ST.xml");
    let changed = std::fs::read_to_string(&state_path)
        .unwrap()
        .replace("Poor", "Changed");
    std::fs::write(state_path, changed).unwrap();
    let problems = validate(&solution, Some("Acme.Orders_MU"))
        .unwrap()
        .remove(0)
        .problems;
    assert!(problems
        .iter()
        .any(|p| p.rule == "state-format-drift" && p.severity == Severity::Advisory));
    assert!(problems
        .iter()
        .any(|p| p.rule == "unknown-state-definition" && p.severity == Severity::Error));
    std::fs::remove_file(dir.path().join("acme/StateDefinitions/Acme.Level_ST.xml")).unwrap();
    let problems = validate(&solution, Some("Acme.Orders_MU"))
        .unwrap()
        .remove(0)
        .problems;
    assert_eq!(
        problems
            .iter()
            .filter(|p| p.rule == "unknown-state-definition")
            .count(),
        2
    );
    let report = crate::core::check::run(&solution);
    let check = format!("{report:?}");
    assert!(!check.contains("unknown-state-definition"));
    assert!(!check.contains("state-format-drift"));
    for (widget, property, value) in [
        (
            id.as_str(),
            "TargetFormat",
            json!({"StateDefinition":"","StateFormats":[]}),
        ),
        (
            "ptcsgrid-5",
            "ColumnFormat",
            json!({"formatInfo":[{"FieldName":"note","FormatOptions":{"formatInfo":{"StateDefinition":""}}}]}),
        ),
    ] {
        let plan = plan_set(
            &solution,
            &SetRequest {
                mashup: "Acme.Orders_MU".into(),
                widget: widget.into(),
                style_from: None,
                styles: Vec::new(),
                properties: vec![(property.into(), value)],
            },
        )
        .unwrap();
        apply_edit(&solution, plan.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    }
    let problems = validate(&solution, Some("Acme.Orders_MU"))
        .unwrap()
        .remove(0)
        .problems;
    assert!(
        !problems
            .iter()
            .any(|p| p.rule == "unknown-state-definition" || p.rule == "state-format-drift"),
        "empty references are allowed"
    );
}

#[test]
fn state_format_refuses_a_malformed_grid_format_without_writing() {
    let (dir, solution) = collection_fixture();
    state_format_definition(dir.path(), "string");
    rewrite_content(dir.path(), |content| {
        let mut document = mashup::edit::parse(&mashup::to_sidecar(content)).unwrap();
        let found = document.find_exact("ptcsgrid-5").unwrap();
        document.set_property(&found.path, "ColumnFormat", json!("malformed"));
        *content = document.value().clone();
    });
    let before = tree_bytes(dir.path());
    let mut request = state_format_request("ptcsgrid-5");
    request.property = None;
    request.column = Some("note".into());
    let error = plan_state_format_set(&solution, &request).unwrap_err();
    assert!(error.to_string().contains("no column note"));
    assert!(before == tree_bytes(dir.path()));
}

/// Add one widget and apply it; the id of what was placed.
fn add_one(
    solution: &Solution,
    mashup: &str,
    widget: &str,
    placement: Placement,
    properties: &[(&str, &str)],
) -> String {
    let mut request = add_request(mashup, widget);
    request.placement = Some(placement);
    request.properties = properties
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let plan = plan_add(solution, &request).unwrap();
    apply_edit(solution, &plan.edit, &lock(solution)).unwrap();
    plan.placed.id
}

fn find_widget<'a>(widget: &'a Value, id: &str) -> Option<&'a Value> {
    if widget["Properties"]["Id"] == id {
        return Some(widget);
    }
    widget["Widgets"]
        .as_array()?
        .iter()
        .find_map(|child| find_widget(child, id))
}

fn parent_id(widget: &Value, id: &str) -> Option<String> {
    for child in widget["Widgets"].as_array()? {
        if child["Properties"]["Id"] == id {
            return widget["Properties"]["Id"].as_str().map(str::to_string);
        }
        if let Some(found) = parent_id(child, id) {
            return Some(found);
        }
    }
    None
}

#[test]
fn a_card_of_label_value_rows_and_separators_gets_a_measured_range_row() {
    use crate::core::mashup::edit::Side;
    let (_dir, solution) = one_project();
    let name = "Acme.Kpi_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let beside = |anchor: &str, side| Placement::Beside {
        anchor: anchor.to_string(),
        side,
    };
    // A page title first, as on the kits' pages: the cards then get a row of their own.
    add_one(
        &solution,
        name,
        "label",
        Placement::In("center".into()),
        &[("LabelType", "header")],
    );
    let mut cards: Vec<String> = Vec::new();
    for index in 0..2 {
        let card = if index == 0 {
            add_one(
                &solution,
                name,
                "container",
                Placement::In("center".into()),
                &[],
            )
        } else {
            add_one(
                &solution,
                name,
                "container",
                beside(&cards[0], Side::Right),
                &[],
            )
        };
        let mut previous = add_one(
            &solution,
            name,
            "label",
            Placement::In(card.clone()),
            &[("LabelType", "sub-header")],
        );
        for row in 0..3 {
            if row > 0 {
                previous = add_one(
                    &solution,
                    name,
                    "separator",
                    beside(&previous, Side::Below),
                    &[],
                );
            }
            let dot = add_one(
                &solution,
                name,
                "shape",
                beside(&previous, Side::Below),
                &[("Margin", "0 5"), ("Height", "12"), ("Width", "12")],
            );
            let label = add_one(
                &solution,
                name,
                "label",
                beside(&dot, Side::Right),
                &[("Margin", "0 5")],
            );
            add_one(
                &solution,
                name,
                "label",
                beside(&label, Side::End),
                &[("Margin", "0 18 0 5")],
            );
            previous = dot;
        }
        cards.push(card);
    }
    let content = content_of(&solution, name);
    let row = parent_id(&content["UI"], &cards[0]).unwrap();
    let row = find_widget(&content["UI"], &row).unwrap();
    let properties = &row["Properties"];
    assert_eq!(properties["flex-size"], "range", "{properties}");
    assert_eq!(properties["flex-max-height"], "");
    // The title row as fixed, three label rows of 19px, two 1px separators, and a row margin.
    let title = &find_widget(&content["UI"], &cards[0]).unwrap()["Widgets"][0]["Properties"];
    let title_px: f64 = title["flex-min-height"]
        .as_str()
        .unwrap()
        .trim_end_matches("px")
        .parse()
        .unwrap();
    assert_eq!(
        properties["flex-min-height"],
        format!("{}px", (title_px + 3.0 * 19.0 + 2.0 + 12.0).ceil())
    );
    // The separator carries the kits' colour as a root style entry.
    let styles = &content["UI"]["Properties"]["StyleProperties"]["data"];
    let separators: Vec<_> = styles
        .as_object()
        .unwrap()
        .values()
        .filter(|entry| entry["parts"]["$"]["$"]["background"] == "#C2C7CE")
        .collect();
    assert_eq!(separators.len(), 4, "{styles}");
}

#[test]
fn a_separator_goes_only_between_rows() {
    let (_dir, solution) = one_project();
    let name = "Acme.Kpi_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let mut request = add_request(name, "separator");
    request.placement = Some(Placement::In("center".into()));
    assert!(matches!(
        plan_add(&solution, &request),
        Err(AuthoringError::Invalid(_))
    ));
}

#[test]
fn placement_measures_the_widget_with_the_props_it_is_given() {
    use crate::core::mashup::edit::Side;
    let (_dir, solution) = one_project();
    let name = "Acme.Kpi_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let first = add_one(
        &solution,
        name,
        "label",
        Placement::In("center".into()),
        &[],
    );
    let tall = add_one(
        &solution,
        name,
        "label",
        Placement::Beside {
            anchor: first,
            side: Side::Below,
        },
        &[("Margin", "20")],
    );
    // A row below it, so the measured row is not the column's last, growing one.
    add_one(
        &solution,
        name,
        "label",
        Placement::Beside {
            anchor: tall.clone(),
            side: Side::Below,
        },
        &[],
    );
    let content = content_of(&solution, name);
    let row = parent_id(&content["UI"], &tall).unwrap();
    let row = &find_widget(&content["UI"], &row).unwrap()["Properties"];
    // A 36px label with 40px of vertical margin and the row's 12px: measured as given.
    assert_eq!(row["flex-min-height"], "88px", "{row}");
}

#[test]
fn a_separator_refuses_a_card_anchor_and_a_malformed_style_map() {
    use crate::core::mashup::edit::Side;
    let (_dir, solution) = one_project();
    let name = "Acme.Kpi_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let card = add_one(
        &solution,
        name,
        "container",
        Placement::In("center".into()),
        &[],
    );
    let title = add_one(&solution, name, "label", Placement::In(card.clone()), &[]);
    let below = |anchor: &str| {
        let mut request = add_request(name, "separator");
        request.placement = Some(Placement::Beside {
            anchor: anchor.to_string(),
            side: Side::Below,
        });
        plan_add(&solution, &request)
    };
    assert!(matches!(below(&card), Err(AuthoringError::Invalid(_))));
    // A root style map that is not Composer's shape is refused, not a panic.
    let mut set = SetRequest {
        mashup: name.into(),
        widget: "mashup-root".into(),
        properties: vec![("StyleProperties".into(), json!({"data": []}))],
        style_from: None,
        styles: Vec::new(),
    };
    let plan = plan_set(&solution, &set).unwrap();
    apply_edit(&solution, plan.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    assert!(matches!(below(&title), Err(AuthoringError::Refused(_))));
    set.properties.clear();
}

#[test]
fn end_refuses_a_filling_widget_before_changing_anything() {
    use crate::core::mashup::edit::Side;
    let (dir, solution) = one_project();
    let name = "Acme.Kpi_MU";
    apply_new(
        &solution,
        &plan_new(&solution, &request(name)).unwrap(),
        &lock(&solution),
    )
    .unwrap();
    let grid = add_one(&solution, name, "grid", Placement::In("center".into()), &[]);
    let before = tree_bytes(dir.path());
    let mut request = add_request(name, "label");
    request.placement = Some(Placement::Beside {
        anchor: grid,
        side: Side::End,
    });
    assert!(matches!(
        plan_add(&solution, &request),
        Err(AuthoringError::Edit(_))
    ));
    assert_eq!(tree_bytes(dir.path()), before);
}

fn cell_menu_definition(dir: &Path, values: &[&str]) {
    let states: Vec<Value> = values
        .iter()
        .map(|value| json!({"defaultValue": value, "displayString": value, "name": ""}))
        .collect();
    let definition = json!({"stateType": "string", "stateDefinitions": states});
    write(
        dir,
        "acme/StateDefinitions/Acme.Menu_ST.xml",
        &format!(
            r#"<Entities><StateDefinitions><StateDefinition name="Acme.Menu_ST" projectName="Acme"><content><![CDATA[{definition}]]></content></StateDefinition></StateDefinitions></Entities>"#
        ),
    );
}

#[test]
fn a_cell_menu_item_event_is_checked_against_the_menu_state_definition() {
    let (dir, solution) = collection_fixture();
    let plan = plan_collection_add(&solution, &collection_request(&[("id", "Id")])).unwrap();
    apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
    let collection = plan.id.clone();
    cell_menu_definition(dir.path(), &["Edit", "More Details"]);
    let event = |item: &str| format!("ui:{collection}.CellMenu:{item}");
    let invoke = "data:Acme.Orders.Manager.GetOrders";
    // No cell menu yet.
    assert!(matches!(
        on(&solution, &event("Edit"), invoke),
        Err(AuthoringError::Invalid(_))
    ));
    let set = plan_set(
        &solution,
        &SetRequest {
            mashup: "Acme.Orders_MU".into(),
            widget: collection.clone(),
            properties: vec![("CellMenu".into(), json!("Acme.Menu_ST"))],
            style_from: None,
            styles: Vec::new(),
        },
    )
    .unwrap();
    apply_edit(&solution, set.edit.as_ref().unwrap(), &lock(&solution)).unwrap();
    // Composer's form only: spaces become hyphens.
    let error = on(&solution, &event("More Details"), invoke)
        .unwrap_err()
        .to_string();
    assert!(error.contains("CellMenu:More-Details"), "{error}");
    // An item the StateDefinition lacks.
    let error = on(&solution, &event("Archive"), invoke)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("CellMenu:Edit, CellMenu:More-Details"),
        "{error}"
    );
    let wired = on(&solution, &event("More-Details"), invoke).unwrap();
    apply_edit(&solution, &wired.edit, &lock(&solution)).unwrap();
    let report =
        crate::core::mashup::validate::validate(&solution, Some("Acme.Orders_MU")).unwrap();
    assert!(
        !report[0].problems.iter().any(|p| p.rule == "cell-menu"),
        "{:?}",
        report[0].problems
    );
    // The state goes from the menu: validate names the event that now fires nothing.
    cell_menu_definition(dir.path(), &["Edit"]);
    let report =
        crate::core::mashup::validate::validate(&solution, Some("Acme.Orders_MU")).unwrap();
    assert!(
        report[0]
            .problems
            .iter()
            .any(|p| p.rule == "cell-menu" && p.message.contains("CellMenu:More-Details")),
        "{:?}",
        report[0].problems
    );
}
