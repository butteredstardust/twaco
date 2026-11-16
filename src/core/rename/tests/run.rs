use super::*;

#[test]
fn run_refuses_a_failing_gate_before_planning_and_skip_checks_overrides_it() {
    let fixture = run_fixture();
    write(&fixture.root, "notes.md", "one\r\ntwo\n");
    let requested = spec(Kind::Entity, "P.Manager", "P.Director");
    assert!(matches!(
        run(&fixture.solution, &requested, &run_options(false, false), None),
        Err(RenameError::GatesFail { gates }) if gates.contains(&"line endings".to_string())
    ));
    let outcome = run(
        &fixture.solution,
        &requested,
        &run_options(false, true),
        None,
    )
    .unwrap();
    assert!(outcome.applied.is_none());
}

#[test]
fn run_dry_run_is_the_identity_and_apply_verifies_cleanly() {
    let fixture = run_fixture();
    let requested = spec(Kind::Entity, "P.Manager", "P.Director");
    let before = snapshot(&fixture.root);
    let dry = run(
        &fixture.solution,
        &requested,
        &run_options(false, false),
        None,
    )
    .unwrap();
    assert!(dry.applied.is_none());
    assert!(dry.verification.is_none());
    assert_eq!(snapshot(&fixture.root), before);

    let applied = run(
        &fixture.solution,
        &requested,
        &run_options(true, false),
        Some(&locked(&fixture)),
    )
    .unwrap();
    assert!(applied.applied.is_some());
    assert_eq!(
        applied.verification,
        Some(Verification {
            sync_problems: Vec::new(),
            blocking_gates: Vec::new(),
        })
    );
}

/// A Thing whose service script and sidecar agree, so the rename starts in step.
fn in_step_fixture() -> Fixture {
    let fixture = run_fixture();
    write(
            &fixture.root,
            "Things/P.Manager.xml",
            "<Entities><Things><Thing name=\"P.Manager\" projectName=\"P\"><ThingShape><ServiceDefinitions><ServiceDefinition name=\"Run\"><ResultType baseType=\"NOTHING\" description=\"\" name=\"result\"/><ParameterDefinitions/></ServiceDefinition></ServiceDefinitions><ServiceImplementations><ServiceImplementation description=\"\" handlerName=\"Script\" name=\"Run\"><ConfigurationTables><ConfigurationTable description=\"Script\" isMultiRow=\"false\" name=\"Script\" ordinal=\"0\"><Rows><Row><code><![CDATA[return Things[\"P.Manager\"].X;]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>
",
        );
    // Sidecars as `twaco extract` writes them, so the fixture is genuinely in step.
    let discovered = workspace::discover(&fixture.solution);
    let extracted = crate::core::workflow::extract(
        &fixture.solution,
        &discovered.entities,
        &[],
        true,
        &crate::core::lock::acquire(&fixture.solution.root, "test", &[]).unwrap(),
    );
    assert_eq!(discovered.entities.len(), 1, "{:?}", discovered.unreadable);
    assert_eq!(extracted.failed, 0, "{:?}", extracted.log);
    assert!(
        fixture
            .root
            .join("src/P.Manager/services/Run/definition.xml")
            .is_file(),
        "{:?} written {} entities {} under {:?}",
        extracted.log,
        extracted.written,
        extracted.entities,
        std::fs::read_dir(fixture.root.join("src"))
            .map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>())
    );
    assert!(
        sync_problems(&fixture.solution).is_empty(),
        "the fixture starts in step"
    );
    fixture
}

fn request(kind: Kind, names: &[&str]) -> Request {
    let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    Request::from_names(kind, &names).unwrap()
}

#[test]
fn one_request_decides_which_combinations_are_valid_for_every_front_end() {
    let root = Path::new("/ws");
    // Positionals: arity and shape per kind.
    assert!(Request::from_names(Kind::Entity, &["a".into()])
        .unwrap_err()
        .contains("<old> <new>"));
    assert!(
        Request::from_names(Kind::Param, &["a".into(), "b".into(), "c".into()])
            .unwrap_err()
            .contains("<entity> <service> <old> <new>")
    );
    let param = request(Kind::Param, &["E", "S", "old", "new"]);
    assert_eq!(
        (
            param.scope.as_deref(),
            param.service.as_deref(),
            param.old.as_str()
        ),
        (Some("E"), Some("S"), "old")
    );
    let (spec, options) = request(Kind::Field, &["D", "a", "b"])
        .build(root, "2026-10-02")
        .unwrap();
    assert_eq!(
        (spec.scope.as_deref(), options.date.as_str(), &options.sql),
        (Some("D"), "2026-10-02", &SqlChoice::Unset)
    );
    // A member rename has no text pass, and no database script unless it is a field.
    for kind in [Kind::Service, Kind::Table, Kind::Property, Kind::Field] {
        let mut text = request(kind, &["E", "a", "b"]);
        text.include_outside = true;
        assert!(text
            .build(root, "d")
            .unwrap_err()
            .contains("rename has no text pass"));
    }
    let mut sql = request(Kind::Table, &["E", "a", "b"]);
    sql.database.no_sql = true;
    assert!(sql
        .build(root, "d")
        .unwrap_err()
        .contains("no database script"));
    let mut field = request(Kind::Field, &["D", "a", "b"]);
    field.database.dir = Some("migrations".into());
    let (_, options) = field.clone().build(root, "d").unwrap();
    assert_eq!(options.sql, SqlChoice::Write(root.join("migrations")));
    field.database.no_sql = true;
    assert!(field
        .build(root, "d")
        .unwrap_err()
        .contains("say different things"));
    // Scope and service are required, or refused, by kind.
    let mut missing = request(Kind::Service, &["E", "a", "b"]);
    missing.scope = None;
    assert!(missing
        .build(root, "d")
        .unwrap_err()
        .contains("scope is required"));
    let mut extra = request(Kind::Entity, &["a", "b"]);
    extra.scope = Some("E".into());
    assert!(extra
        .build(root, "d")
        .unwrap_err()
        .contains("scope is only accepted"));
    let mut no_service = request(Kind::Param, &["E", "S", "a", "b"]);
    no_service.service = None;
    assert!(no_service
        .build(root, "d")
        .unwrap_err()
        .contains("service is required"));
    let mut stray = request(Kind::Service, &["E", "a", "b"]);
    stray.service = Some("S".into());
    assert!(stray
        .build(root, "d")
        .unwrap_err()
        .contains("service is only accepted"));
    // The words round trip, and the unknown-kind text lists them all.
    for kind in Kind::ALL {
        assert_eq!(Kind::from_word(kind.word()), Some(kind));
    }
    assert!(Kind::from_word("frob").is_none());
    assert!(
        Kind::list_words().starts_with("`entity`, `prefix`")
            && Kind::list_words().ends_with("or `property`")
    );
}

#[test]
fn a_rename_is_verified_in_a_scratch_copy_that_is_always_removed() {
    let fixture = in_step_fixture();
    let requested = spec(Kind::Entity, "P.Manager", "P.Director");
    let mut copies_seen = Vec::new();
    let outcome = run_with(
        &fixture.solution,
        &requested,
        &run_options(true, true),
        Some(&locked(&fixture)),
        &mut |copy| {
            assert!(copy
                .root
                .join("src/P.Director/services/Run/script.js")
                .is_file());
            copies_seen.push(copy.root.clone());
        },
    )
    .unwrap();
    assert_eq!(
        copies_seen.len(),
        1,
        "the rename was applied to a copy first"
    );
    assert!(!copies_seen[0].exists(), "the copy is removed");
    assert_eq!(
        outcome.verification.unwrap().sync_problems,
        Vec::<String>::new()
    );
}

#[test]
fn a_rename_that_would_leave_sidecars_out_of_step_writes_nothing() {
    let fixture = in_step_fixture();
    let before = snapshot(&fixture.root);
    let requested = spec(Kind::Entity, "P.Manager", "P.Director");
    // Stand in for a planner bug: the sidecar no longer matches the document in the copy.
    let mut copy_root = PathBuf::new();
    let error = run_with(
        &fixture.solution,
        &requested,
        &run_options(true, true),
        Some(&locked(&fixture)),
        &mut |copy| {
            copy_root = copy.root.clone();
            std::fs::write(
                copy.root.join("src/P.Director/services/Run/script.js"),
                "return 'not what the document says';",
            )
            .unwrap();
        },
    )
    .unwrap_err();
    match error {
        RenameError::WouldDesync { entities } => assert_eq!(entities, ["Things/P.Director"]),
        other => panic!("expected WouldDesync, got {other}"),
    }
    assert_eq!(
        snapshot(&fixture.root),
        before,
        "the real workspace was not touched"
    );
    assert!(!copy_root.exists(), "the copy is removed");
}

#[test]
fn sidecar_disagreement_is_a_run_preflight_refusal() {
    let fixture = run_fixture();
    write(
        &fixture.root,
        "src/P.Manager/services/Missing/script.js",
        "result = 1;\n",
    );
    let error = run(
        &fixture.solution,
        &spec(Kind::Entity, "P.Manager", "P.Director"),
        &run_options(false, false),
        None,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        RenameError::GatesFail { gates } if gates.contains(&"sidecars".to_string())
    ));
}

#[test]
fn a_plan_digest_applies_exactly_the_plan_that_was_reviewed() {
    let fixture = in_step_fixture();
    let requested = spec(Kind::Entity, "P.Manager", "P.Director");
    let reviewed = plan(&fixture.solution, &requested).unwrap().digest();
    assert_eq!(
        reviewed,
        plan(&fixture.solution, &requested).unwrap().digest(),
        "the same workspace gives the same digest"
    );
    assert_ne!(
        reviewed,
        plan(
            &fixture.solution,
            &spec(Kind::Entity, "P.Manager", "P.Other")
        )
        .unwrap()
        .digest()
    );
    // The workspace changes after the review: the digest no longer matches and nothing is written.
    write(
        &fixture.root,
        "docs/NOTE.md",
        "mentions P.Manager now
",
    );
    let before = snapshot(&fixture.root);
    let mut options = run_options(true, true);
    options.expect_digest = Some(reviewed.clone());
    assert!(matches!(
        run(&fixture.solution, &requested, &options, None),
        Err(RenameError::PlanChanged { .. })
    ));
    assert_eq!(snapshot(&fixture.root), before);
    // The digest of the plan as it now stands applies.
    options.include_outside = true;
    options.expect_digest = Some(plan(&fixture.solution, &requested).unwrap().digest());
    assert!(run(
        &fixture.solution,
        &requested,
        &options,
        Some(&locked(&fixture))
    )
    .unwrap()
    .applied
    .is_some());
}

#[test]
fn datashape_rename_keeps_definitions_and_scripts_in_step_with_and_without_text() {
    for old in ["Rows", "Acme.Rows_DS"] {
        for include_outside in [false, true] {
            let fixture = run_fixture();
            write(
                &fixture.root,
                "twaco.toml",
                "[[project]]\nname='Acme.Project'\ncollections=['Things','DataShapes']\n",
            );
            let solution = Solution::load(&fixture.root.join(CONFIG_FILE)).unwrap();
            let script = format!(
                r#"var t = DataShapes["{old}"];
Resources["InfoTableFunctions"].CreateInfoTableFromDataShape({{ infoTableName: "t", dataShapeName: "{old}" }});
// {old}
logger.info("{old}");
var unrelated = {{ dataShapeName: "{old}" }};
"#
            );
            let xml = format!(
                r#"<Entities><Things><Thing name="Acme.Reader" projectName="Acme.Project"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ResultType name="result" baseType="INFOTABLE" aspect.dataShape="{old}"/><ParameterDefinitions><FieldDefinition name="rows" baseType="INFOTABLE" aspect.dataShape="{old}"/></ParameterDefinitions></ServiceDefinition></ServiceDefinitions><ServiceImplementations><ServiceImplementation name="Run" handlerName="Script"><ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>
"#
            );
            write(&fixture.root, "Things/Acme.Reader.xml", &xml);
            write(
                &fixture.root,
                &format!("DataShapes/{old}.xml"),
                &format!(
                    r#"<Entities><DataShapes><DataShape name="{old}" projectName="Acme.Project"><FieldDefinitions/></DataShape></DataShapes></Entities>"#
                ),
            );
            let found = workspace::discover(&solution);
            let extracted = crate::core::workflow::extract(
                &solution,
                &found.entities,
                &[],
                true,
                &locked(&fixture),
            );
            assert_eq!(extracted.failed, 0, "{:?}", extracted.log);
            assert!(sync_problems(&solution).is_empty());
            let mut options = run_options(true, true);
            options.include_outside = include_outside;
            let outcome = run(
                &solution,
                &spec(Kind::Entity, old, "Acme.Items_DS"),
                &options,
                Some(&locked(&fixture)),
            )
            .unwrap();
            assert_eq!(
                outcome.verification.unwrap().sync_problems,
                Vec::<String>::new()
            );
            assert!(
                sync_problems(&solution).is_empty(),
                "sync --all must be an identity"
            );
            let script_after = std::fs::read_to_string(
                fixture.root.join("src/Acme.Reader/services/Run/script.js"),
            )
            .unwrap();
            assert!(script_after.contains(r#"DataShapes["Acme.Items_DS"]"#));
            assert!(script_after.contains(r#"dataShapeName: "Acme.Items_DS""#));
            assert!(script_after.contains(&format!("// {old}")));
            assert!(script_after.contains(&format!(r#"logger.info("{old}")"#)));
            assert!(
                script_after.contains(&format!(r#"var unrelated = {{ dataShapeName: "{old}" }}"#))
            );
            let definition = std::fs::read_to_string(
                fixture
                    .root
                    .join("src/Acme.Reader/services/Run/definition.xml"),
            )
            .unwrap();
            assert_eq!(
                definition
                    .matches(r#"aspect.dataShape="Acme.Items_DS""#)
                    .count(),
                2
            );
            assert!(
                outcome
                    .plan
                    .changes
                    .iter()
                    .filter(|c| c.path.ends_with("script.js"))
                    .flat_map(|c| &c.findings)
                    .filter(|f| !f.applied && f.tier == refs::Tier::Review)
                    .count()
                    >= 3
            );
            let entity_after =
                std::fs::read_to_string(fixture.root.join("Things/Acme.Reader.xml")).unwrap();
            assert!(entity_after.contains(script_after.trim_end()));
        }
    }
}

#[test]
fn entity_names_with_spaces_move_and_update_whole_references() {
    let fixture = run_fixture();
    write(
        &fixture.root,
        "Things/Acme Stream_thing.xml",
        r#"<Entities><Things><Thing name="Acme Stream_thing" projectName="P"/></Things></Entities>"#,
    );
    write(
        &fixture.root,
        "Things/Acme.Reader.xml",
        r#"<Entities><Things><Thing name="Acme.Reader" valueStream="Acme Stream_thing"><Reference>Acme Stream_thing</Reference><code><![CDATA[Things["Acme Stream_thing"].Run(); Things.Acme;]]></code></Thing></Things></Entities>"#,
    );
    run(
        &fixture.solution,
        &spec(Kind::Entity, "Acme Stream_thing", "Acme Stream2_thing"),
        &run_options(true, true),
        Some(&locked(&fixture)),
    )
    .unwrap();
    assert!(!fixture.root.join("Things/Acme Stream_thing.xml").exists());
    assert!(fixture.root.join("Things/Acme Stream2_thing.xml").is_file());
    let reader = std::fs::read_to_string(fixture.root.join("Things/Acme.Reader.xml")).unwrap();
    assert!(reader.contains(r#"Things["Acme Stream2_thing"]"#));
    assert!(reader.contains(r#"valueStream="Acme Stream2_thing""#));
    assert!(reader.contains("<Reference>Acme Stream2_thing</Reference>"));
    assert!(reader.contains("Things.Acme;"));
}

#[test]
fn a_name_inside_a_longer_spaced_entity_name_is_refused_and_writes_nothing() {
    let fixture = run_fixture();
    write(
        &fixture.root,
        "Things/Acme.Foo.xml",
        r#"<Entities><Things><Thing name="Acme.Foo" projectName="P"/></Things></Entities>"#,
    );
    write(
        &fixture.root,
        "Things/Acme.Foo Bar.xml",
        r#"<Entities><Things><Thing name="Acme.Foo Bar" projectName="P"/></Things></Entities>"#,
    );
    let reader = r#"<Entities><Things><Thing name="Acme.Reader" valueStream="Acme.Foo Bar"><code><![CDATA[Things["Acme.Foo Bar"].Run();]]></code></Thing></Things></Entities>"#;
    write(&fixture.root, "Things/Acme.Reader.xml", reader);
    let error = run(
        &fixture.solution,
        &spec(Kind::Entity, "Acme.Foo", "Acme.Baz"),
        &run_options(true, true),
        Some(&locked(&fixture)),
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("Acme.Foo Bar") && error.contains("Rename that entity first"),
        "{error}"
    );
    assert!(fixture.root.join("Things/Acme.Foo.xml").is_file());
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("Things/Acme.Reader.xml")).unwrap(),
        reader
    );
    // The longer name itself renames normally.
    run(
        &fixture.solution,
        &spec(Kind::Entity, "Acme.Foo Bar", "Acme.Foo Baz"),
        &run_options(true, true),
        Some(&locked(&fixture)),
    )
    .unwrap();
    let after = std::fs::read_to_string(fixture.root.join("Things/Acme.Reader.xml")).unwrap();
    assert!(after.contains(r#"Things["Acme.Foo Baz"]"#), "{after}");
}

#[test]
fn qualified_entity_rename_applies_and_verifies_without_renaming_another_identity() {
    for new in ["Acme.NewKey", "ApplicationKeys/Acme.NewKey"] {
        let fixture = run_fixture();
        write(
            &fixture.root,
            "twaco.toml",
            "[[project]]\nname='Acme.Project'\ncollections=['Things','ApplicationKeys']\n",
        );
        let solution = Solution::load(&fixture.root.join(CONFIG_FILE)).unwrap();
        write(
            &fixture.root,
            "Things/Acme.Key.xml",
            r#"<Entities><Things><Thing name="Acme.Key" projectName="Acme.Project" valueStream="Acme.Key"><code><![CDATA[Things["Acme.Key"].Run();]]></code></Thing></Things></Entities>"#,
        );
        write(
            &fixture.root,
            "ApplicationKeys/Acme.Key.xml",
            r#"<Entities><ApplicationKeys><ApplicationKey name="Acme.Key" projectName="Acme.Project"/></ApplicationKeys></Entities>"#,
        );
        let other = fixture.root.join("Things/Acme.Key.xml");
        let before = std::fs::read(&other).unwrap();
        let outcome = run(
            &solution,
            &spec(Kind::Entity, "ApplicationKeys/Acme.Key", new),
            &run_options(true, true),
            Some(&locked(&fixture)),
        )
        .unwrap();
        assert!(outcome.verification.unwrap().sync_problems.is_empty());
        assert!(fixture
            .root
            .join("ApplicationKeys/Acme.NewKey.xml")
            .is_file());
        assert_eq!(std::fs::read(&other).unwrap(), before);
        assert!(outcome
            .follow_up
            .iter()
            .all(|note| !note.contains("could not be planned")));
    }
}
