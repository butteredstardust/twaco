use super::*;

fn service_fixture(tag: &str) -> Fixture {
    let root_guard = tempfile::Builder::new()
        .prefix(&format!("twaco-rename-service-{tag}-"))
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(
        &root,
        "twaco.toml",
        r#"[validate]
inherited_overrides = ["P.Template.Run", "Run", "P.Other.Run"]
[[project]]
name = "P"
collections = ["ThingShapes", "ThingTemplates", "Things", "Mashups"]
[project.deploy]
entry_point_thing = "P.Child"
deploy_service = "Run"
[[project.deploy.post_import]]
thing = "P.Child"
service = "Run"
"#,
    );
    let service = |name: &str, script: &str| {
        format!("<ServiceDefinitions><ServiceDefinition name=\"{name}\"/></ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"{name}\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>")
    };
    write(&root, "ThingShapes/P.Shape.xml", &format!("<Entities><ThingShapes><ThingShape name=\"P.Shape\" projectName=\"P\">{}</ThingShape></ThingShapes></Entities>\n", service("Run", "me.Run();")));
    write(&root, "ThingTemplates/P.Template.xml", &format!("<Entities><ThingTemplates><ThingTemplate name=\"P.Template\" projectName=\"P\" baseThingTemplate=\"GenericThing\"><ImplementedShapes><ImplementedShape name=\"P.Shape\"/></ImplementedShapes><ThingShape>{}</ThingShape></ThingTemplate></ThingTemplates></Entities>\n", service("Run", "this.Run();")));
    write(&root, "Things/P.Child.xml", "<Entities><Things><Thing name=\"P.Child\" projectName=\"P\" thingTemplate=\"P.Template\"><ThingShape><ServiceDefinitions/><ServiceImplementations/></ThingShape></Thing></Things></Entities>\n");
    write(&root, "Things/P.Caller.xml", &format!("<Entities><Things><Thing name=\"P.Caller\" projectName=\"P\" thingTemplate=\"GenericThing\"><ThingShape>{}</ThingShape></Thing></Things></Entities>\n", service("Call", "const x = Things[\"P.Child\"]; x.Run(); Things.POther.Run(); Things[\"P.Other\"].Run();")));
    write(&root, "Things/P.Other.xml", &format!("<Entities><Things><Thing name=\"P.Other\" projectName=\"P\" thingTemplate=\"GenericThing\"><ThingShape>{}</ThingShape></Thing></Things></Entities>\n", service("Run", "me.Run();")));
    for entity in ["P.Shape", "P.Template"] {
        write(
            &root,
            &format!("src/{entity}/services/Run/definition.xml"),
            "<ServiceDefinition name=\"Run\"/>\n",
        );
        write(
            &root,
            &format!("src/{entity}/services/Run/script.js"),
            "/** @function Run */\nme.Run();\nconst note = \"Run\";",
        );
    }
    let content = r#"{"Data":{"D":{"DataName":"D","EntityName":"P.Child","Services":[{"Name":"Run","Target":"Run"}]},"U":{"DataName":"U","EntityName":"P.Other","Services":[{"Name":"Run","Target":"Run"}]}},"DataBindings":[{"SourceId":"Run","SourceName":"Run","SourceSection":"D","TargetId":"Run","TargetSection":"D"}],"Events":[{"EventHandlerId":"D","EventHandlerService":"Run","EventTriggerId":"Run","EventTriggerSection":"D"}],"Label":"Run"}"#;
    write(&root, "Mashups/P.View.xml", &format!("<Entities><Mashups><Mashup name=\"P.View\" projectName=\"P\"><mashupContent><![CDATA[{content}]]></mashupContent></Mashup></Mashups></Entities>\n"));
    write(&root, "src/P.View/mashup/content.json", content);
    let solution = Solution::load(&root.join(CONFIG_FILE)).unwrap();
    Fixture {
        _dir: root_guard,
        root,
        solution,
    }
}

fn service_spec(scope: &str, old: &str, new: &str) -> Spec {
    Spec {
        kind: Kind::Service,
        old: old.to_string(),
        new: new.to_string(),
        scope: Some(scope.to_string()),
        service: None,
    }
}

#[test]
fn service_rename_moves_overrides_and_edits_only_resolved_callers_then_round_trips() {
    let fixture = service_fixture("all-places");
    let before = snapshot(&fixture.root);
    let planned = plan(
        &fixture.solution,
        &service_spec("P.Shape", "Run", "Execute"),
    )
    .unwrap();
    assert_eq!(snapshot(&fixture.root), before, "planning writes nothing");
    assert_eq!(
        planned
            .moves
            .iter()
            .map(|item| item.old_name.as_str())
            .collect::<Vec<_>>(),
        ["P.Shape", "P.Template"]
    );
    assert!(
        planned.baseline_keys.is_empty() && planned.outside.is_empty() && planned.named.is_empty()
    );
    assert_eq!(planned.service_mashups, 2);
    assert!(planned
        .changes
        .iter()
        .flat_map(|change| &change.findings)
        .any(|finding| finding.tier == refs::Tier::Review));
    apply(
        &fixture.solution,
        &planned,
        &options(false),
        &locked(&fixture),
    )
    .unwrap();
    for entity in ["P.Shape", "P.Template"] {
        assert!(!fixture
            .root
            .join(format!("src/{entity}/services/Run"))
            .exists());
        assert!(fixture
            .root
            .join(format!("src/{entity}/services/Execute"))
            .is_dir());
    }
    let caller = std::fs::read_to_string(fixture.root.join("Things/P.Caller.xml")).unwrap();
    assert!(
        caller.contains("x.Execute()")
            && caller.contains("Things.POther.Run()")
            && caller.contains("Things[\"P.Other\"].Run()")
    );
    let config = std::fs::read_to_string(fixture.root.join(CONFIG_FILE)).unwrap();
    assert!(
        config.contains("P.Template.Execute") && config.contains("deploy_service = \"Execute\"")
    );
    assert!(config.contains("\"Run\", \"P.Other.Run\""));
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join(".twaco/renames.json")).unwrap())
            .unwrap();
    assert_eq!(ledger[0]["kind"], "service");
    assert_eq!(ledger[0]["scope"], "P.Shape");
    assert_eq!(ledger[0]["entities"].as_array().unwrap().len(), 2);

    let reverse = plan(
        &fixture.solution,
        &service_spec("P.Shape", "Execute", "Run"),
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
fn service_rename_refuses_unknown_inherited_invalid_same_and_conflicting_names() {
    let fixture = service_fixture("refusals");
    assert!(matches!(
        plan(&fixture.solution, &service_spec("Missing", "Run", "X")),
        Err(RenameError::Unknown { .. })
    ));
    assert!(matches!(
        plan(&fixture.solution, &service_spec("P.Shape", "Missing", "X")),
        Err(RenameError::ServiceScope { .. })
    ));
    let inherited = plan(&fixture.solution, &service_spec("P.Child", "Run", "X"))
        .unwrap_err()
        .to_string();
    assert!(inherited.contains("declared on P.Template") && inherited.contains("rename it there"));
    assert!(matches!(
        plan(&fixture.solution, &service_spec("P.Shape", "Run", "Run")),
        Err(RenameError::Same { .. })
    ));
    for bad in ["", "1x", "a.b", "a b"] {
        assert!(matches!(
            plan(&fixture.solution, &service_spec("P.Shape", "Run", bad)),
            Err(RenameError::InvalidNew { .. })
        ));
    }
    let path = fixture.root.join("ThingTemplates/P.Template.xml");
    let text = std::fs::read_to_string(&path).unwrap().replace(
        "<ServiceDefinition name=\"Run\"/>",
        "<ServiceDefinition name=\"Run\"/><ServiceDefinition name=\"Execute\"/>",
    );
    std::fs::write(path, text).unwrap();
    assert!(matches!(
        plan(
            &fixture.solution,
            &service_spec("P.Shape", "Run", "Execute")
        ),
        Err(RenameError::Exists { .. })
    ));
}

#[test]
fn every_injected_service_apply_failure_restores_files_and_folders() {
    let count_fixture = service_fixture("rollback-count");
    let count_plan = plan(
        &count_fixture.solution,
        &service_spec("P.Shape", "Run", "Execute"),
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
    assert!(steps.iter().any(|step| matches!(step, Step::Move(_, _))));
    for (fail_at, failed_step) in steps.iter().enumerate() {
        let fixture = service_fixture("rollback");
        let planned = plan(
            &fixture.solution,
            &service_spec("P.Shape", "Run", "Execute"),
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
