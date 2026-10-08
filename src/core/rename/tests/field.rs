use super::*;

fn field_fixture(tag: &str) -> Fixture {
    let root_guard = tempfile::Builder::new()
        .prefix(&format!("twaco-rename-field-{tag}-"))
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "twaco.toml", "[[project]]\nname = \"P\"\ncollections = [\"DataShapes\", \"Things\", \"ThingTemplates\"]\n");
    let shape = "<Entities><DataShapes><DataShape name=\"P.D\" projectName=\"P\"><FieldDefinitions><FieldDefinition name=\"Name\" baseType=\"STRING\" ordinal=\"1\" description=\"\"/><FieldDefinition name=\"Period\" baseType=\"STRING\" ordinal=\"2\" description=\"\"/><FieldDefinition name=\"PeriodDisplayName\" baseType=\"STRING\" ordinal=\"3\" description=\"\"/><FieldDefinition name=\"UID\" baseType=\"STRING\" ordinal=\"4\" description=\"\"/></FieldDefinitions></DataShape></DataShapes></Entities>\n";
    write(&root, "DataShapes/P.D.xml", shape);
    write(&root, "DataShapes/P.Other.xml", "<Entities><DataShapes><DataShape name=\"P.Other\" projectName=\"P\"><FieldDefinitions><FieldDefinition name=\"Period\" baseType=\"STRING\" ordinal=\"1\" description=\"\"/></FieldDefinitions></DataShape></DataShapes></Entities>\n");
    let fields = datashape::extract(shape.as_bytes()).unwrap();
    write(
        &root,
        "src/P.D/fields.json",
        &datashape::to_sidecar(&fields),
    );
    let thing = "\u{feff}<Entities>\r\n<Things><Thing name=\"P.T\" projectName=\"P\"><ConfigurationTables>\r\n<ConfigurationTable dataShapeName=\"P.D\" name=\"T\"><DataShape><FieldDefinitions><FieldDefinition name=\"Period\"/><FieldDefinition name=\"PeriodDisplayName\"/></FieldDefinitions></DataShape><Rows><Row><Period><![CDATA[a<b]]></Period><PeriodDisplayName>x</PeriodDisplayName></Row><Row><Period/></Row></Rows></ConfigurationTable>\r\n<ConfigurationTable dataShapeName=\"P.Other\" name=\"U\"><DataShape><FieldDefinitions><FieldDefinition name=\"Period\"/></FieldDefinitions></DataShape><Rows><Row><Period>stay</Period></Row></Rows></ConfigurationTable>\r\n</ConfigurationTables></Thing></Things></Entities>\r\n";
    write(&root, "Things/P.T.xml", thing);
    write(&root, "ThingTemplates/P.Base.xml", "<Entities><ThingTemplates><ThingTemplate name=\"P.Base\" projectName=\"P\"><ConfigurationTables><ConfigurationTable dataShapeName=\"P.D\" name=\"T\"><DataShape><FieldDefinitions><FieldDefinition name=\"Period\"/></FieldDefinitions></DataShape><Rows><Row><Period>template</Period></Row></Rows></ConfigurationTable></ConfigurationTables></ThingTemplate></ThingTemplates></Entities>\n");
    let solution = Solution::load(&root.join(CONFIG_FILE)).unwrap();
    Fixture {
        _dir: root_guard,
        root,
        solution,
    }
}

fn data_table_fixture(tag: &str) -> Fixture {
    let root_guard = tempfile::Builder::new()
        .prefix(&format!("twaco-rename-dt-{tag}-"))
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "twaco.toml", "[[project]]\nname = \"P\"\ncollections = [\"DataShapes\", \"Things\", \"ThingTemplates\"]\n");
    let shape = r#"<Entities><DataShapes><DataShape name="P.D" projectName="P"><FieldDefinitions><FieldDefinition name="Name" baseType="STRING" ordinal="1" description=""/><FieldDefinition name="Period" baseType="STRING" ordinal="2" description=""/></FieldDefinitions></DataShape></DataShapes></Entities>
"#;
    write(&root, "DataShapes/P.D.xml", shape);
    write(
        &root,
        "src/P.D/fields.json",
        &datashape::to_sidecar(&datashape::extract(shape.as_bytes()).unwrap())
            .replace('\n', "\r\n"),
    );
    // A template declares the property; the Thing only holds a value, so the type is inherited.
    write(
        &root,
        "ThingTemplates/P.Base.xml",
        r#"<Entities><ThingTemplates><ThingTemplate name="P.Base" projectName="P"><ThingShape><PropertyDefinitions><PropertyDefinition name="Limits" baseType="INFOTABLE" aspect.dataShape="P.D"/><PropertyDefinition name="Other" baseType="INFOTABLE" aspect.dataShape="P.Other"/></PropertyDefinitions></ThingShape></ThingTemplate></ThingTemplates></Entities>
"#,
    );
    write(
        &root,
        "Things/P.T.xml",
        r#"<Entities><Things><Thing name="P.T" projectName="P" thingTemplate="P.Base"><ThingProperties><Limits><Value><infoTable><DataShape><FieldDefinitions><FieldDefinition name="Name" baseType="STRING" ordinal="1"/><FieldDefinition name="Period" baseType="STRING" ordinal="2"/></FieldDefinitions></DataShape><Rows><Row><Name>a</Name><Period><![CDATA[Today]]></Period></Row></Rows></infoTable></Value></Limits><Other><Value><infoTable><DataShape><FieldDefinitions><FieldDefinition name="Period" baseType="STRING" ordinal="1"/></FieldDefinitions></DataShape><Rows><Row><Period>x</Period></Row></Rows></infoTable></Value></Other></ThingProperties></Thing></Things></Entities>
"#,
    );
    let table = r#"<Entities><Things><Thing name="P.Rows_DT" projectName="P" thingTemplate="DataTable"><ConfigurationTables><ConfigurationTable name="Settings"><Rows><Row><accumulatedDataShape><json><![CDATA[{"fieldDefinitions":{"Name":{"name":"Name","baseType":"STRING"},"Period":{"name":"Period","baseType":"STRING"}}}]]></json></accumulatedDataShape><dataShape><![CDATA[P.D]]></dataShape></Row></Rows></ConfigurationTable><ConfigurationTable name="Indexes"><Rows><Row><name><![CDATA[byBoth]]></name><fieldNames><![CDATA[Name,Period]]></fieldNames></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>
"#;
    write(&root, "Things/P.Rows_DT.xml", table);
    write(
        &root,
        "src/P.Rows_DT/datatable.json",
        &datatable::to_sidecar(&datatable::extract(table.as_bytes()).unwrap()).unwrap(),
    );
    write(
        &root,
        "src/P.T/services/Run/script.js",
        "const first = rows.row.Period;\nconst label = 'Period';\nreturn first;\n",
    );
    let solution = Solution::load(&root.join(CONFIG_FILE)).unwrap();
    Fixture {
        _dir: root_guard,
        root,
        solution,
    }
}

#[test]
fn a_field_rename_reaches_inherited_infotable_values_datatables_and_lists_other_mentions() {
    let fixture = data_table_fixture("all");
    let before = snapshot(&fixture.root);
    let planned = plan(&fixture.solution, &field_spec("P.D", "Period", "PeriodKey")).unwrap();
    assert_eq!(snapshot(&fixture.root), before, "a plan writes nothing");
    // Two mentions in the script are for a person: the member access and the quoted literal.
    let review: usize = planned
        .changes
        .iter()
        .flat_map(|c| &c.findings)
        .filter(|f| !f.applied)
        .count();
    assert_eq!(review, 2);
    apply(
        &fixture.solution,
        &planned,
        &options(false),
        &locked(&fixture),
    )
    .unwrap();

    let thing = std::fs::read_to_string(fixture.root.join("Things/P.T.xml")).unwrap();
    // The value typed by P.D (declared on the template) changed; the one typed by P.Other did not.
    assert!(thing.contains("<Limits><Value><infoTable><DataShape><FieldDefinitions><FieldDefinition name=\"Name\" baseType=\"STRING\" ordinal=\"1\"/><FieldDefinition name=\"PeriodKey\""), "{thing}");
    assert!(
        !thing.contains("<Period><![CDATA[Today]]></Period>")
            && thing.contains("<PeriodKey><![CDATA[Today]]></PeriodKey>"),
        "{thing}"
    );
    assert!(thing.contains("<Other><Value><infoTable><DataShape><FieldDefinitions><FieldDefinition name=\"Period\""), "{thing}");
    assert!(thing.contains("<Period>x</Period>"), "{thing}");

    let table = std::fs::read_to_string(fixture.root.join("Things/P.Rows_DT.xml")).unwrap();
    assert!(
        table.contains("\"PeriodKey\":{\"name\":\"PeriodKey\""),
        "{table}"
    );
    assert!(table.contains("<![CDATA[Name,PeriodKey]]>"), "{table}");
    // The sidecar is what a fresh extract writes, in the existing file's line endings (LF here).
    let sidecar =
        std::fs::read_to_string(fixture.root.join("src/P.Rows_DT/datatable.json")).unwrap();
    assert_eq!(
        sidecar,
        datatable::to_sidecar(&datatable::extract(table.as_bytes()).unwrap()).unwrap()
    );
    // fields.json was CRLF and stays CRLF.
    let fields = std::fs::read(fixture.root.join("src/P.D/fields.json")).unwrap();
    assert!(
        fields.windows(2).any(|pair| pair == [13, 10])
            && !fields.windows(2).any(|pair| pair == [10, 10])
    );
    // The script is untouched: its mentions are listed, not rewritten.
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("src/P.T/services/Run/script.js")).unwrap(),
        "const first = rows.row.Period;\nconst label = 'Period';\nreturn first;\n"
    );

    // And back, byte for byte (the ledger is the only addition).
    let back = plan(&fixture.solution, &field_spec("P.D", "PeriodKey", "Period")).unwrap();
    apply(&fixture.solution, &back, &options(false), &locked(&fixture)).unwrap();
    assert_eq!(snapshot_without_twaco(&fixture.root), before);
}

#[test]
fn a_datatable_field_rename_refuses_when_the_new_name_exists_or_the_json_is_not_compact() {
    let fixture = data_table_fixture("refuse");
    let mut table = std::fs::read_to_string(fixture.root.join("Things/P.Rows_DT.xml")).unwrap();
    table = table.replace("{\"fieldDefinitions\"", "{ \"fieldDefinitions\"");
    std::fs::write(fixture.root.join("Things/P.Rows_DT.xml"), &table).unwrap();
    let error = plan(&fixture.solution, &field_spec("P.D", "Period", "PeriodKey")).unwrap_err();
    assert!(error.to_string().contains("compact form"), "{error}");
    let clash = data_table_fixture("clash");
    let error = plan(&clash.solution, &field_spec("P.D", "Period", "Name")).unwrap_err();
    assert!(matches!(error, RenameError::Exists { .. }), "{error}");
}

pub(super) fn field_spec(scope: &str, old: &str, new: &str) -> Spec {
    Spec {
        kind: Kind::Field,
        old: old.to_string(),
        new: new.to_string(),
        scope: Some(scope.to_string()),
        service: None,
    }
}

#[test]
fn field_rename_edits_shape_sidecar_and_only_matching_configuration_tables() {
    let fixture = field_fixture("all-places");
    let before = snapshot(&fixture.root);
    let requested = field_spec("P.D", "Period", "PeriodKey");
    let planned = plan(&fixture.solution, &requested).unwrap();
    assert_eq!(snapshot(&fixture.root), before, "planning writes nothing");
    assert!(planned.moves.is_empty() && planned.baseline_keys.is_empty());
    assert!(planned.outside.is_empty() && planned.named.is_empty() && planned.skipped.is_empty());
    assert_eq!(planned.field_tables, 2);
    assert_eq!(follow_up(&planned, false), [
            "The rows of a configuration table keep their values on the server unless you deploy with --overwrite-tables",
            "Scripts, mashup bindings and services that read the field by name are not changed; search for \"Period\"",
            "If this shape is stored through DBConnection, the column must be renamed in the database before the import",
        ]);
    apply(
        &fixture.solution,
        &planned,
        &options(false),
        &locked(&fixture),
    )
    .unwrap();

    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join(".twaco/renames.json")).unwrap())
            .unwrap();
    let record = &ledger[0];
    assert_eq!(record["kind"], "field");
    assert_eq!(record["scope"], "P.D");
    assert_eq!(record["old"], "Period");
    assert_eq!(record["new"], "PeriodKey");
    assert_eq!(record["entities"].as_array().unwrap().len(), 3);
    assert!(record["entities"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["old"] == item["new"]));

    let shape = std::fs::read(fixture.root.join("DataShapes/P.D.xml")).unwrap();
    let fields = datashape::extract(&shape).unwrap();
    assert!(fields.iter().any(|field| field.name == "PeriodKey"));
    assert!(!fields.iter().any(|field| field.name == "Period"));
    assert_eq!(
        std::fs::read(fixture.root.join("src/P.D/fields.json")).unwrap(),
        datashape::to_sidecar(&fields).as_bytes()
    );
    let thing = std::fs::read_to_string(fixture.root.join("Things/P.T.xml")).unwrap();
    assert!(
        thing.contains("name=\"PeriodKey\"")
            && thing.contains("<PeriodKey><![CDATA[a<b]]></PeriodKey>")
            && thing.contains("<PeriodKey/>")
    );
    assert!(
        thing.contains("name=\"PeriodDisplayName\"")
            && thing.contains("<PeriodDisplayName>x</PeriodDisplayName>")
    );
    assert!(thing.contains("dataShapeName=\"P.Other\"") && thing.contains("<Period>stay</Period>"));
    let template = std::fs::read_to_string(fixture.root.join("ThingTemplates/P.Base.xml")).unwrap();
    assert!(template.contains("<PeriodKey>template</PeriodKey>"));

    let reverse = plan(&fixture.solution, &field_spec("P.D", "PeriodKey", "Period")).unwrap();
    apply(
        &fixture.solution,
        &reverse,
        &options(false),
        &locked(&fixture),
    )
    .unwrap();
    for relative in [
        "DataShapes/P.D.xml",
        "src/P.D/fields.json",
        "Things/P.T.xml",
        "ThingTemplates/P.Base.xml",
    ] {
        assert_eq!(
            std::fs::read(fixture.root.join(relative)).unwrap(),
            before[&PathBuf::from(relative)],
            "{relative}"
        );
    }
}

#[test]
fn field_rename_refuses_unknown_invalid_existing_and_inline_conflicts() {
    let fixture = field_fixture("refusals");
    assert!(matches!(
        plan(&fixture.solution, &field_spec("Missing", "Period", "X")),
        Err(RenameError::Unknown { .. })
    ));
    assert!(matches!(
        plan(&fixture.solution, &field_spec("P.T", "Period", "X")),
        Err(RenameError::Unknown { .. })
    ));
    assert!(matches!(
        plan(&fixture.solution, &field_spec("P.D", "Missing", "X")),
        Err(RenameError::Unknown { .. })
    ));
    assert!(matches!(
        plan(&fixture.solution, &field_spec("P.D", "Period", "Name")),
        Err(RenameError::Exists { .. })
    ));
    assert!(matches!(
        plan(&fixture.solution, &field_spec("P.D", "Period", "Period")),
        Err(RenameError::Same { .. })
    ));
    for bad in ["", "1x", "a b", "a.b"] {
        assert!(
            matches!(
                plan(&fixture.solution, &field_spec("P.D", "Period", bad)),
                Err(RenameError::InvalidNew { .. })
            ),
            "{bad:?}"
        );
        assert!(
            matches!(
                plan(&fixture.solution, &field_spec("P.D", bad, "Valid")),
                Err(RenameError::InvalidOld { .. })
            ),
            "{bad:?}"
        );
    }
    let path = fixture.root.join("ThingTemplates/P.Base.xml");
    let text = std::fs::read_to_string(&path).unwrap().replace(
        "<FieldDefinition name=\"Period\"/>",
        "<FieldDefinition name=\"Period\"/><FieldDefinition name=\"PeriodKey\"/>",
    );
    std::fs::write(path, text).unwrap();
    assert!(matches!(
        plan(&fixture.solution, &field_spec("P.D", "Period", "PeriodKey")),
        Err(RenameError::Exists { .. })
    ));
}

#[test]
fn every_injected_field_apply_failure_rolls_back_the_complete_tree() {
    let count_fixture = field_fixture("rollback-count");
    let count_plan = plan(
        &count_fixture.solution,
        &field_spec("P.D", "Period", "PeriodKey"),
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
            >= 3
    );
    for (fail_at, failed_step) in steps.iter().enumerate() {
        let fixture = field_fixture("rollback");
        let planned = plan(&fixture.solution, &field_spec("P.D", "Period", "PeriodKey")).unwrap();
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
