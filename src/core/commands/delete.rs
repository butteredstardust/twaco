//! The command policy around guarded entity deletion.

use super::{Access, Effects, Mode};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{entity_delete, lock, profile};
use std::fmt;

/// The arguments that affect guarded entity deletion.
#[derive(Clone, Debug)]
pub struct EntityDeleteRequest {
    /// Entity spellings supplied by the caller.
    pub entities: Vec<String>,
    pub renamed: bool,
    pub mode: Mode,
    pub acknowledgements: entity_delete::Acknowledged,
    /// Whether the deprecated `--force` spelling supplied the acknowledgements.
    pub legacy_force_used: bool,
    pub backup: bool,
    pub profile: String,
}

/// One completed guarded entity deletion, before either adapter projects it to its wire format.
#[derive(Debug)]
pub enum EntityDeleteOutcome {
    Plan {
        report: entity_delete::Report,
        effects: Effects,
        legacy_force_used: bool,
    },
    Applied {
        report: entity_delete::Report,
        date: String,
        effects: Effects,
        legacy_force_used: bool,
    },
}

impl EntityDeleteOutcome {
    /// The access this specific outcome implies.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }

    pub const fn legacy_force_used(&self) -> bool {
        match self {
            Self::Plan {
                legacy_force_used, ..
            }
            | Self::Applied {
                legacy_force_used, ..
            } => *legacy_force_used,
        }
    }
}

const READ_EFFECTS: Effects = Effects::new(Access::Read, Access::Read);

/// A remote that can perform every server operation used by guarded entity deletion.
pub trait Remote: entity_delete::Remote {}

impl<T: entity_delete::Remote + ?Sized> Remote for T {}

/// A failure before a typed delete outcome could be produced.
#[derive(Debug)]
pub enum EntityDeleteCommandError {
    Lock(lock::LockError),
    Profile(profile::ProfileError),
    Delete(entity_delete::DeleteError),
}

impl fmt::Display for EntityDeleteCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(why) => why.fmt(f),
            Self::Profile(why) => why.fmt(f),
            Self::Delete(why) => why.fmt(f),
        }
    }
}

impl std::error::Error for EntityDeleteCommandError {}

impl Coded for EntityDeleteCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
            Self::Profile(why) => why.code(),
            Self::Delete(why) => why.code(),
        }
    }
}

/// Execute one guarded entity deletion. A lock is necessary only when the prepared ledger can be
/// changed. Once it is held, preparation runs again so an intervening ledger update is observed.
pub fn execute<R, F>(
    solution: &Solution,
    request: &EntityDeleteRequest,
    open: F,
) -> Result<EntityDeleteOutcome, EntityDeleteCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    execute_after_lock(solution, request, open, || {})
}

fn execute_after_lock<R, F, H>(
    solution: &Solution,
    request: &EntityDeleteRequest,
    open: F,
    after_lock: H,
) -> Result<EntityDeleteOutcome, EntityDeleteCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
    H: FnOnce(),
{
    let mut prepared = entity_delete::prepare(solution, &request.entities, request.renamed)
        .map_err(EntityDeleteCommandError::Delete)?;
    let _lock = if prepared.ledger_will_be_written(matches!(request.mode, Mode::Apply)) {
        let lock =
            lock::acquire_for(solution, "entity delete").map_err(EntityDeleteCommandError::Lock)?;
        after_lock();
        prepared = entity_delete::prepare(solution, &request.entities, request.renamed)
            .map_err(EntityDeleteCommandError::Delete)?;
        Some(lock)
    } else {
        None
    };
    let profile = profile::load(&solution.root, &request.profile)
        .map_err(EntityDeleteCommandError::Profile)?;
    let date = jiff::Zoned::now().strftime("%Y-%m-%d").to_string();
    if request.backup {
        prepared = prepared.with_backup(&crate::core::backup::new_stamp());
    }
    let report = entity_delete::run(
        &open(profile),
        solution,
        prepared,
        matches!(request.mode, Mode::Apply),
        request.acknowledgements,
        &date,
    )
    .map_err(EntityDeleteCommandError::Delete)?;
    let effects = effects(&report);
    Ok(match request.mode {
        Mode::Plan => EntityDeleteOutcome::Plan {
            report,
            effects,
            legacy_force_used: request.legacy_force_used,
        },
        Mode::Apply => EntityDeleteOutcome::Applied {
            report,
            date,
            effects,
            legacy_force_used: request.legacy_force_used,
        },
    })
}

fn effects(report: &entity_delete::Report) -> Effects {
    if !report.applied {
        return READ_EFFECTS;
    }
    let server = if report.entities.iter().any(|entity| {
        matches!(
            entity.status,
            entity_delete::Status::Deleted | entity_delete::Status::Failed
        )
    }) {
        Access::Write
    } else {
        Access::Read
    };
    let workspace = if report.ledger_changed {
        Access::Write
    } else {
        Access::Read
    };
    Effects::new(workspace, server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::backup;
    use crate::core::entity_key::EntityKey;
    use crate::core::server::ServerError;
    use serde_json::json;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Clone)]
    struct Fake {
        state: Rc<RefCell<FakeState>>,
    }

    struct FakeState {
        held: BTreeSet<(String, String)>,
        dependents: BTreeMap<(String, String), Vec<entity_delete::Dependent>>,
        events: Vec<String>,
        fail_backup: bool,
        failed_deletes: BTreeSet<(String, String)>,
        file_repositories: BTreeSet<(String, String)>,
    }

    impl Fake {
        fn new(held: &[(&str, &str)]) -> Self {
            Self {
                state: Rc::new(RefCell::new(FakeState {
                    held: held
                        .iter()
                        .map(|(collection, name)| ((*collection).to_string(), (*name).to_string()))
                        .collect(),
                    dependents: BTreeMap::new(),
                    events: Vec::new(),
                    fail_backup: false,
                    failed_deletes: BTreeSet::new(),
                    file_repositories: BTreeSet::new(),
                })),
            }
        }

        fn events(&self) -> Vec<String> {
            self.state.borrow().events.clone()
        }
    }

    impl entity_delete::Remote for Fake {
        fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
            Ok(self
                .state
                .borrow()
                .held
                .contains(&(key.collection().to_string(), key.name().to_string())))
        }

        fn incoming(&self, key: &EntityKey) -> Result<Vec<entity_delete::Dependent>, ServerError> {
            Ok(self
                .state
                .borrow()
                .dependents
                .get(&(key.collection().to_string(), key.name().to_string()))
                .cloned()
                .unwrap_or_default())
        }

        fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
            let file_repository = self
                .state
                .borrow()
                .file_repositories
                .contains(&(key.collection().to_string(), key.name().to_string()));
            let template = if file_repository {
                "FileRepository"
            } else {
                "GenericThing"
            };
            Ok(format!(
                "<Entities><Things><Thing name=\"{}\" thingTemplate=\"{template}\"/></Things></Entities>",
                key.name()
            )
            .into_bytes())
        }

        fn delete_service(&self, service: &str, name: &str) -> Result<(), ServerError> {
            let key = self
                .state
                .borrow()
                .held
                .iter()
                .find(|(_, held_name)| held_name == name)
                .cloned()
                .expect("the delete target exists");
            self.delete(key, format!("SERVICE {service} {name}"))
        }

        fn delete_rest(&self, key: &EntityKey) -> Result<(), ServerError> {
            self.delete(
                (key.collection().to_string(), key.name().to_string()),
                format!("DELETE {key}"),
            )
        }

        fn backup(
            &self,
            _: &Solution,
            entities: &[EntityKey],
            _: &str,
        ) -> Result<Option<String>, backup::BackupError> {
            let mut state = self.state.borrow_mut();
            state.events.push(format!(
                "BACKUP {}",
                entities
                    .iter()
                    .map(EntityKey::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
            if state.fail_backup {
                return Err(backup::BackupError::Unreadable {
                    entity: "Things/A".to_string(),
                });
            }
            Ok(Some(".twaco/backups/test".to_string()))
        }
    }

    impl Fake {
        fn delete(&self, key: (String, String), event: String) -> Result<(), ServerError> {
            let mut state = self.state.borrow_mut();
            state.events.push(event);
            if state.failed_deletes.contains(&key) {
                return Err(ServerError::InvalidUrl("delete failed".to_string()));
            }
            state.held.remove(&key);
            Ok(())
        }
    }

    fn root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "twaco-command-delete-{}-{nonce}",
            std::process::id()
        ))
    }

    fn setup(ledger: Option<serde_json::Value>) -> (PathBuf, Solution) {
        let root = root();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\nroot = \".\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        if let Some(ledger) = ledger {
            std::fs::write(
                root.join(".twaco/renames.json"),
                serde_json::to_vec_pretty(&ledger).unwrap(),
            )
            .unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    fn pending_ledger(deleted: Option<&str>) -> serde_json::Value {
        let mut entity = json!({ "collection": "Things", "old": "Old", "new": "New" });
        if let Some(deleted) = deleted {
            entity["deleted"] = json!(deleted);
        }
        json!([{
            "date": "2026-10-01",
            "kind": "entity",
            "old": "Old",
            "new": "New",
            "entities": [entity]
        }])
    }

    fn request(mode: Mode, entities: &[&str], renamed: bool, backup: bool) -> EntityDeleteRequest {
        EntityDeleteRequest {
            entities: entities
                .iter()
                .map(|entity| (*entity).to_string())
                .collect(),
            renamed,
            mode,
            acknowledgements: entity_delete::Acknowledged::default(),
            legacy_force_used: false,
            backup,
            profile: "default".to_string(),
        }
    }

    #[test]
    fn plans_and_server_only_applies_do_not_take_the_workspace_lock() {
        let (root, solution) = setup(None);
        let held = lock::acquire(&root, "test holder", &[]).unwrap();
        let plan_remote = Fake::new(&[("Mashups", "M")]);
        let plan = execute(
            &solution,
            &request(Mode::Plan, &["Mashups/M"], false, false),
            {
                let remote = plan_remote.clone();
                move |_| remote
            },
        )
        .unwrap();
        assert_eq!(plan.effects(), Effects::new(Access::Read, Access::Read));
        let apply_remote = Fake::new(&[("Mashups", "M")]);
        let applied = execute(
            &solution,
            &request(Mode::Apply, &["Mashups/M"], false, false),
            {
                let remote = apply_remote.clone();
                move |_| remote
            },
        )
        .unwrap();
        assert_eq!(applied.effects(), Effects::new(Access::Read, Access::Write));
        drop(held);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_apply_that_can_mark_the_ledger_takes_the_workspace_lock() {
        let (root, solution) = setup(Some(pending_ledger(None)));
        let held = lock::acquire(&root, "test holder", &[]).unwrap();
        let error = execute(&solution, &request(Mode::Apply, &[], true, false), |_| {
            Fake::new(&[("Things", "Old")])
        })
        .unwrap_err();
        assert!(matches!(error, EntityDeleteCommandError::Lock(_)));
        drop(held);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preparing_again_after_the_lock_uses_the_current_ledger() {
        let (root, solution) = setup(Some(pending_ledger(None)));
        let remote = Fake::new(&[("Things", "Old")]);
        let ledger_path = root.join(".twaco/renames.json");
        let outcome = execute_after_lock(
            &solution,
            &request(Mode::Apply, &[], true, false),
            {
                let remote = remote.clone();
                move |_| remote
            },
            || {
                std::fs::write(
                    &ledger_path,
                    serde_json::to_vec_pretty(&pending_ledger(Some("2026-10-02"))).unwrap(),
                )
                .unwrap();
            },
        )
        .unwrap();
        let EntityDeleteOutcome::Applied { report, .. } = outcome else {
            panic!("apply has an applied outcome");
        };
        assert!(report.entities.is_empty());
        assert!(!report.ledger_changed);
        assert!(remote.events().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backups_finish_before_the_first_delete_and_a_failed_backup_deletes_nothing() {
        let (root, solution) = setup(None);
        let remote = Fake::new(&[("Mashups", "A"), ("Mashups", "B")]);
        execute(
            &solution,
            &request(Mode::Apply, &["Mashups/A", "Mashups/B"], false, true),
            {
                let remote = remote.clone();
                move |_| remote
            },
        )
        .unwrap();
        let events = remote.events();
        assert!(events
            .first()
            .is_some_and(|event| event.starts_with("BACKUP ")));
        assert!(events
            .iter()
            .skip(1)
            .all(|event| event.starts_with("DELETE ")));
        std::fs::remove_dir_all(&root).unwrap();

        let (root, solution) = setup(None);
        let remote = Fake::new(&[("Mashups", "A")]);
        remote.state.borrow_mut().fail_backup = true;
        let error = execute(
            &solution,
            &request(Mode::Apply, &["Mashups/A"], false, true),
            {
                let remote = remote.clone();
                move |_| remote
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            EntityDeleteCommandError::Delete(entity_delete::DeleteError::Backup(_))
        ));
        assert!(remote
            .events()
            .iter()
            .all(|event| event.starts_with("BACKUP ")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn partial_applies_keep_the_report_and_every_refusal_code() {
        let (root, solution) = setup(None);
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(
            root.join("Things/T.xml"),
            "<Entities><Things><Thing name=\"T\" projectName=\"P\"/></Things></Entities>",
        )
        .unwrap();
        let remote = Fake::new(&[
            ("Mashups", "A"),
            ("Mashups", "B"),
            ("Things", "T"),
            ("Unknowns", "U"),
        ]);
        {
            let mut state = remote.state.borrow_mut();
            state
                .failed_deletes
                .insert(("Mashups".to_string(), "B".to_string()));
            state
                .file_repositories
                .insert(("Things".to_string(), "T".to_string()));
            state.dependents.insert(
                ("Things".to_string(), "T".to_string()),
                vec![entity_delete::Dependent {
                    collection: "Mashups".to_string(),
                    name: "Outside".to_string(),
                }],
            );
        }
        let outcome = execute(
            &solution,
            &request(
                Mode::Apply,
                &["Mashups/A", "Mashups/B", "Things/T", "Unknowns/U"],
                false,
                false,
            ),
            {
                let remote = remote.clone();
                move |_| remote
            },
        )
        .unwrap();
        let EntityDeleteOutcome::Applied {
            report, effects, ..
        } = outcome
        else {
            panic!("apply has an applied outcome");
        };
        assert!(report.failed());
        assert_eq!(effects, Effects::new(Access::Read, Access::Write));
        let codes = report
            .entities
            .iter()
            .flat_map(|entity| entity.refusal_codes())
            .collect::<Vec<_>>();
        for code in [
            entity_delete::GuardCode::RepositoryDefined,
            entity_delete::GuardCode::OutsideDependents,
            entity_delete::GuardCode::FileRepositoryDataLoss,
            entity_delete::GuardCode::NoDeleteMethod,
        ] {
            assert!(codes.contains(&code));
        }
        let wire = serde_json::to_value(&report.entities).unwrap();
        for code in codes {
            assert!(wire.to_string().contains(code.as_str()));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn command_errors_keep_the_wrapped_stable_code() {
        let error = EntityDeleteCommandError::Lock(lock::LockError::Held {
            holder: "other command".to_string(),
        });
        assert_eq!(error.code(), ErrorCode::WorkspaceLocked);
        let error = EntityDeleteCommandError::Profile(profile::ProfileError::Missing {
            name: "default".to_string(),
            searched: Vec::new(),
        });
        assert_eq!(error.code(), ErrorCode::InvalidData);
        let error = EntityDeleteCommandError::Delete(entity_delete::DeleteError::Target(
            "bad target".to_string(),
        ));
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        let error = EntityDeleteCommandError::Delete(entity_delete::DeleteError::Remote {
            entity: "Things/T".to_string(),
            why: ServerError::InvalidUrl("bad URL".to_string()),
        });
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        let error = EntityDeleteCommandError::Delete(entity_delete::DeleteError::Backup(
            "backup failed".to_string(),
        ));
        assert_eq!(error.code(), ErrorCode::IoError);
    }
}
