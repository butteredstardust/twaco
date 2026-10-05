use super::*;
use crate::core::backup;
use crate::core::config::Solution;
use crate::core::entity_key::EntityKey;
use crate::core::server::ServerError;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

struct Fake {
    held: RefCell<BTreeSet<(String, String)>>,
    dependencies: BTreeMap<(String, String), Vec<Dependent>>,
    repository_things: BTreeSet<String>,
    calls: RefCell<Vec<String>>,
    keep_after_delete: BTreeSet<(String, String)>,
    backup_fails: bool,
}

impl Fake {
    fn new(held: &[(&str, &str)]) -> Self {
        Self {
            held: RefCell::new(
                held.iter()
                    .map(|(c, n)| ((*c).to_string(), (*n).to_string()))
                    .collect(),
            ),
            dependencies: BTreeMap::new(),
            repository_things: BTreeSet::new(),
            calls: RefCell::new(Vec::new()),
            keep_after_delete: BTreeSet::new(),
            backup_fails: false,
        }
    }
}

impl Remote for Fake {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        self.calls.borrow_mut().push(format!("GET {key}"));
        Ok(self
            .held
            .borrow()
            .contains(&(key.collection().to_string(), key.name().to_string())))
    }
    fn incoming(&self, key: &EntityKey) -> Result<Vec<Dependent>, ServerError> {
        self.calls.borrow_mut().push(format!("DEPS {key}"));
        Ok(self
            .dependencies
            .get(&(key.collection().to_string(), key.name().to_string()))
            .cloned()
            .unwrap_or_default())
    }
    fn backup(
        &self,
        _: &Solution,
        entities: &[EntityKey],
        stamp: &str,
    ) -> Result<Option<String>, backup::BackupError> {
        let names: Vec<&str> = entities.iter().map(EntityKey::name).collect();
        self.calls
            .borrow_mut()
            .push(format!("BACKUP {stamp} {}", names.join(",")));
        if self.backup_fails {
            return Err(backup::BackupError::Unreadable {
                entity: "Things/X".to_string(),
            });
        }
        Ok(Some(format!(".twaco/backups/{stamp}")))
    }
    fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
        self.calls.borrow_mut().push(format!("FETCH {key}"));
        let template = if self.repository_things.contains(key.name()) {
            "FileRepository"
        } else {
            "GenericThing"
        };
        Ok(format!("<Entities><Things><Thing name=\"{}\" thingTemplate=\"{template}\"/></Things></Entities>", key.name()).into_bytes())
    }
    fn delete_service(&self, service: &str, name: &str) -> Result<(), ServerError> {
        self.calls
            .borrow_mut()
            .push(format!("SERVICE {service} {name}"));
        let key = self
            .held
            .borrow()
            .iter()
            .find(|(_, held_name)| held_name == name)
            .cloned();
        if let Some(key) = key {
            if !self.keep_after_delete.contains(&key) {
                self.held.borrow_mut().remove(&key);
            }
        }
        Ok(())
    }
    fn delete_rest(&self, key: &EntityKey) -> Result<(), ServerError> {
        self.calls.borrow_mut().push(format!("DELETE {key}"));
        let held_key = (key.collection().to_string(), key.name().to_string());
        if !self.keep_after_delete.contains(&held_key) {
            self.held.borrow_mut().remove(&held_key);
        }
        Ok(())
    }
}

fn solution() -> (PathBuf, Solution) {
    let root = std::env::temp_dir().join(format!(
        "twaco-delete-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\nroot = \".\"\n",
    )
    .unwrap();
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    (root, solution)
}

fn execute(fake: &Fake, solution: &Solution, names: &[&str], apply: bool, force: bool) -> Report {
    execute_ack(
        fake,
        solution,
        names,
        apply,
        acknowledged(force, false, false, false).0,
    )
}

fn execute_ack(
    fake: &Fake,
    solution: &Solution,
    names: &[&str],
    apply: bool,
    acknowledged: Acknowledged,
) -> Report {
    let names = names
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    let prepared = prepare(solution, &names, false).unwrap();
    run(fake, solution, prepared, apply, acknowledged, "2026-10-02").unwrap()
}

#[test]
fn a_plan_sends_only_reads_and_reports_absent_entities() {
    let (root, solution) = solution();
    let fake = Fake::new(&[("Things", "T")]);
    let report = execute(
        &fake,
        &solution,
        &["Things/T", "Mashups/Missing"],
        false,
        false,
    );
    assert_eq!(
        report
            .entities
            .iter()
            .map(|e| &e.status)
            .collect::<Vec<_>>(),
        [&Status::Absent, &Status::Ready]
    );
    assert!(!fake
        .calls
        .borrow()
        .iter()
        .any(|call| call.starts_with("DELETE") || call.starts_with("SERVICE")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reverse_collection_order_deletes_dependents_first_and_uses_both_methods() {
    let (root, solution) = solution();
    let fake = Fake::new(&[("DataShapes", "D"), ("ThingShapes", "S"), ("Things", "T")]);
    let report = execute(
        &fake,
        &solution,
        &["DataShapes/D", "Things/T", "ThingShapes/S"],
        true,
        false,
    );
    assert!(report.entities.iter().all(|e| e.status == Status::Deleted));
    let deletes: Vec<String> = fake
        .calls
        .borrow()
        .iter()
        .filter(|call| call.starts_with("DELETE") || call.starts_with("SERVICE"))
        .cloned()
        .collect();
    assert_eq!(
        deletes,
        [
            "SERVICE DeleteThing T",
            "SERVICE DeleteThingShape S",
            "DELETE DataShapes/D"
        ]
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn outside_dependents_and_repository_definitions_refuse_unless_forced() {
    let (root, solution) = solution();
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::write(
        root.join("Things/T.xml"),
        "<Entities><Things><Thing name=\"T\" projectName=\"P\"/></Things></Entities>",
    )
    .unwrap();
    let mut fake = Fake::new(&[("Things", "T")]);
    fake.dependencies.insert(
        ("Things".into(), "T".into()),
        vec![Dependent {
            collection: "Mashups".into(),
            name: "M".into(),
        }],
    );
    let refused = execute(&fake, &solution, &["Things/T"], true, false);
    assert_eq!(refused.entities[0].status, Status::Refused);
    assert_eq!(refused.entities[0].refusals().count(), 2);
    let forced = execute(&fake, &solution, &["Things/T"], true, true);
    assert_eq!(forced.entities[0].status, Status::Deleted);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn acknowledgements_independently_guard_repository_dependents_and_file_data() {
    let (root, solution) = solution();
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::write(
        root.join("Things/Repo.xml"),
        "<Entities><Things><Thing name=\"Repo\" projectName=\"P\"/></Things></Entities>",
    )
    .unwrap();
    let mut fake = Fake::new(&[("Things", "Repo")]);
    fake.repository_things.insert("Repo".into());
    fake.dependencies.insert(
        ("Things".into(), "Repo".into()),
        vec![Dependent {
            collection: "Mashups".into(),
            name: "Outside".into(),
        }],
    );
    for repository_defined in [false, true] {
        for outside_dependents in [false, true] {
            for file_repository_data_loss in [false, true] {
                let report = execute_ack(
                    &fake,
                    &solution,
                    &["Things/Repo"],
                    false,
                    Acknowledged {
                        repository_defined,
                        outside_dependents,
                        file_repository_data_loss,
                    },
                );
                let entity = &report.entities[0];
                let expected = [
                    (!repository_defined).then_some(GuardCode::RepositoryDefined),
                    (!outside_dependents).then_some(GuardCode::OutsideDependents),
                    (!file_repository_data_loss).then_some(GuardCode::FileRepositoryDataLoss),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                assert_eq!(entity.refusal_codes().collect::<Vec<_>>(), expected);
                assert_eq!(
                    entity.status,
                    if expected.is_empty() {
                        Status::Ready
                    } else {
                        Status::Refused
                    }
                );
                assert_eq!(
                    entity.warnings.len(),
                    usize::from(file_repository_data_loss)
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn legacy_force_acknowledges_only_its_two_original_guards() {
    assert_eq!(
        acknowledged(false, false, false, false),
        (Acknowledged::default(), false)
    );
    assert_eq!(
        acknowledged(true, false, false, false),
        (
            Acknowledged {
                repository_defined: true,
                outside_dependents: true,
                file_repository_data_loss: false,
            },
            true,
        )
    );
    assert_eq!(
        acknowledged(true, false, false, true),
        (
            Acknowledged {
                repository_defined: true,
                outside_dependents: true,
                file_repository_data_loss: true,
            },
            true,
        )
    );
    assert_eq!(
        acknowledged(false, true, false, true),
        (
            Acknowledged {
                repository_defined: true,
                outside_dependents: false,
                file_repository_data_loss: true,
            },
            false,
        )
    );
    assert_eq!(
        acknowledged(false, false, true, false),
        (
            Acknowledged {
                repository_defined: false,
                outside_dependents: true,
                file_repository_data_loss: false,
            },
            false,
        )
    );
}

#[test]
fn refusals_serialize_as_parallel_messages_and_codes() {
    let (root, solution) = solution();
    let fake = Fake::new(&[("Unknowns", "T")]);
    let prepared = prepare(&solution, &["Unknowns/T".into()], false).unwrap();
    let refused = run(
        &fake,
        &solution,
        prepared,
        false,
        Acknowledged::default(),
        "2026-10-02",
    )
    .unwrap();
    let value = serde_json::to_value(&refused.entities[0]).unwrap();
    assert_eq!(
        value["refusals"],
        serde_json::json!([
            "twaco has no delete method for this collection; delete it in Composer"
        ])
    );
    assert_eq!(
        value["refusal_codes"],
        serde_json::json!(["no_delete_method"])
    );

    let prepared = prepare(&solution, &["Unknowns/T".into()], false).unwrap();
    let still_refused = run(
        &fake,
        &solution,
        prepared,
        false,
        Acknowledged {
            repository_defined: true,
            outside_dependents: true,
            file_repository_data_loss: true,
        },
        "2026-10-02",
    )
    .unwrap();
    assert_eq!(still_refused.entities[0].status, Status::Refused);
    assert_eq!(
        still_refused.entities[0]
            .refusal_codes()
            .collect::<Vec<_>>(),
        [GuardCode::NoDeleteMethod]
    );

    let absent = execute(&fake, &solution, &["Things/Missing"], false, false);
    let value = serde_json::to_value(&absent.entities[0]).unwrap();
    assert!(value.get("refusals").is_none());
    assert!(value.get("refusal_codes").is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn invalid_qualified_names_keep_the_target_error() {
    let (root, solution) = solution();
    let fake = Fake::new(&[]);
    for requested in ["Things/..", "Things/A/B", "/X"] {
        let prepared = prepare(&solution, &[requested.to_string()], false).unwrap();
        let error = run(
            &fake,
            &solution,
            prepared,
            false,
            Acknowledged::default(),
            "2026-10-02",
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            format!("{requested:?} must be Collection/Name or a bare server entity name")
        );
    }
    assert!(fake.calls.borrow().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_invalid_ledger_entity_key_is_reported_without_a_request() {
    let (root, solution) = solution();
    std::fs::create_dir_all(root.join(".twaco")).unwrap();
    std::fs::write(
            root.join(".twaco/renames.json"),
            r#"[{"date":"2026-10-01","kind":"entity","old":"Bad","new":"New","entities":[{"collection":"Things","old":"..","new":"New"}]}]"#,
        )
        .unwrap();
    let fake = Fake::new(&[]);
    let prepared = prepare(&solution, &[], true).unwrap();
    let error = run(
        &fake,
        &solution,
        prepared,
        false,
        Acknowledged::default(),
        "2026-10-02",
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        error,
        format!(
            "{}: invalid rename ledger: Things/.. is not a valid entity key",
            root.join(".twaco/renames.json").display()
        )
    );
    assert!(fake.calls.borrow().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_invalid_dependent_key_is_outside_the_delete_set() {
    let (root, solution) = solution();
    let mut fake = Fake::new(&[("Things", "T")]);
    fake.dependencies.insert(
        ("Things".into(), "T".into()),
        vec![Dependent {
            collection: "Mashups".into(),
            name: "..".into(),
        }],
    );
    let report = execute(&fake, &solution, &["Things/T"], false, false);
    let entity = &report.entities[0];
    assert_eq!(entity.status, Status::Refused);
    assert_eq!(
        entity.refusal_codes().collect::<Vec<_>>(),
        [GuardCode::OutsideDependents]
    );
    assert_eq!(
            entity.refusals().collect::<Vec<_>>(),
            ["incoming dependents outside this delete set: Mashups/.. (pass --allow-outside-dependents)"]
        );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn refused_and_deleted_entity_results_keep_their_json_shape() {
    let (root, solution) = solution();
    let fake = Fake::new(&[("Unknowns", "T"), ("Mashups", "M")]);
    let refused = execute(&fake, &solution, &["Unknowns/T"], false, false);
    assert_eq!(
        serde_json::to_value(&refused.entities[0]).unwrap(),
        serde_json::json!({
            "collection": "Unknowns",
            "name": "T",
            "status": "refused",
            "method": "delete it in Composer",
            "dependents": [],
            "warnings": [],
            "refusals": ["twaco has no delete method for this collection; delete it in Composer"],
            "refusal_codes": ["no_delete_method"]
        })
    );
    let deleted = execute(&fake, &solution, &["Mashups/M"], true, false);
    assert_eq!(
        serde_json::to_value(&deleted.entities[0]).unwrap(),
        serde_json::json!({
            "collection": "Mashups",
            "name": "M",
            "status": "deleted",
            "method": "REST DELETE",
            "dependents": [],
            "warnings": []
        })
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_dependent_inside_the_set_is_allowed_and_repository_files_are_warned() {
    let (root, solution) = solution();
    let mut fake = Fake::new(&[("ThingTemplates", "Base"), ("Things", "Repo")]);
    fake.dependencies.insert(
        ("ThingTemplates".into(), "Base".into()),
        vec![Dependent {
            collection: "Things".into(),
            name: "Repo".into(),
        }],
    );
    fake.repository_things.insert("Repo".into());
    let report = execute_ack(
        &fake,
        &solution,
        &["ThingTemplates/Base", "Things/Repo"],
        false,
        Acknowledged {
            file_repository_data_loss: true,
            ..Acknowledged::default()
        },
    );
    assert!(report
        .entities
        .iter()
        .all(|entity| entity.status == Status::Ready));
    assert!(report
        .entities
        .iter()
        .find(|entity| entity.name == "Repo")
        .unwrap()
        .warnings[0]
        .contains("deletes all"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_success_response_that_did_not_delete_is_a_per_entity_failure_and_the_run_continues() {
    let (root, solution) = solution();
    let mut fake = Fake::new(&[("Mashups", "A"), ("Mashups", "B")]);
    fake.keep_after_delete
        .insert(("Mashups".into(), "A".into()));
    let report = execute(&fake, &solution, &["Mashups/A", "Mashups/B"], true, false);
    assert!(report.failed());
    assert_eq!(
        report
            .entities
            .iter()
            .find(|e| e.name == "A")
            .unwrap()
            .status,
        Status::Failed
    );
    assert_eq!(
        report
            .entities
            .iter()
            .find(|e| e.name == "B")
            .unwrap()
            .status,
        Status::Deleted
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn renamed_marks_deleted_and_absent_entries_and_skips_already_marked_and_member_records() {
    let (root, solution) = solution();
    std::fs::create_dir_all(root.join(".twaco")).unwrap();
    let ledger = serde_json::json!([
        {"date":"2026-10-01","kind":"entity","old":"Old","new":"New","entities":[
            {"collection":"Things","old":"Old","new":"New"},
            {"collection":"Mashups","old":"Gone","new":"NewMashup","deleted":"2026-10-01"}
        ]},
        {"date":"2026-10-01","kind":"field","old":"a","new":"b","entities":[
            {"collection":"DataShapes","old":"D","new":"D"}
        ]}
    ]);
    std::fs::write(
        root.join(".twaco/renames.json"),
        serde_json::to_vec_pretty(&ledger).unwrap(),
    )
    .unwrap();
    let fake = Fake::new(&[("Things", "Old")]);
    let prepared = prepare(&solution, &[], true).unwrap();
    let report = run(
        &fake,
        &solution,
        prepared,
        true,
        Acknowledged::default(),
        "2026-10-02",
    )
    .unwrap();
    assert_eq!(report.entities.len(), 1);
    assert!(report.ledger_changed);
    let updated: Value =
        serde_json::from_slice(&std::fs::read(root.join(".twaco/renames.json")).unwrap()).unwrap();
    assert_eq!(updated[0]["entities"][0]["deleted"], "2026-10-02");
    assert_eq!(updated[0]["entities"][1]["deleted"], "2026-10-01");
    assert!(updated[1]["entities"][0].get("deleted").is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_delete_set_is_backed_up_before_the_first_delete_and_a_failed_backup_deletes_nothing() {
    let (root, solution) = solution();
    let fake = Fake::new(&[("Things", "A"), ("Things", "B")]);
    let names = vec![
        "Things/A".to_string(),
        "Things/B".to_string(),
        "Things/Absent".to_string(),
    ];
    let prepared = prepare(&solution, &names, false)
        .unwrap()
        .with_backup("20261002-1");
    let report = run(
        &fake,
        &solution,
        prepared,
        true,
        Acknowledged::default(),
        "2026-10-02",
    )
    .unwrap();
    assert_eq!(report.backup.as_deref(), Some(".twaco/backups/20261002-1"));
    let calls = fake.calls.borrow();
    let backup_at = calls
        .iter()
        .position(|call| call.starts_with("BACKUP"))
        .unwrap();
    let first_delete = calls
        .iter()
        .position(|call| call.starts_with("SERVICE"))
        .unwrap();
    assert!(backup_at < first_delete, "{calls:?}");
    assert_eq!(
        calls[backup_at], "BACKUP 20261002-1 A,B",
        "only what will be deleted is saved"
    );
    drop(calls);
    // A plan takes no backup, and --no-backup (no stamp) takes none.
    let plan_fake = Fake::new(&[("Things", "A")]);
    let planned = prepare(&solution, &["Things/A".to_string()], false)
        .unwrap()
        .with_backup("s");
    run(
        &plan_fake,
        &solution,
        planned,
        false,
        Acknowledged::default(),
        "d",
    )
    .unwrap();
    let unbacked = Fake::new(&[("Things", "A")]);
    execute(&unbacked, &solution, &["Things/A"], true, false);
    assert!(plan_fake
        .calls
        .borrow()
        .iter()
        .chain(unbacked.calls.borrow().iter())
        .all(|call| !call.starts_with("BACKUP")));
    // A backup that fails stops everything before any delete.
    let mut failing = Fake::new(&[("Things", "A")]);
    failing.backup_fails = true;
    let prepared = prepare(&solution, &["Things/A".to_string()], false)
        .unwrap()
        .with_backup("s");
    let error = run(
        &failing,
        &solution,
        prepared,
        true,
        Acknowledged::default(),
        "d",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("nothing was deleted") && error.contains("--no-backup"),
        "{error}"
    );
    assert!(failing
        .calls
        .borrow()
        .iter()
        .all(|call| !call.starts_with("SERVICE") && !call.starts_with("DELETE")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_entity_is_deleted_after_the_entities_of_the_set_that_depend_on_it() {
    let (root, solution) = solution();
    let mut fake = Fake::new(&[
        ("ThingTemplates", "A_TT"),
        ("ThingTemplates", "B_TT"),
        ("ThingTemplates", "C_TT"),
    ]);
    // B and C inherit from A: both must go first, though A sorts before them by name.
    let dependent = |name: &str| Dependent {
        collection: "ThingTemplates".to_string(),
        name: name.to_string(),
    };
    fake.dependencies.insert(
        ("ThingTemplates".to_string(), "A_TT".to_string()),
        vec![dependent("B_TT"), dependent("C_TT")],
    );
    let report = execute(
        &fake,
        &solution,
        &[
            "ThingTemplates/A_TT",
            "ThingTemplates/B_TT",
            "ThingTemplates/C_TT",
        ],
        true,
        false,
    );
    let order: Vec<&str> = report
        .entities
        .iter()
        .map(|entity| entity.name.as_str())
        .collect();
    assert_eq!(order, ["B_TT", "C_TT", "A_TT"]);
    assert!(
        report
            .entities
            .iter()
            .all(|entity| entity.status == Status::Deleted),
        "{:?}",
        report
            .entities
            .iter()
            .map(|e| (&e.name, &e.status))
            .collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn corrupt_ledger_and_unknown_collection_refuse_before_any_request() {
    let (root, solution) = solution();
    std::fs::create_dir_all(root.join(".twaco")).unwrap();
    std::fs::write(root.join(".twaco/renames.json"), "not json").unwrap();
    let fake = Fake::new(&[]);
    assert!(prepare(&solution, &["Things/T".into()], false).is_err());
    assert!(fake.calls.borrow().is_empty());
    std::fs::write(root.join(".twaco/renames.json"), "[]").unwrap();
    let prepared = prepare(&solution, &["Unknowns/T".into()], false).unwrap();
    let report = run(
        &fake,
        &solution,
        prepared,
        false,
        Acknowledged::default(),
        "2026-10-02",
    )
    .unwrap();
    assert_eq!(report.entities[0].status, Status::Refused);
    assert_eq!(report.entities[0].method, Method::Composer);
    assert!(fake.calls.borrow().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_collection_without_a_delete_method_is_refused_whatever_is_acknowledged() {
    let (root, solution) = solution();
    let fake = Fake::new(&[]);
    let everything = Acknowledged {
        repository_defined: true,
        outside_dependents: true,
        file_repository_data_loss: true,
    };
    let report = execute_ack(&fake, &solution, &["Unknowns/T"], true, everything);
    let entity = &report.entities[0];
    assert_eq!(entity.status, Status::Refused);
    assert_eq!(
        entity.refusal_codes().collect::<Vec<_>>(),
        [GuardCode::NoDeleteMethod]
    );
    assert!(
        fake.calls.borrow().is_empty(),
        "nothing is asked of the server for it"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_bare_name_resolves_on_the_server_and_ambiguity_lists_its_collections() {
    let (root, solution) = solution();
    let one = Fake::new(&[("Mashups", "Shared")]);
    let report = execute(&one, &solution, &["Shared"], false, false);
    assert_eq!(
        (
            report.entities[0].collection.as_str(),
            report.entities[0].name.as_str()
        ),
        ("Mashups", "Shared")
    );

    let several = Fake::new(&[("Mashups", "Shared"), ("Things", "Shared")]);
    let prepared = prepare(&solution, &["Shared".into()], false).unwrap();
    let error = run(
        &several,
        &solution,
        prepared,
        false,
        Acknowledged::default(),
        "2026-10-02",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("ambiguous") && error.contains("Mashups") && error.contains("Things"),
        "{error}"
    );
    assert!(!several
        .calls
        .borrow()
        .iter()
        .any(|call| call.starts_with("DELETE") || call.starts_with("SERVICE")));
    let _ = std::fs::remove_dir_all(root);
}
