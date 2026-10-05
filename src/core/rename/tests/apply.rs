use super::*;

#[test]
fn apply_prefix_moves_and_edits_source_and_optionally_outside_text() {
    for include_outside in [false, true] {
        let fixture = fixture();
        let docs_before = std::fs::read(fixture.root.join("docs/GUIDE.md")).unwrap();
        let sql_before = std::fs::read(fixture.root.join("sql/m.sql")).unwrap();
        let planned = plan(
            &fixture.solution,
            &spec(Kind::Prefix, "Acme.App", "Acme.New"),
        )
        .unwrap();
        let applied = apply(&fixture.solution, &planned, &options(include_outside)).unwrap();
        assert_eq!(applied.moved.len(), planned.moves.len() + 2);
        for item in &planned.moves {
            assert!(!item.old_file.exists());
            assert!(item.new_file.exists());
            if let (Some(old), Some(new)) = (&item.old_sidecars, &item.new_sidecars) {
                assert!(!old.exists());
                assert!(new.exists());
            }
        }
        assert!(std::fs::read_to_string(fixture.root.join("twaco.toml"))
            .unwrap()
            .contains("Acme.New"));
        assert!(std::fs::read_to_string(
            fixture
                .root
                .join("src/Acme.New.Manager/services/Run/definition.xml")
        )
        .unwrap()
        .contains("Acme.New.Manager"));
        assert!(std::fs::read_to_string(
            fixture
                .root
                .join("src/Acme.New.Dashboard/mashup/content.json")
        )
        .unwrap()
        .contains("DynamicThingShapes_Acme.New.Manager"));
        if include_outside {
            assert!(std::fs::read_to_string(fixture.root.join("docs/GUIDE.md"))
                .unwrap()
                .contains("Acme.New"));
            // A SQL file documents names (a migration says old and new): --text never rewrites it.
            assert_eq!(
                std::fs::read_to_string(fixture.root.join("sql/m.sql")).unwrap(),
                "select 'Acme.App.Manager', 'Acme.App.Model_DS';
"
            );
        } else {
            assert_eq!(
                std::fs::read(fixture.root.join("docs/GUIDE.md")).unwrap(),
                docs_before
            );
            assert_eq!(
                std::fs::read(fixture.root.join("sql/m.sql")).unwrap(),
                sql_before
            );
        }
    }
}

#[test]
fn rename_and_reverse_restore_every_non_ledger_byte_including_bom_and_crlf() {
    let fixture = fixture();
    let entity_path = fixture.root.join("Things/Acme.App.Manager.xml");
    std::fs::write(
            &entity_path,
            b"\xef\xbb\xbf<Entities>\r\n<Things><Entity name=\"Acme.App.Manager\" projectName=\"Acme.App\"><Description>uses Acme.App.Manager</Description></Entity></Things>\r\n</Entities>\r\n",
        ).unwrap();
    let sidecar = fixture
        .root
        .join("src/Acme.App.Manager/services/Run/script.js");
    std::fs::write(
        &sidecar,
        b"\xef\xbb\xbfconst manager = Things[\"Acme.App.Manager\"];\r\n",
    )
    .unwrap();
    let before = snapshot_without_twaco(&fixture.root);

    let forward = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    apply(&fixture.solution, &forward, &options(true)).unwrap();
    let reverse = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.New", "Acme.App"),
    )
    .unwrap();
    apply(&fixture.solution, &reverse, &options(true)).unwrap();

    assert_eq!(snapshot_without_twaco(&fixture.root), before);
}

#[test]
fn applied_plan_is_idempotently_unknown_and_leaves_no_applicable_old_hit() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    apply(&fixture.solution, &planned, &options(true)).unwrap();
    assert!(matches!(
        plan(
            &fixture.solution,
            &spec(Kind::Prefix, "Acme.App", "Acme.New")
        ),
        Err(RenameError::Unknown { .. })
    ));
    for path in check::walk_files(&fixture.solution) {
        // SQL files are listed, never rewritten.
        if path.extension().is_some_and(|ext| ext == "sql") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let Ok(pass) = rename_scan::scan_text(&bytes, "Acme.App", refs::Mode::Prefix, "unused")
        else {
            continue;
        };
        assert!(
            pass.findings
                .iter()
                .all(|finding| { finding.tier == refs::Tier::Review || !finding.applied }),
            "applicable old hit remains in {}",
            path.display()
        );
    }
}

#[test]
fn stale_plan_and_new_target_are_refused_without_an_apply_write() {
    let first_fixture = fixture();
    let planned = plan(
        &first_fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    std::fs::write(
        first_fixture.root.join("docs/GUIDE.md"),
        "changed after planning\n",
    )
    .unwrap();
    let before = snapshot(&first_fixture.root);
    assert!(matches!(
        apply(&first_fixture.solution, &planned, &options(true)),
        Err(RenameError::Stale { .. })
    ));
    assert_eq!(snapshot(&first_fixture.root), before);

    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    std::fs::write(&planned.moves[0].new_file, "occupied").unwrap();
    let before = snapshot(&fixture.root);
    assert!(matches!(
        apply(&fixture.solution, &planned, &options(true)),
        Err(RenameError::Exists { .. })
    ));
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn every_injected_apply_failure_rolls_back_the_complete_tree() {
    let count_fixture = fixture();
    let count_plan = plan(
        &count_fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    seed_baseline(&count_fixture, &count_plan);
    let mut steps = Vec::new();
    apply_with(
        &count_fixture.solution,
        &count_plan,
        &options(true),
        &mut |step| {
            steps.push(step.clone());
            Ok(())
        },
    )
    .unwrap();
    assert!(steps.iter().any(|step| matches!(step, Step::Baseline)));
    assert!(steps.iter().any(|step| matches!(step, Step::Ledger)));

    for (fail_at, failed_step) in steps.iter().enumerate() {
        let fixture = fixture();
        let planned = plan(
            &fixture.solution,
            &spec(Kind::Prefix, "Acme.App", "Acme.New"),
        )
        .unwrap();
        seed_baseline(&fixture, &planned);
        let before = snapshot(&fixture.root);
        let mut index = 0usize;
        let error = apply_with(&fixture.solution, &planned, &options(true), &mut |_| {
            let current = index;
            index += 1;
            if current == fail_at {
                Err(std::io::Error::other("injected failure"))
            } else {
                Ok(())
            }
        })
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

#[test]
fn baseline_removes_only_renamed_entries_and_is_not_created_when_absent() {
    let baseline_fixture = fixture();
    let planned = plan(
        &baseline_fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    seed_baseline(&baseline_fixture, &planned);
    let applied = apply(&baseline_fixture.solution, &planned, &options(false)).unwrap();
    assert_eq!(applied.baseline_removed, planned.baseline_keys.len());
    let baseline = Baseline::load(&baseline_fixture.root).unwrap();
    assert!(baseline.get("Things", "Unchanged").is_some());
    for (collection, name) in &planned.baseline_keys {
        assert!(baseline.get(collection, name).is_none());
    }

    let fixture = fixture();
    let planned = plan(&fixture.solution, &spec(Kind::Entity, "T", "U")).unwrap();
    apply(&fixture.solution, &planned, &options(false)).unwrap();
    assert!(!fixture.root.join(BASELINE_PATH).exists());
}

#[test]
fn ledger_is_created_then_appended_and_corruption_is_a_preflight_refusal() {
    let ledger_fixture = fixture();
    let first = plan(
        &ledger_fixture.solution,
        &spec(Kind::Entity, "Acme.App.Manager", "Acme.App.Director"),
    )
    .unwrap();
    let first_applied = apply(&ledger_fixture.solution, &first, &options(false)).unwrap();
    let second = plan(&ledger_fixture.solution, &spec(Kind::Entity, "T", "U")).unwrap();
    apply(&ledger_fixture.solution, &second, &options(false)).unwrap();
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&first_applied.ledger).unwrap()).unwrap();
    let records = ledger.as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["date"], "2026-10-02");
    assert_eq!(records[0]["kind"], "entity");
    assert_eq!(records[0]["old"], "Acme.App.Manager");
    assert_eq!(records[0]["new"], "Acme.App.Director");
    assert_eq!(records[0]["entities"][0]["collection"], "Things");
    assert_eq!(records[1]["old"], "T");
    assert!(std::fs::read(&first_applied.ledger)
        .unwrap()
        .ends_with(b"\n"));

    let fixture = fixture();
    std::fs::create_dir_all(fixture.root.join(".twaco")).unwrap();
    std::fs::write(fixture.root.join(".twaco/renames.json"), "not json").unwrap();
    let planned = plan(&fixture.solution, &spec(Kind::Entity, "T", "U")).unwrap();
    let before = snapshot(&fixture.root);
    assert!(matches!(
        apply(&fixture.solution, &planned, &options(false)),
        Err(RenameError::InvalidLedger { .. })
    ));
    assert_eq!(snapshot(&fixture.root), before);
}
#[test]
fn a_ledger_holding_something_that_is_not_a_record_is_refused_before_any_write() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.root.join(".twaco")).unwrap();
    std::fs::write(fixture.root.join(".twaco/renames.json"), "[1, \"x\", null]").unwrap();
    let planned = plan(&fixture.solution, &spec(Kind::Entity, "T", "U")).unwrap();
    let before = snapshot(&fixture.root);
    assert!(matches!(
        apply(&fixture.solution, &planned, &options(false)),
        Err(RenameError::InvalidLedger { .. })
    ));
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn one_entity_renamed_twice_leaves_two_records_in_order() {
    let fixture = fixture();
    for (old, new) in [("T", "U"), ("U", "V")] {
        let planned = plan(&fixture.solution, &spec(Kind::Entity, old, new)).unwrap();
        apply(&fixture.solution, &planned, &options(false)).unwrap();
    }
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join(".twaco/renames.json")).unwrap())
            .unwrap();
    let olds: Vec<&str> = ledger
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["old"].as_str().unwrap())
        .collect();
    assert_eq!(olds, ["T", "U"]);
}

#[test]
fn with_src_at_the_root_only_an_entitys_own_folder_is_a_sidecar() {
    // `src = "."` makes the whole repository "below src": docs must still be outside text,
    // subject to --text, and a gitignored file must still be left alone.
    let fixture = fixture();
    write(
        &fixture.root,
        "twaco.toml",
        "[solution]\nsrc = \".\"\n[[project]]\nname = \"Acme.App\"\n",
    );
    // With `src = "."` an entity's sidecars sit at <root>/<entity>/, not under a src folder.
    write(
        &fixture.root,
        "Acme.App.Manager/services/Run/script.js",
        "const manager = Things[\"Acme.App.Manager\"];
",
    );
    let solution = Solution::load(&fixture.root.join(CONFIG_FILE)).unwrap();
    let planned = plan(
        &solution,
        &spec(Kind::Entity, "Acme.App.Manager", "Acme.App.Director"),
    )
    .unwrap();
    let kind_of = |name: &str| {
        planned
            .changes
            .iter()
            .chain(&planned.outside)
            .find(|c| c.path.ends_with(name))
            .map(|c| c.kind)
    };
    assert_eq!(kind_of("GUIDE.md"), Some(FileKind::Outside));
    assert_eq!(kind_of("m.sql"), Some(FileKind::Outside));
    assert_eq!(kind_of("script.js"), Some(FileKind::Sidecar));
    assert_eq!(kind_of("ignored.txt"), None);
    let before = std::fs::read_to_string(fixture.root.join("docs/GUIDE.md")).unwrap();
    apply(&solution, &planned, &options(false)).unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("docs/GUIDE.md")).unwrap(),
        before
    );
}

#[test]
fn an_entity_already_filed_under_its_new_name_is_edited_but_not_moved() {
    let fixture = fixture();
    // Old name in a file called for the new one: the move is a no-op, not a collision.
    std::fs::rename(
        fixture.root.join("Things/T.xml"),
        fixture.root.join("Things/U.xml"),
    )
    .unwrap();
    let solution = Solution::load(&fixture.root.join(CONFIG_FILE)).unwrap();
    let planned = plan(&solution, &spec(Kind::Entity, "T", "U")).unwrap();
    apply(&solution, &planned, &options(false)).unwrap();
    let text = std::fs::read_to_string(fixture.root.join("Things/U.xml")).unwrap();
    assert!(text.contains("name=\"U\""), "{text}");
}

#[test]
fn a_file_saved_between_the_check_and_its_write_is_refused_and_everything_is_restored() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    let before = snapshot(&fixture.root);
    let mut writes = 0usize;
    let victim = fixture.root.join("Things/Acme.App.Manager.xml");
    let error = apply_with(&fixture.solution, &planned, &options(true), &mut |step| {
        if let Step::Write(path) = step {
            writes += 1;
            // An editor saves the file just before twaco gets to it (and after the digest check).
            if path == &victim {
                std::fs::write(path, b"<edited-by-a-person/>").unwrap();
            }
        }
        Ok(())
    })
    .unwrap_err();
    assert!(writes >= 2);
    // The person's save survives; everything twaco had done before it is undone.
    assert!(matches!(error, RenameError::Stale { .. }), "{error}");
    let mut expected = before;
    expected.insert(
        PathBuf::from("Things/Acme.App.Manager.xml"),
        b"<edited-by-a-person/>".to_vec(),
    );
    assert_eq!(snapshot(&fixture.root), expected);
}

#[test]
fn a_save_made_after_twaco_wrote_a_file_is_not_overwritten_by_the_rollback() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    let victim = fixture.root.join("Things/Acme.App.Manager.xml");
    let moved = fixture.root.join("Things/Acme.New.Manager.xml");
    let error = apply_with(&fixture.solution, &planned, &options(true), &mut |step| {
        if matches!(step, Step::Ledger) {
            // Written and moved already; a person saves the file at its new path, and then the
            // ledger step fails.
            std::fs::write(&moved, b"<later-save/>").unwrap();
            return Err(std::io::Error::other("injected failure"));
        }
        Ok(())
    })
    .unwrap_err();
    match error {
        RenameError::RollbackFailed { leftover, .. } => {
            assert!(leftover.contains(&victim), "{leftover:?}");
        }
        other => panic!("expected the rollback to report the conflict, got {other}"),
    }
    assert_eq!(std::fs::read(&victim).unwrap(), b"<later-save/>");
}

#[test]
fn a_leftover_temporary_of_any_older_name_is_never_truncated() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Prefix, "Acme.App", "Acme.New"),
    )
    .unwrap();
    let target = fixture.root.join("docs/GUIDE.md");
    let squatters: Vec<PathBuf> = ["twaco-rename-tmp", "twaco-tmp"]
        .iter()
        .map(|suffix| target.with_file_name(format!(".GUIDE.md.{}.{suffix}", std::process::id())))
        .collect();
    for squatter in &squatters {
        std::fs::write(squatter, b"somebody else's file").unwrap();
    }
    apply(&fixture.solution, &planned, &options(true)).unwrap();
    for squatter in &squatters {
        assert_eq!(std::fs::read(squatter).unwrap(), b"somebody else's file");
    }
}

#[cfg(unix)]
#[test]
fn an_edited_file_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = fixture();
    let script = fixture
        .root
        .join("src/Acme.App.Manager/services/Run/script.js");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o750)).unwrap();
    let planned = plan(
        &fixture.solution,
        &spec(Kind::Entity, "Acme.App.Manager", "Acme.App.Director"),
    )
    .unwrap();
    apply(&fixture.solution, &planned, &options(false)).unwrap();
    let moved = fixture
        .root
        .join("src/Acme.App.Director/services/Run/script.js");
    assert_eq!(
        std::fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
        0o750
    );
}

#[test]
fn projects_that_share_a_root_do_not_duplicate_entities_in_a_plan() {
    let fixture = fixture();
    write(
        &fixture.root,
        "twaco.toml",
        "[[project]]\nname = \"Acme.App\"\n[[project]]\nname = \"Other\"\n",
    );
    let solution = Solution::load(&fixture.root.join(CONFIG_FILE)).unwrap();
    // Both projects look in the same folders, so discovery sees every file twice.
    assert!(workspace::discover(&solution).entities.len() > 7);
    let planned = plan(
        &solution,
        &spec(Kind::Entity, "Acme.App.Manager", "Acme.App.Director"),
    )
    .unwrap();
    assert_eq!(planned.moves.len(), 1);
    let entity_files = planned
        .changes
        .iter()
        .filter(|c| c.kind == FileKind::Entity)
        .count();
    let distinct: BTreeSet<&PathBuf> = planned.changes.iter().map(|c| &c.path).collect();
    assert_eq!(
        distinct.len(),
        planned.changes.len(),
        "a file is planned once"
    );
    assert!(entity_files >= 1);
    let applied = apply(&solution, &planned, &options(false)).unwrap();
    assert_eq!(
        applied.moved.len(),
        2,
        "one entity file and its sidecar folder"
    );
}
