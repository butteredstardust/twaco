use super::*;

fn contract(key: &str, members: &[(&str, &str, Value)]) -> Contract {
    let mut entity = Entity {
        project: "Acme.App".into(),
        ..Entity::default()
    };
    for (kind, name, value) in members {
        entity.add(kind, name, value.clone());
    }
    // Signatures belong to an existing service.
    if members
        .iter()
        .any(|(kind, _, _)| matches!(*kind, "param" | "result"))
    {
        entity.add("service", "Run", json!({}));
    }
    Contract {
        entities: BTreeMap::from([(key.into(), entity)]),
        ..Contract::default()
    }
}

fn param(required: bool, default: Option<&str>) -> Value {
    json!({"baseType": "STRING", "dataShape": null, "required": required, "default": default})
}

#[test]
fn every_member_removal_and_addition_is_classified() {
    for kind in [
        "service",
        "property",
        "event",
        "field",
        "mashup_parameter",
        "param",
    ] {
        let name = if kind == "param" {
            "Run.input"
        } else {
            "input"
        };
        let value = if kind == "param" {
            param(false, None)
        } else {
            json!({"baseType":"STRING"})
        };
        let report = compare(
            contract("Things/Acme.One", &[(kind, name, value.clone())]),
            contract("Things/Acme.One", &[]),
            &Ledger::default(),
        );
        // Keep the service present so removing its parameter is reported separately.
        let report = if kind == "param" {
            compare(
                contract("Things/Acme.One", &[(kind, name, value.clone())]),
                contract("Things/Acme.One", &[("service", "Run", json!({}))]),
                &Ledger::default(),
            )
        } else {
            report
        };
        assert_eq!(report.breaking.len(), 1, "{kind}");
        assert_eq!(report.breaking[0].kind, kind);
        let before = if kind == "param" {
            contract("Things/Acme.One", &[("service", "Run", json!({}))])
        } else {
            contract("Things/Acme.One", &[])
        };
        let report = compare(
            before,
            contract("Things/Acme.One", &[(kind, name, value)]),
            &Ledger::default(),
        );
        assert!(report.breaking.is_empty(), "{kind}");
        assert_eq!(report.compatible.len(), 1, "{kind}");
    }
}

#[test]
fn signature_changes_cover_types_shapes_required_and_defaults() {
    let cases = [
        (param(false, None), param(true, None), true),
        (param(true, None), param(false, None), false),
        (param(false, None), param(false, Some("a")), false),
        (param(false, Some("a")), param(false, Some("b")), false),
        (param(true, None), param(true, Some("a")), false),
        (
            param(false, None),
            json!({"baseType":"NUMBER", "dataShape":null,"required":false,"default":null}),
            true,
        ),
        (
            param(false, None),
            json!({"baseType":"STRING", "dataShape":"Acme.Rows","required":false,"default":null}),
            true,
        ),
    ];
    for (before, after, breaking) in cases {
        let report = compare(
            contract("Things/Acme.One", &[("param", "Run.input", before)]),
            contract("Things/Acme.One", &[("param", "Run.input", after)]),
            &Ledger::default(),
        );
        assert_eq!(report.breaking.len(), usize::from(breaking));
        assert_eq!(report.compatible.len(), usize::from(!breaking));
    }
    for (required, default, breaking) in [
        (true, None, true),
        (false, None, false),
        (true, Some("x"), false),
    ] {
        let report = compare(
            contract("Things/Acme.One", &[("service", "Run", json!({}))]),
            contract(
                "Things/Acme.One",
                &[("param", "Run.input", param(required, default))],
            ),
            &Ledger::default(),
        );
        assert_eq!(report.breaking.len(), usize::from(breaking));
        assert_eq!(report.compatible.len(), usize::from(!breaking));
    }
    for kind in ["result", "property", "field", "mashup_parameter"] {
        for attribute in ["baseType", "dataShape"] {
            if kind == "mashup_parameter" && attribute == "dataShape" {
                continue;
            }
            let name = if kind == "result" { "Run" } else { "value" };
            let before = json!({"baseType":"STRING", "dataShape":null});
            let mut after = before.clone();
            after[attribute] = json!("Acme.Changed");
            let report = compare(
                contract("Things/Acme.One", &[(kind, name, before)]),
                contract("Things/Acme.One", &[(kind, name, after)]),
                &Ledger::default(),
            );
            assert_eq!(report.breaking.len(), 1, "{kind} {attribute}");
        }
    }
    for (kind, before, after) in [
        (
            "event",
            json!({"dataShape":"Acme.Before"}),
            json!({"dataShape":"Acme.After"}),
        ),
        (
            "field",
            json!({"isPrimaryKey":false}),
            json!({"isPrimaryKey":true}),
        ),
        ("base_data_shape", json!(null), json!("Acme.Base")),
    ] {
        let report = compare(
            contract("DataShapes/Acme.Rows", &[(kind, "value", before)]),
            contract("DataShapes/Acme.Rows", &[(kind, "value", after)]),
            &Ledger::default(),
        );
        assert_eq!(report.breaking.len(), 1, "{kind}");
    }
}

#[test]
fn entity_changes_and_project_moves_use_identity() {
    for collection in [
        "Things",
        "ThingTemplates",
        "ThingShapes",
        "DataShapes",
        "Mashups",
    ] {
        let key = format!("{collection}/Acme.One");
        let report = compare(contract(&key, &[]), Contract::default(), &Ledger::default());
        assert_eq!(report.breaking.len(), 1);
        let report = compare(Contract::default(), contract(&key, &[]), &Ledger::default());
        assert_eq!(report.compatible.len(), 1);
        let mut after = contract(&key, &[]);
        after.entities.get_mut(&key).unwrap().project = "Acme.Other".into();
        let report = compare(contract(&key, &[]), after, &Ledger::default());
        assert!(report.breaking.is_empty());
        assert_eq!(report.compatible[0].change, "moved to project Acme.Other");
    }
}

fn ledger_record(kind: &str, collection: &str, entity: &str, old: &str, new: &str) -> Ledger {
    serde_json::from_value(json!([{"date":"2026-10-20", "kind":kind,"old":old,"new":new,"scope":entity,"service":"Run",
        "entities":[{"collection":collection,"old":if kind == "entity" { old } else { entity },"new":if kind == "entity" { new } else { entity }}]}])).unwrap()
}

#[test]
fn ledger_pairs_entity_service_field_property_and_param_renames() {
    for kind in ["entity", "service", "field", "property", "param"] {
        let old_name = if kind == "param" { "Run.old" } else { "old" };
        let new_name = if kind == "param" { "Run.new" } else { "new" };
        let collection = if kind == "field" {
            "DataShapes"
        } else {
            "Things"
        };
        let key = format!("{collection}/Acme.One");
        let pair = || {
            if kind == "entity" {
                (
                    contract("Things/Acme.Old", &[]),
                    contract("Things/Acme.New", &[]),
                )
            } else {
                (
                    contract(&key, &[(kind, old_name, param(false, None))]),
                    contract(&key, &[(kind, new_name, param(false, None))]),
                )
            }
        };
        let (before, after) = pair();
        let plain = compare(before, after, &Ledger::default());
        assert_eq!(plain.breaking.len(), 1, "{kind}");
        assert_eq!(plain.compatible.len(), 1, "{kind}");
        let ledger = ledger_record(
            kind,
            collection,
            "Acme.One",
            if kind == "entity" { "Acme.Old" } else { "old" },
            if kind == "entity" { "Acme.New" } else { "new" },
        );
        let (before, after) = pair();
        let report = compare(before, after, &ledger);
        assert_eq!(report.breaking.len(), 1, "{kind}");
        assert!(report.breaking[0].change.starts_with("renamed "), "{kind}");
        assert!(report.compatible.is_empty(), "{kind}");
    }
}

fn fixture() -> (tempfile::TempDir, Solution) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("twaco.toml"),
        "[[project]]\nname='Acme.App'\n",
    )
    .unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution)
}

fn write_entity(
    solution: &Solution,
    collection: &str,
    tag: &str,
    name: &str,
    attrs: &str,
    body: &str,
) {
    std::fs::create_dir_all(solution.root.join(collection)).unwrap();
    std::fs::write(solution.root.join(collection).join(format!("{name}.xml")), format!("<Entities><{collection}><{tag} name=\"{name}\" projectName=\"Acme.App\" {attrs}>{body}</{tag}></{collection}></Entities>")).unwrap();
}

#[test]
fn loader_compares_contract_only_and_reports_unreadable_inputs() {
    let (_a, before) = fixture();
    let (_b, after) = fixture();
    for solution in [&before, &after] {
        write_entity(solution, "DataShapes", "DataShape", "Acme.Rows", "", "<FieldDefinitions><FieldDefinition name=\"id\" baseType=\"STRING\" aspect.isPrimaryKey=\"true\"/></FieldDefinitions>");
        write_entity(solution, "Things", "Thing", "Acme.One", "thingTemplate=\"GenericThing\"", "<ThingShape><EventDefinitions><EventDefinition name=\"Changed\" dataShape=\"Acme.Rows\"/></EventDefinitions><PropertyDefinitions><PropertyDefinition name=\"value\" baseType=\"STRING\" description=\"before\"/></PropertyDefinitions></ThingShape>");
        write_entity(solution, "Mashups", "Mashup", "Acme.Card", "", "<ParameterDefinitions><FieldDefinition name=\"id\" baseType=\"STRING\"/></ParameterDefinitions>");
    }
    let path = after.root.join("Things/Acme.One.xml");
    let xml = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, xml.replace("before", "after")).unwrap();
    // File layout is never identity.
    std::fs::create_dir(after.root.join("Things/nested")).unwrap();
    std::fs::rename(&path, after.root.join("Things/nested/Acme.One.xml")).unwrap();
    let report = compare(load(&before, None), load(&after, None), &Ledger::default());
    assert!(report.breaking.is_empty());
    assert!(report.compatible.is_empty());
    let path = after.root.join("Things/nested/Acme.One.xml");
    let xml = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        xml.replace("dataShape=\"Acme.Rows\"", "dataShape=\"Acme.OtherRows\""),
    )
    .unwrap();
    let events = compare(load(&before, None), load(&after, None), &Ledger::default());
    assert_eq!(events.breaking.len(), 1);
    assert_eq!(events.breaking[0].kind, "event");
    std::fs::write(&path, xml).unwrap();
    let path = after.root.join("Mashups/Acme.Card.xml");
    let xml = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, xml.replace("STRING", "NUMBER")).unwrap();
    let report = compare(load(&before, None), load(&after, None), &Ledger::default());
    assert_eq!(report.breaking[0].kind, "mashup_parameter");
    std::fs::write(
        after.root.join("Things/Broken.xml"),
        "<Entities><Things><Thing name=\"Acme.Broken\"",
    )
    .unwrap();
    let mut report = compare(load(&before, None), load(&after, None), &Ledger::default());
    report.summary = summary(&report, "HEAD");
    assert!(!report.skipped.is_empty());
    assert!(report.summary.contains("partial"));
}

#[test]
fn loader_preserves_primary_keys_and_base_datashapes_but_ignores_field_metadata() {
    let (_a, before) = fixture();
    let (_b, after) = fixture();
    let field = "<FieldDefinitions><FieldDefinition name=\"id\" baseType=\"STRING\" aspect.isPrimaryKey=\"true\" ordinal=\"0\" description=\"before\"/></FieldDefinitions>";
    write_entity(
        &before,
        "DataShapes",
        "DataShape",
        "Acme.Rows",
        "baseDataShape=\"Acme.Base\"",
        field,
    );
    write_entity(
        &after,
        "DataShapes",
        "DataShape",
        "Acme.Rows",
        "baseDataShape=\"Acme.Base\"",
        &field
            .replace("ordinal=\"0\"", "ordinal=\"9\"")
            .replace("before", "after"),
    );
    let report = compare(load(&before, None), load(&after, None), &Ledger::default());
    assert!(report.breaking.is_empty());
    assert!(report.compatible.is_empty());
    write_entity(
        &after,
        "DataShapes",
        "DataShape",
        "Acme.Rows",
        "baseDataShape=\"Acme.Base\"",
        &field.replace("isPrimaryKey=\"true\"", "isPrimaryKey=\"false\""),
    );
    let report = compare(load(&before, None), load(&after, None), &Ledger::default());
    assert_eq!(report.breaking.len(), 1);
    assert_eq!(report.breaking[0].kind, "field");
    assert_eq!(
        report.breaking[0].from.as_ref().unwrap()["isPrimaryKey"],
        true
    );
    assert_eq!(
        report.breaking[0].to.as_ref().unwrap()["isPrimaryKey"],
        false
    );
    write_entity(
        &after,
        "DataShapes",
        "DataShape",
        "Acme.Rows",
        "baseDataShape=\"Acme.OtherBase\"",
        field,
    );
    let report = compare(load(&before, None), load(&after, None), &Ledger::default());
    assert_eq!(report.breaking.len(), 1);
    assert_eq!(report.breaking[0].kind, "base_data_shape");
}

#[test]
fn inherited_callers_include_transitive_templates_shapes_and_exclude_mentions() {
    let (_dir, solution) = fixture();
    write_entity(&solution, "ThingShapes", "ThingShape", "Acme.Shape", "", "");
    write_entity(&solution, "ThingTemplates", "ThingTemplate", "Acme.Base", "baseThingTemplate=\"GenericThing\"", "<ImplementedShapes><ImplementedShape name=\"Acme.Shape\"/></ImplementedShapes><ThingShape/>");
    write_entity(
        &solution,
        "ThingTemplates",
        "ThingTemplate",
        "Acme.Child",
        "baseThingTemplate=\"Acme.Base\"",
        "<ThingShape/>",
    );
    write_entity(
        &solution,
        "Things",
        "Thing",
        "Acme.One",
        "thingTemplate=\"Acme.Child\"",
        "<ThingShape/>",
    );
    write_entity(
        &solution,
        "Things",
        "Thing",
        "Acme.Caller",
        "thingTemplate=\"GenericThing\"",
        r#"<ThingShape><ServiceDefinitions><ServiceDefinition name="Call"/></ServiceDefinitions><ServiceImplementations><ServiceImplementation name="Call"><HandlerDefinition name="Script"/><ConfigurationTables><ConfigurationTable><DataShape/><Rows><Row><code><![CDATA[Things["Acme.One"].Run(); var mention = "Acme.Base";]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape>"#,
    );
    let index = Index::build(&solution);
    for entity in [
        "ThingTemplates/Acme.Base",
        "ThingTemplates/Acme.Child",
        "ThingShapes/Acme.Shape",
    ] {
        let mut report = compare(
            contract(entity, &[("service", "Run", json!({}))]),
            contract(entity, &[]),
            &Ledger::default(),
        );
        attach_callers(&mut report, &index, &solution);
        assert_eq!(
            report.breaking[0].callers,
            ["Things/Acme.Caller.Call"],
            "{entity}; {:?}",
            index.edges()
        );
    }
    let mut report = compare(
        contract("ThingTemplates/Acme.Base", &[]),
        Contract::default(),
        &Ledger::default(),
    );
    attach_callers(&mut report, &index, &solution);
    assert!(!report.breaking[0]
        .callers
        .iter()
        .any(|c| c.contains("Acme.Caller")));
    assert!(report.breaking[0]
        .callers
        .iter()
        .any(|c| c.contains("Acme.Child")));

    // A child override no longer inherits the removed declaration.
    write_entity(&solution, "ThingTemplates", "ThingTemplate", "Acme.Child", "baseThingTemplate=\"Acme.Base\"", "<ThingShape><ServiceDefinitions><ServiceDefinition name=\"Run\"/></ServiceDefinitions></ThingShape>");
    let mut report = compare(
        contract("ThingTemplates/Acme.Base", &[("service", "Run", json!({}))]),
        contract("ThingTemplates/Acme.Base", &[]),
        &Ledger::default(),
    );
    attach_callers(&mut report, &Index::build(&solution), &solution);
    assert!(report.breaking[0].callers.is_empty());
}

#[test]
fn removed_entities_still_resolve_current_callers_without_historical_sources() {
    let (_dir, solution) = fixture();
    write_entity(
        &solution,
        "Things",
        "Thing",
        "Acme.Caller",
        "thingTemplate=\"GenericThing\"",
        r#"<ThingShape><ServiceDefinitions><ServiceDefinition name="Call"/></ServiceDefinitions><ServiceImplementations><ServiceImplementation name="Call"><HandlerDefinition name="Script"/><ConfigurationTables><ConfigurationTable><DataShape/><Rows><Row><code><![CDATA[Things["Acme.Removed"].Run(); var mention = "Acme.Removed";]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations><PropertyDefinitions><PropertyDefinition name="rows" baseType="INFOTABLE" aspect.dataShape="Acme.RemovedRows"/></PropertyDefinitions></ThingShape>"#,
    );
    let removed = [
        EntityKey::new("Things", "Acme.Removed").unwrap(),
        EntityKey::new("DataShapes", "Acme.RemovedRows").unwrap(),
    ];
    let plain = Index::build(&solution);
    assert!(plain
        .edges()
        .iter()
        .all(|(_, to, _)| !to.contains("Acme.Removed")));
    let index = Index::with_targets(&solution, &removed);
    assert!(index
        .edges()
        .iter()
        .all(|(from, _, _)| !from.contains("Acme.Removed")));
    for (key, caller) in [
        ("Things/Acme.Removed", "Things/Acme.Caller.Call"),
        ("DataShapes/Acme.RemovedRows", "Things/Acme.Caller.rows"),
    ] {
        let mut report = compare(contract(key, &[]), Contract::default(), &Ledger::default());
        attach_callers(&mut report, &index, &solution);
        assert_eq!(report.breaking[0].callers, [caller]);
    }
}

#[test]
fn project_filters_apply_to_each_snapshot_and_project_moves_remain_single_changes() {
    let (_a, before) = fixture();
    let (_b, after) = fixture();
    for solution in [&before, &after] {
        std::fs::write(solution.root.join("twaco.toml"), "[[project]]\nname='Acme.App'\nroot='App'\n[[project]]\nname='Acme.Other'\nroot='Other'\n").unwrap();
    }
    let before = Solution::load(&before.root.join("twaco.toml")).unwrap();
    let after = Solution::load(&after.root.join("twaco.toml")).unwrap();
    for (solution, folder, project) in [
        (&before, "App", "Acme.App"),
        (&after, "Other", "Acme.Other"),
    ] {
        std::fs::create_dir_all(solution.root.join(folder).join("DataShapes")).unwrap();
        std::fs::write(solution.root.join(folder).join("DataShapes/Acme.Rows.xml"), format!("<Entities><DataShapes><DataShape name=\"Acme.Rows\" projectName=\"{project}\"><FieldDefinitions/></DataShape></DataShapes></Entities>")).unwrap();
    }
    let full = compare(load(&before, None), load(&after, None), &Ledger::default());
    assert!(full.breaking.is_empty());
    assert_eq!(full.compatible.len(), 1);
    assert_eq!(full.compatible[0].change, "moved to project Acme.Other");
    let app = compare(
        load(&before, Some("Acme.App")),
        load(&after, Some("Acme.App")),
        &Ledger::default(),
    );
    assert_eq!(app.breaking.len(), 1);
    assert!(app.compatible.is_empty());
    let other = compare(
        load(&before, Some("Acme.Other")),
        load(&after, Some("Acme.Other")),
        &Ledger::default(),
    );
    assert!(other.breaking.is_empty());
    assert_eq!(other.compatible.len(), 1);
}

#[test]
fn service_rename_preserves_comparison_of_its_signature() {
    let before = contract(
        "Things/Acme.One",
        &[
            ("service", "Old", json!({})),
            ("result", "Old", json!({"baseType":"STRING"})),
            ("param", "Old.input", param(false, None)),
        ],
    );
    let after = contract(
        "Things/Acme.One",
        &[
            ("service", "New", json!({})),
            ("result", "New", json!({"baseType":"STRING"})),
            ("param", "New.input", param(false, None)),
        ],
    );
    let report = compare(
        before,
        after,
        &ledger_record("service", "Things", "Acme.One", "Old", "New"),
    );
    assert_eq!(report.breaking.len(), 1);
    assert!(report.compatible.is_empty());
    assert_eq!(report.breaking[0].change, "renamed Old -> New");
}

#[test]
fn datashape_users_and_mashup_containers_and_navigation_are_callers() {
    let (_dir, solution) = fixture();
    write_entity(
        &solution,
        "DataShapes",
        "DataShape",
        "Acme.Rows",
        "",
        "<FieldDefinitions/>",
    );
    write_entity(&solution, "Things", "Thing", "Acme.One", "thingTemplate=\"GenericThing\"", "<ThingShape><PropertyDefinitions><PropertyDefinition name=\"rows\" baseType=\"INFOTABLE\" aspect.dataShape=\"Acme.Rows\"/></PropertyDefinitions></ThingShape>");
    let content = json!({"UI":{"Properties":{"Id":"root"},"Widgets":[
        {"Properties":{"Id":"container-1","Type":"mashupcontainer","Name":"Acme.Card"}},
        {"Properties":{"Id":"navigation-2","Type":"navigationfunction","TargetMashup":"Acme.Card"}}
    ]}});
    write_entity(
        &solution,
        "Mashups",
        "Mashup",
        "Acme.Host",
        "",
        &format!("<mashupContent><![CDATA[{content}]]></mashupContent>"),
    );
    let index = Index::build(&solution);
    let mut report = compare(
        contract("DataShapes/Acme.Rows", &[("field", "id", json!({}))]),
        contract("DataShapes/Acme.Rows", &[]),
        &Ledger::default(),
    );
    attach_callers(&mut report, &index, &solution);
    assert_eq!(report.breaking[0].callers, ["Things/Acme.One.rows"]);
    let mut report = compare(
        contract(
            "Mashups/Acme.Card",
            &[("mashup_parameter", "id", json!({}))],
        ),
        contract("Mashups/Acme.Card", &[]),
        &Ledger::default(),
    );
    attach_callers(&mut report, &index, &solution);
    assert_eq!(
        report.breaking[0].callers,
        [
            "Mashups/Acme.Host.container-1",
            "Mashups/Acme.Host.navigation-2"
        ]
    );
}

#[test]
fn summary_detail_and_json_keep_counts_and_caller_limits() {
    let mut report = compare(
        contract("Things/Acme.One", &[("service", "Run", json!({}))]),
        contract("Things/Acme.One", &[("property", "id", json!({}))]),
        &Ledger::default(),
    );
    report.breaking[0].callers = (0..5).map(|i| format!("Things/Acme.Caller{i}")).collect();
    report.summary = summary(&report, "origin/main");
    assert_eq!(
        report.summary,
        "contract against origin/main: 1 breaking, 1 compatible; 5 callers in this repository"
    );
    let text = render(&report, false);
    assert!(text.contains("+2 more"));
    assert!(!text.contains("Caller4"));
    let detail = render(&report, true);
    assert!(detail.contains("Caller4"));
    assert!(detail.contains("compatible: Things/Acme.One.id"));
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["breaking"][0]["callers"].as_array().unwrap().len(), 5);
    let mut empty = compare(Contract::default(), Contract::default(), &Ledger::default());
    empty.summary = summary(&empty, "HEAD");
    assert_eq!(empty.summary, "contract against HEAD: no change");
}
