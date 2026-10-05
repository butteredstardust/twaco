use super::field::field_spec;
use super::*;

fn db_fixture(tag: &str) -> Fixture {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "twaco-rename-db-{tag}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    write(
        &root,
        "twaco.toml",
        "[[project]]\nname = \"P\"\ncollections = [\"DataShapes\", \"ThingTemplates\"]\n",
    );
    for (name, fields) in [
        ("P.Dashboards", ["UID", "UserName"]),
        ("P.Shares", ["UID", "UserName"]),
    ] {
        let shape = format!("<Entities><DataShapes><DataShape name=\"{name}\" projectName=\"P\"><FieldDefinitions><FieldDefinition name=\"{}\" baseType=\"LONG\" ordinal=\"1\" description=\"\"/><FieldDefinition name=\"{}\" baseType=\"STRING\" ordinal=\"2\" description=\"\"/></FieldDefinitions></DataShape></DataShapes></Entities>\n", fields[0], fields[1]);
        write(&root, &format!("DataShapes/{name}.xml"), &shape);
        write(
            &root,
            &format!("src/{name}/fields.json"),
            &datashape::to_sidecar(&datashape::extract(shape.as_bytes()).unwrap()),
        );
    }
    let script = r#"var result = {
    dbInfo: [
        {
            dataShapeName: "P.Dashboards",
            fields: [ { name: "UserName", notNull: true } ],
            indexedFields: [ { name: "UserName", unique: false } ]
        },
        {
            dataShapeName: "P.Shares",
            fields: [ { name: "UserName", notNull: true } ],
            indexedFields: [ { fieldNames: ["UID", "UserName"], unique: true } ],
            foreignKeys: [ { name: "Owner", referenceDataShapeName: "P.Dashboards", referenceFieldName: "UserName" } ]
        }
    ]
};
"#;
    write(&root, "ThingTemplates/P.Manager_TT.xml", &format!("<Entities><ThingTemplates><ThingTemplate name=\"P.Manager_TT\" projectName=\"P\" baseThingTemplate=\"GenericThing\"><ThingShape><ServiceImplementations><ServiceImplementation name=\"GetDBInfo\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></ThingTemplate></ThingTemplates></Entities>\n"));
    let solution = Solution::load(&root.join(CONFIG_FILE)).unwrap();
    Fixture { root, solution }
}

fn db_options(root: &Path, sql: SqlChoice) -> RunOptions {
    RunOptions {
        apply: true,
        include_outside: false,
        skip_checks: true,
        date: "2026-10-02".to_string(),
        sql: match sql {
            SqlChoice::Write(_) => SqlChoice::Write(root.join("sql")),
            other => other,
        },
        expect_digest: None,
    }
}

#[test]
fn a_rename_that_touches_a_dbconnection_table_is_refused_until_the_caller_chooses() {
    let fixture = db_fixture("refuse");
    let before = snapshot(&fixture.root);
    let error = run(
        &fixture.solution,
        &field_spec("P.Dashboards", "UserName", "OwnerName"),
        &db_options(&fixture.root, SqlChoice::Unset),
    )
    .unwrap_err();
    match &error {
        RenameError::DatabaseHalf { shapes, .. } => assert_eq!(shapes, &["P.Dashboards"]),
        other => panic!("{other}"),
    }
    assert!(error.to_string().contains("--sql") && error.to_string().contains("--no-sql"));
    assert_eq!(snapshot(&fixture.root), before, "nothing was written");
    // An entity rename is also refused: rows name entities.
    let error = run(
        &fixture.solution,
        &spec(Kind::Entity, "P.Dashboards", "P.Boards"),
        &db_options(&fixture.root, SqlChoice::Unset),
    )
    .unwrap_err();
    assert!(matches!(error, RenameError::DatabaseHalf { .. }), "{error}");
    // A shape that is not in GetDBInfo is not a table.
    let plain = db_fixture("plain");
    write(&plain.root, "DataShapes/P.Plain.xml", "<Entities><DataShapes><DataShape name=\"P.Plain\" projectName=\"P\"><FieldDefinitions><FieldDefinition name=\"Name\" baseType=\"STRING\" ordinal=\"1\" description=\"\"/></FieldDefinitions></DataShape></DataShapes></Entities>\n");
    write(
        &plain.root,
        "src/P.Plain/fields.json",
        &datashape::to_sidecar(
            &datashape::extract(
                std::fs::read(plain.root.join("DataShapes/P.Plain.xml"))
                    .unwrap()
                    .as_slice(),
            )
            .unwrap(),
        ),
    );
    let solution = Solution::load(&plain.root.join(CONFIG_FILE)).unwrap();
    run(
        &solution,
        &field_spec("P.Plain", "Name", "Title"),
        &db_options(&plain.root, SqlChoice::Unset),
    )
    .unwrap();
}

#[test]
fn a_field_rename_follows_get_db_info_for_its_own_shape_and_writes_the_column_migration() {
    let fixture = db_fixture("field");
    let before = snapshot(&fixture.root);
    let outcome = run(
        &fixture.solution,
        &field_spec("P.Dashboards", "UserName", "OwnerName"),
        &db_options(&fixture.root, SqlChoice::Write(PathBuf::new())),
    )
    .unwrap();
    let script =
        std::fs::read_to_string(fixture.root.join("ThingTemplates/P.Manager_TT.xml")).unwrap();
    // The scoped shape's own entries and the foreign key that points at the column changed;
    // the other shape's own UserName did not.
    assert!(
        script.contains(
            "dataShapeName: \"P.Dashboards\",\n            fields: [ { name: \"OwnerName\""
        ),
        "{script}"
    );
    assert!(
        script.contains("indexedFields: [ { name: \"OwnerName\", unique: false } ]"),
        "{script}"
    );
    assert!(
        script.contains("referenceFieldName: \"OwnerName\""),
        "{script}"
    );
    assert!(
        script.contains("dataShapeName: \"P.Shares\",\n            fields: [ { name: \"UserName\""),
        "{script}"
    );
    assert!(
        script.contains("fieldNames: [\"UID\", \"UserName\"]"),
        "{script}"
    );
    let sql = std::fs::read_to_string(
        fixture
            .root
            .join("sql/2026-10-02-rename-field-Dashboards-UserName-to-OwnerName.sql"),
    )
    .unwrap();
    assert!(
        sql.contains("ALTER TABLE \"dashboards\" RENAME COLUMN \"username\" TO \"ownername\";"),
        "{sql}"
    );
    assert_eq!(outcome.sql.as_ref().unwrap().text, sql);
    assert!(outcome
        .verification
        .is_some_and(|v| v.sync_problems.is_empty()));
    // And back: every repository file is identical; only the two migration files are new.
    run(
        &fixture.solution,
        &field_spec("P.Dashboards", "OwnerName", "UserName"),
        &db_options(&fixture.root, SqlChoice::Write(PathBuf::new())),
    )
    .unwrap();
    let mut after = snapshot_without_twaco(&fixture.root);
    after.retain(|path, _| !path.starts_with("sql"));
    assert_eq!(after, before);
}

#[test]
fn an_entity_rename_of_a_dbconnection_shape_renames_the_table_and_its_derived_names() {
    let fixture = db_fixture("table");
    let outcome = run(
        &fixture.solution,
        &spec(Kind::Entity, "P.Dashboards", "P.Boards"),
        &db_options(&fixture.root, SqlChoice::Write(PathBuf::new())),
    )
    .unwrap();
    let sql = outcome.sql.unwrap().text;
    assert!(
        sql.contains("ALTER TABLE IF EXISTS \"dashboards\" RENAME TO \"boards\";"),
        "{sql}"
    );
    assert!(
        sql.contains("\"dashboards_pkey\" RENAME TO \"boards_pkey\"")
            && sql.contains("\"dashboards_username_idx\" RENAME TO \"boards_username_idx\""),
        "{sql}"
    );
    // Rows name entities: both tables, under their names after the rename.
    assert!(sql.contains("table_name IN ('boards', 'shares')"), "{sql}");
    assert!(sql.contains("'P.Dashboards', 'P.Boards'"), "{sql}");
    // The literal in GetDBInfo follows by the ordinary name rule, including the reference.
    let script =
        std::fs::read_to_string(fixture.root.join("ThingTemplates/P.Manager_TT.xml")).unwrap();
    assert!(
        script.contains("dataShapeName: \"P.Boards\"")
            && script.contains("referenceDataShapeName: \"P.Boards\"")
            && !script.contains("P.Dashboards"),
        "{script}"
    );
}

#[test]
fn no_sql_writes_no_script_and_a_failed_apply_removes_the_one_it_wrote() {
    let fixture = db_fixture("nosql");
    let outcome = run(
        &fixture.solution,
        &field_spec("P.Dashboards", "UserName", "OwnerName"),
        &db_options(&fixture.root, SqlChoice::Off),
    )
    .unwrap();
    assert!(outcome.sql.is_none() && !fixture.root.join("sql").exists());

    let failing = db_fixture("rollback");
    let planned = plan(
        &failing.solution,
        &field_spec("P.Dashboards", "UserName", "OwnerName"),
    )
    .unwrap();
    let before = snapshot(&failing.root);
    let extra = failing.root.join("sql/migration.sql");
    let error = apply_with(
        &failing.solution,
        &planned,
        &ApplyOptions {
            include_outside: false,
            date: "2026-10-02".to_string(),
            extra_files: vec![(extra.clone(), b"-- migration".to_vec())],
        },
        &mut |step| {
            if matches!(step, Step::Ledger) {
                Err(std::io::Error::other("injected"))
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
        snapshot(&failing.root),
        before,
        "the script and its folder are gone"
    );
    assert!(!failing.root.join("sql").exists());
    // A migration file that already exists is refused before anything is written.
    write(&failing.root, "sql/migration.sql", "somebody's");
    let error = apply(
        &failing.solution,
        &planned,
        &ApplyOptions {
            include_outside: false,
            date: "d".to_string(),
            extra_files: vec![(extra, b"x".to_vec())],
        },
    )
    .unwrap_err();
    assert!(matches!(error, RenameError::Exists { .. }), "{error}");
}

/// The refusal asks for a decision, so it has to say what the decision is about. Naming an
/// unreadable `GetDBInfo` when every one was read sent people looking for a script that was fine.
#[test]
fn the_database_refusal_says_why_it_is_asking() {
    let said = |shapes: Vec<String>, unsure: Vec<String>| {
        RenameError::DatabaseHalf { shapes, unsure }.to_string()
    };
    assert!(
        said(vec!["P.Dashboards".into()], vec![]).contains("DBConnection table(s) of P.Dashboards")
    );
    let unreadable = said(vec![], vec!["Things/P.Db".into()]);
    assert!(
        unreadable.contains("a GetDBInfo it could not read completely")
            && unreadable.contains("(could not read completely: Things/P.Db)"),
        "{unreadable}"
    );
    let names = said(vec![], vec![]);
    assert!(
        names.contains("whose rows store entity names") && !names.contains("could not read"),
        "{names}"
    );
}
