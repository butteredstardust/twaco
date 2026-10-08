use super::*;

fn table_fixture(tag: &str) -> Fixture {
    let root_guard = tempfile::Builder::new()
        .prefix(&format!("twaco-rename-table-{tag}-"))
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "twaco.toml", "[[project]]\nname = \"P\"\ncollections = [\"ThingShapes\", \"ThingTemplates\", \"Things\"]\n");
    let definition = "<ConfigurationTableDefinitions><ConfigurationTableDefinition dataShapeName=\"P.Limits_CT\" name=\"Limits_CT\"/></ConfigurationTableDefinitions>";
    let table = |value: &str| {
        format!("<ConfigurationTables><ConfigurationTable dataShapeName=\"P.Limits_CT\" name=\"Limits_CT\"><DataShape><FieldDefinitions><FieldDefinition name=\"Value\"/></FieldDefinitions></DataShape><Rows><Row><Value>{value}</Value></Row></Rows></ConfigurationTable></ConfigurationTables>")
    };
    let script = |body: &str| {
        format!("<ThingShape><ServiceImplementations><ServiceImplementation name=\"Use\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{body}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape>")
    };
    write(&root, "ThingShapes/P.Limits.xml", &format!("<Entities><ThingShapes><ThingShape name=\"P.Limits\" projectName=\"P\">{definition}{}</ThingShape></ThingShapes></Entities>\n", script("const x = { tableName: \"Limits_CT\" };")));
    write(&root, "ThingTemplates/P.Base.xml", &format!("<Entities><ThingTemplates><ThingTemplate name=\"P.Base\" projectName=\"P\" baseThingTemplate=\"GenericThing\"><ImplementedShapes><ImplementedShape name=\"P.Limits\"/></ImplementedShapes>{}{}</ThingTemplate></ThingTemplates></Entities>\n", table("template"), script("let x = {'tableName' : 'Limits_CT'};")));
    write(&root, "Things/P.One.xml", &format!("\u{feff}<Entities>\r\n<Things><Thing name=\"P.One\" projectName=\"P\" thingTemplate=\"P.Base\">{}{}</Thing></Things></Entities>\r\n", table("one"), script("let x = {\"tableName\": \"Limits_CT\"}; const TABLE = \"Limits_CT\";")));
    write(&root, "Things/P.Two.xml", &format!("<Entities><Things><Thing name=\"P.Two\" projectName=\"P\" thingTemplate=\"P.Base\">{}</Thing></Things></Entities>\n", table("two")));
    write(&root, "Things/P.Other.xml", &format!("<Entities><Things><Thing name=\"P.Other\" projectName=\"P\" thingTemplate=\"GenericThing\">{definition}{}{}</Thing></Things></Entities>\n", table("unrelated"), script("const TABLE = \"Limits_CT\"; x.Limits_CT;")));
    write(&root, "Things/P.Empty.xml", "<Entities><Things><Thing name=\"P.Empty\" projectName=\"P\" thingTemplate=\"GenericThing\"><ConfigurationTableDefinitions/><ConfigurationTables/></Thing></Things></Entities>\n");
    write(&root, "src/P.One/services/Use/script.js", "const a = { tableName: \"Limits_CT\" }; const TABLE = \"Limits_CT\"; x.Limits_CT; x[\"Limits_CT\"];\n");
    write(
        &root,
        "src/P.Other/services/Use/script.js",
        "const a = { tableName: \"Limits_CT\" }; const TABLE = 'Limits_CT'; x.Limits_CT;\n",
    );
    let solution = Solution::load(&root.join(CONFIG_FILE)).unwrap();
    Fixture {
        _dir: root_guard,
        root,
        solution,
    }
}

fn table_spec(scope: &str, old: &str, new: &str) -> Spec {
    Spec {
        kind: Kind::Table,
        old: old.to_string(),
        new: new.to_string(),
        scope: Some(scope.to_string()),
        service: None,
    }
}

#[test]
fn a_shape_the_scope_implements_has_its_table_calls_rewritten() {
    // The services that read a table often sit on a shape the declaring template implements:
    // an ancestor of the scope, not a descendant. Its `tableName: "..."` calls follow the rename.
    let root_guard = tempfile::Builder::new()
        .prefix("twaco-rename-table-family-")
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(
        &root,
        "twaco.toml",
        "[[project]]\nname = \"P\"\ncollections = [\"ThingShapes\", \"ThingTemplates\"]\n",
    );
    write(&root, "ThingShapes/P.Svc.xml", "<Entities><ThingShapes><ThingShape name=\"P.Svc\" projectName=\"P\"><ServiceImplementations><ServiceImplementation name=\"Read\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[const a = me.GetConfigurationTable({ tableName: \"Limits_CT\" });]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></ThingShapes></Entities>\n");
    write(
        &root,
        "src/P.Svc/services/Read/script.js",
        "const a = me.GetConfigurationTable({ tableName: \"Limits_CT\" });",
    );
    write(&root, "ThingTemplates/P.Mgr.xml", "<Entities><ThingTemplates><ThingTemplate name=\"P.Mgr\" projectName=\"P\" baseThingTemplate=\"GenericThing\"><ImplementedShapes><ImplementedShape name=\"P.Svc\"/></ImplementedShapes><ConfigurationTableDefinitions><ConfigurationTableDefinition dataShapeName=\"P.Limits_CT\" name=\"Limits_CT\"/></ConfigurationTableDefinitions></ThingTemplate></ThingTemplates></Entities>\n");
    let solution = Solution::load(&root.join(CONFIG_FILE)).unwrap();
    let planned = plan(&solution, &table_spec("P.Mgr", "Limits_CT", "Bounds_CT")).unwrap();
    apply(
        &solution,
        &planned,
        &options(false),
        &locked_root(&solution.root),
    )
    .unwrap();
    let shape = std::fs::read_to_string(root.join("ThingShapes/P.Svc.xml")).unwrap();
    assert!(
        shape.contains("tableName: \"Bounds_CT\"") && !shape.contains("Limits_CT"),
        "{shape}"
    );
    let sidecar = std::fs::read_to_string(root.join("src/P.Svc/services/Read/script.js")).unwrap();
    assert!(sidecar.contains("tableName: \"Bounds_CT\""), "{sidecar}");
}

#[test]
fn table_rename_follows_shape_template_thing_ancestry_and_round_trips_bytes() {
    let fixture = table_fixture("all-places");
    let before = snapshot(&fixture.root);
    let planned = plan(
        &fixture.solution,
        &table_spec("P.Limits", "Limits_CT", "Bounds_CT"),
    )
    .unwrap();
    assert_eq!(snapshot(&fixture.root), before, "planning writes nothing");
    assert!(planned.moves.is_empty() && planned.baseline_keys.is_empty());
    assert!(planned.outside.is_empty() && planned.named.is_empty());
    assert_eq!(
        planned.field_tables, 4,
        "one definition and three inherited instances"
    );
    let reviews = planned
        .changes
        .iter()
        .flat_map(|change| &change.findings)
        .filter(|finding| finding.tier == refs::Tier::Review)
        .count();
    assert!(reviews >= 7);
    apply(
        &fixture.solution,
        &planned,
        &options(false),
        &locked(&fixture),
    )
    .unwrap();

    for relative in [
        "ThingShapes/P.Limits.xml",
        "ThingTemplates/P.Base.xml",
        "Things/P.One.xml",
        "Things/P.Two.xml",
    ] {
        let text = String::from_utf8_lossy(&std::fs::read(fixture.root.join(relative)).unwrap())
            .into_owned();
        assert!(text.contains("Bounds_CT"), "{relative}");
        assert!(
            text.contains("P.Limits_CT"),
            "the DataShape must stay named: {relative}"
        );
    }
    let one =
        std::fs::read_to_string(fixture.root.join("src/P.One/services/Use/script.js")).unwrap();
    assert!(
        one.contains("tableName: \"Bounds_CT\"")
            && one.contains("TABLE = \"Limits_CT\"")
            && one.contains("x.Limits_CT")
    );
    let unrelated = std::fs::read_to_string(fixture.root.join("Things/P.Other.xml")).unwrap();
    assert!(unrelated.contains("name=\"Limits_CT\"") && !unrelated.contains("Bounds_CT"));
    let unrelated_script =
        std::fs::read_to_string(fixture.root.join("src/P.Other/services/Use/script.js")).unwrap();
    assert!(!unrelated_script.contains("Bounds_CT"));
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join(".twaco/renames.json")).unwrap())
            .unwrap();
    assert_eq!(ledger[0]["kind"], "table");
    assert_eq!(ledger[0]["scope"], "P.Limits");
    assert_eq!(ledger[0]["entities"].as_array().unwrap().len(), 4);
    assert_eq!(follow_up(&planned, false)[..2], [
            "The new table is a new table on a server: its rows come from the entity XML, so deploy with --overwrite-tables to load them; values edited on the server are not carried.",
            "The table's DataShape keeps its name; rename it with twaco rename entity if it is named for the table.",
        ]);

    let reverse = plan(
        &fixture.solution,
        &table_spec("P.Limits", "Bounds_CT", "Limits_CT"),
    )
    .unwrap();
    apply(
        &fixture.solution,
        &reverse,
        &options(false),
        &locked(&fixture),
    )
    .unwrap();
    for (relative, bytes) in before {
        if relative != Path::new(".twaco/renames.json") {
            assert_eq!(
                std::fs::read(fixture.root.join(&relative)).unwrap(),
                bytes,
                "{}",
                relative.display()
            );
        }
    }
}

#[test]
fn table_rename_refuses_unknown_wrong_owner_conflicts_invalid_and_same_names() {
    let fixture = table_fixture("refusals");
    assert!(matches!(
        plan(&fixture.solution, &table_spec("Missing", "Limits_CT", "X")),
        Err(RenameError::Unknown { .. })
    ));
    let missing = plan(&fixture.solution, &table_spec("P.Empty", "Limits_CT", "X"))
        .unwrap_err()
        .to_string();
    assert!(
        missing.contains("declares no configuration table"),
        "{missing}"
    );
    let inherited = plan(&fixture.solution, &table_spec("P.Base", "Limits_CT", "X"))
        .unwrap_err()
        .to_string();
    assert!(
        inherited.contains("declared on P.Limits") && inherited.contains("rename it there"),
        "{inherited}"
    );
    assert!(matches!(
        plan(
            &fixture.solution,
            &table_spec("P.Limits", "Limits_CT", "Limits_CT")
        ),
        Err(RenameError::Same { .. })
    ));
    for bad in ["", "1x", "a.b", "a b"] {
        assert!(
            matches!(
                plan(&fixture.solution, &table_spec("P.Limits", "Limits_CT", bad)),
                Err(RenameError::InvalidNew { .. })
            ),
            "{bad:?}"
        );
        assert!(
            matches!(
                plan(&fixture.solution, &table_spec("P.Limits", bad, "Valid")),
                Err(RenameError::InvalidOld { .. })
            ),
            "{bad:?}"
        );
    }
    let path = fixture.root.join("Things/P.Two.xml");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("name=\"Limits_CT\"", "name=\"Bounds_CT\"");
    std::fs::write(path, text).unwrap();
    assert!(matches!(
        plan(
            &fixture.solution,
            &table_spec("P.Limits", "Limits_CT", "Bounds_CT")
        ),
        Err(RenameError::Exists { .. })
    ));
}

#[test]
fn every_injected_table_apply_failure_rolls_back_the_complete_tree() {
    let count_fixture = table_fixture("rollback-count");
    let count_plan = plan(
        &count_fixture.solution,
        &table_spec("P.Limits", "Limits_CT", "Bounds_CT"),
    )
    .unwrap();
    let mut steps = Vec::new();
    apply_with(
        &count_fixture.solution,
        &count_plan,
        &options(false),
        &locked(&count_fixture),
        &mut |step| {
            steps.push(step.clone());
            Ok(())
        },
    )
    .unwrap();
    assert!(
        steps
            .iter()
            .filter(|step| matches!(step, Step::Write(_)))
            .count()
            >= 4
    );
    for (fail_at, failed_step) in steps.iter().enumerate() {
        let fixture = table_fixture("rollback");
        let planned = plan(
            &fixture.solution,
            &table_spec("P.Limits", "Limits_CT", "Bounds_CT"),
        )
        .unwrap();
        let before = snapshot(&fixture.root);
        let mut index = 0usize;
        let error = apply_with(
            &fixture.solution,
            &planned,
            &options(false),
            &locked(&fixture),
            &mut |_| {
                let current = index;
                index += 1;
                if current == fail_at {
                    Err(std::io::Error::other("injected failure"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(
            !matches!(error, RenameError::RollbackFailed { .. }),
            "{error}"
        );
        assert_eq!(
            snapshot(&fixture.root),
            before,
            "failed at step {fail_at}: {failed_step:?}"
        );
        assert_no_rename_temporaries(&fixture.root);
    }
}
