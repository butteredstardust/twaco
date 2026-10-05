use super::*;

#[test]
fn follow_up_contains_only_applicable_items_in_order() {
    let fixture = fixture();
    let mut planned = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    let mut membership = planned.moves[0].clone();
    membership.collection = "Groups".to_string();
    planned.moves.push(membership);
    let items = follow_up(&planned, false);
    assert_eq!(items.len(), 6);
    assert!(items[0].starts_with("The old entities stay"));
    assert!(items[1].starts_with("Persisted property values"));
    assert!(items[2].starts_with("Memberships are server state"));
    assert!(items[3].starts_with("Another project"));
    assert!(items[4].contains("other file(s) were not changed"));

    let review = plan(&fixture.solution, &spec(Kind::Entity, "T", "U")).unwrap();
    let items = follow_up(&review, true);
    assert_eq!(items.len(), 3);
    assert!(items[2].contains("left for a person to review"));
}

#[test]
fn prefix_plan_covers_moves_sidecars_config_and_outside_text() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    assert_eq!(
        planned
            .moves
            .iter()
            .map(|item| (item.old_name.as_str(), item.new_name.as_str()))
            .collect::<Vec<_>>(),
        [
            ("Acme.App.Model_DS", "Acme.New.Model_DS"),
            ("Acme.App.Dashboard", "Acme.New.Dashboard"),
            ("Acme.App", "Acme.New"),
            ("Acme.App.Manager.Child", "Acme.New.Manager.Child"),
            ("Acme.App.Manager", "Acme.New.Manager"),
        ]
    );
    for item in &planned.moves {
        assert_eq!(
            item.new_file.file_name().unwrap().to_string_lossy(),
            format!("{}.xml", item.new_name)
        );
    }
    let sidecar_moves: Vec<&str> = planned
        .moves
        .iter()
        .filter(|item| item.old_sidecars.is_some())
        .map(|item| item.old_name.as_str())
        .collect();
    assert_eq!(sidecar_moves, ["Acme.App.Dashboard", "Acme.App.Manager"]);
    assert!(planned
        .changes
        .iter()
        .any(|change| change.kind == FileKind::Config));
    assert!(planned.changes.iter().any(|change| {
        relative(&fixture.root, &change.path).ends_with("content.json")
            && change.findings.iter().any(|finding| {
                finding
                    .excerpt
                    .contains("DynamicThingShapes_Acme.App.Manager")
            })
    }));
    let outside: Vec<String> = planned
        .outside
        .iter()
        .map(|change| relative(&fixture.root, &change.path))
        .collect();
    assert_eq!(outside, ["docs/GUIDE.md", "sql/m.sql"]);
    assert!(!outside.iter().any(|path| path == "ignored.txt"));
    assert_eq!(
        planned.baseline_keys,
        planned
            .moves
            .iter()
            .map(|item| (item.collection.clone(), item.old_name.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn entity_plan_moves_one_and_does_not_touch_a_dotted_child() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Entity, "Acme.App.Manager", "Acme.App.Director"),
    )
    .unwrap();
    assert_eq!(planned.moves.len(), 1);
    assert_eq!(planned.moves[0].new_name, "Acme.App.Director");
    let child = fixture.root.join("Things/Acme.App.Manager.Child.xml");
    assert!(!planned.changes.iter().any(|change| change.path == child));
    let counts = planned.counts();
    assert!(counts.entity.exact > 0);
    assert!(counts.sidecar.embedded > 0);
    assert!(counts.outside.embedded > 0);
}

#[test]
fn names_are_reported_by_token_rules_but_moved_paths_are_not() {
    let fixture = fixture();
    write(
        &fixture.root,
        "docs/Acme.App.notes.md",
        "no reference here\n",
    );
    write(
        &fixture.root,
        "docs/Acme.App_notes.md",
        "no reference here either\n",
    );
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    let named: Vec<String> = planned
        .named
        .iter()
        .map(|path| relative(&fixture.root, path))
        .collect();
    assert!(named.contains(&"docs/Acme.App.notes.md".to_string()));
    assert!(!named.contains(&"docs/Acme.App_notes.md".to_string()));
    assert!(!named
        .iter()
        .any(|path| path == "Things/Acme.App.Manager.xml"));
}

#[test]
fn repository_content_moves_as_a_tree_without_being_scanned() {
    let fixture = fixture();
    write(
        &fixture.root,
        "filerepository/Acme.App.Manager/docs/Acme.App.Manager.notes.md",
        "Acme.App.Manager stays opaque here\n",
    );
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Entity, "Acme.App.Manager", "Acme.App.Director"),
    )
    .unwrap();
    let item = &planned.moves[0];
    assert_eq!(
        item.old_repo_files.as_deref(),
        Some(
            fixture
                .root
                .join("filerepository/Acme.App.Manager")
                .as_path()
        )
    );
    assert_eq!(
        item.new_repo_files.as_deref(),
        Some(
            fixture
                .root
                .join("filerepository/Acme.App.Director")
                .as_path()
        )
    );
    assert!(!planned
        .outside
        .iter()
        .any(|change| change.path.starts_with(fixture.root.join("filerepository"))));
    assert!(!planned
        .named
        .iter()
        .any(|path| path.starts_with(fixture.root.join("filerepository"))));

    let applied = apply(
        &fixture.solution,
        &planned,
        &options(false),
        &locked(&fixture),
    )
    .unwrap();
    assert!(applied
        .moved
        .iter()
        .any(|(_, new)| new == &fixture.root.join("filerepository/Acme.App.Director")));
    assert!(!fixture
        .root
        .join("filerepository/Acme.App.Manager")
        .exists());
    assert!(fixture
        .root
        .join("filerepository/Acme.App.Director/docs/Acme.App.Manager.notes.md")
        .exists());
}

#[test]
fn planning_writes_nothing() {
    let fixture = fixture();
    let before = snapshot(&fixture.root);
    plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn invalid_new_name_is_refused() {
    let fixture = fixture();
    assert!(matches!(
        plan(
            &fixture.solution,
            &spec(Kind::Entity, "Acme.App.Manager", "bad/name")
        ),
        Err(RenameError::InvalidNew { .. })
    ));
}

#[test]
fn invalid_or_empty_old_name_is_refused() {
    let fixture = fixture();
    assert!(matches!(
        plan(&fixture.solution, &spec(Kind::Entity, "", "Acme.New")),
        Err(RenameError::InvalidOld { .. })
    ));
}

#[test]
fn identical_names_are_refused() {
    let fixture = fixture();
    assert!(matches!(
        plan(
            &fixture.solution,
            &spec(Kind::Entity, "Acme.App.Manager", "Acme.App.Manager")
        ),
        Err(RenameError::Same { .. })
    ));
}

#[test]
fn nested_names_in_either_direction_are_refused() {
    let fixture = fixture();
    for (old, new) in [
        ("Acme.App", "Acme.App.New"),
        ("Acme.App.Manager", "Acme.App"),
    ] {
        assert!(matches!(
            plan(&fixture.solution, &spec(Kind::Prefix, old, new)),
            Err(RenameError::Nested { .. })
        ));
    }
}

#[test]
fn unknown_name_is_refused() {
    let fixture = fixture();
    assert!(matches!(
        plan(
            &fixture.solution,
            &spec(Kind::Prefix, "Missing", "Acme.New")
        ),
        Err(RenameError::Unknown { .. })
    ));
}

#[test]
fn entity_name_duplicated_across_collections_is_ambiguous() {
    let fixture = fixture();
    entity(&fixture.root, "DataShapes", "Acme.App.Manager", "Acme.App");
    assert!(matches!(
        plan(
            &fixture.solution,
            &spec(Kind::Entity, "Acme.App.Manager", "Acme.App.New")
        ),
        Err(RenameError::Ambiguous { .. })
    ));
}

#[test]
fn existing_entity_file_and_sidecar_targets_are_refused_and_named() {
    let fixture = fixture();
    entity(&fixture.root, "DataShapes", "Acme.Taken", "Acme.App");
    entity(&fixture.root, "Things", "Acme.Taken", "Acme.App");
    std::fs::create_dir_all(fixture.root.join("src/Acme.Taken")).unwrap();
    let error = plan(
        &fixture.solution,
        &spec(Kind::Entity, "Acme.App.Manager", "Acme.Taken"),
    )
    .unwrap_err();
    let RenameError::Exists { conflicts } = error else {
        panic!("wrong error: {error}")
    };
    let joined = conflicts.join("\n");
    assert!(joined.contains("entity Acme.Taken"));
    assert!(joined.contains("Things\\Acme.Taken.xml") || joined.contains("Things/Acme.Taken.xml"));
    assert!(joined.contains("src\\Acme.Taken") || joined.contains("src/Acme.Taken"));
}

#[test]
fn unreadable_discovery_candidates_are_refused() {
    let fixture = fixture();
    write(
        &fixture.root,
        "Things/broken.xml",
        "<Entities><Things><Thing",
    );
    assert!(matches!(
        plan(
            &fixture.solution,
            &spec(Kind::Entity, "Acme.App.Manager", "Acme.New")
        ),
        Err(RenameError::Unreadable { .. })
    ));
}

#[test]
fn unqualified_entity_keeps_unsafe_text_as_review_without_an_edit() {
    let fixture = fixture();
    let planned = plan(&fixture.solution, &spec(Kind::Entity, "T", "U")).unwrap();
    assert_eq!(planned.moves.len(), 1);
    let script = planned
        .changes
        .iter()
        .find(|change| {
            relative(&fixture.root, &change.path).ends_with("src/T/services/Run/script.js")
        })
        .unwrap();
    assert_eq!(script.findings.len(), 2);
    assert_eq!(script.edits.len(), 1);
    assert_eq!(script.findings[0].tier, refs::Tier::Exact);
    assert_eq!(script.findings[1].tier, refs::Tier::Review);
    assert!(!script.findings[1].applied);
}

#[test]
fn non_utf8_source_file_is_skipped() {
    let fixture = fixture();
    let path = fixture.root.join("src/Acme.App.Manager/binary.bin");
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Entity, "Acme.App.Manager", "Acme.App.Director"),
    )
    .unwrap();
    assert!(planned.skipped.contains(&path));
}

#[test]
fn counts_add_up_to_every_finding() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    let counts = planned.counts();
    let counted = [counts.entity, counts.sidecar, counts.config, counts.outside]
        .iter()
        .map(|count| count.exact + count.embedded + count.review)
        .sum::<usize>();
    let findings = planned
        .changes
        .iter()
        .chain(&planned.outside)
        .map(|change| change.findings.len())
        .sum::<usize>();
    assert_eq!(counted, findings);
    assert_eq!(
        counts.entity.files + counts.sidecar.files + counts.config.files + counts.outside.files,
        planned.changes.len() + planned.outside.len()
    );
}
