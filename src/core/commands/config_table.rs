//! The command policy around one live configuration table.

use super::{Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{config_table, profile};
use std::fmt;
use std::path::PathBuf;

/// The requested configuration-table operation.
#[derive(Clone, Debug)]
pub enum ConfigTableAction {
    Read,
    Backup { path: PathBuf },
    Restore { path: PathBuf, mode: Mode },
    Diff { entity: PathBuf },
}

/// The arguments that affect a configuration-table operation.
#[derive(Clone, Debug)]
pub struct ConfigTableRequest {
    pub thing: String,
    pub table: String,
    pub action: ConfigTableAction,
    pub profile: String,
}

/// The completed configuration-table operation.
#[derive(Debug)]
pub enum ConfigTableOutcome {
    Read {
        table: config_table::Table,
        effects: Effects,
    },
    BackedUp {
        table: config_table::Table,
        effects: Effects,
    },
    Restored {
        plan: config_table::RestorePlan,
        effects: Effects,
    },
    Diffed {
        table: config_table::Table,
        differences: Vec<String>,
        effects: Effects,
    },
}

impl ConfigTableOutcome {
    /// The access this operation used or may have used.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Read { effects, .. }
            | Self::BackedUp { effects, .. }
            | Self::Restored { effects, .. }
            | Self::Diffed { effects, .. } => *effects,
        }
    }
}

/// A remote that can inspect and restore configuration tables.
pub trait Remote: config_table::Remote {}

impl<T: config_table::Remote + ?Sized> Remote for T {}

/// A failure before a typed configuration-table outcome could be produced.
#[derive(Debug)]
pub enum ConfigTableCommandError {
    Profile(profile::ProfileError),
    Backup(config_table::TableError),
    Table(config_table::TableError),
    Read { path: PathBuf, why: std::io::Error },
}

impl fmt::Display for ConfigTableCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile(error) => error.fmt(f),
            Self::Backup(error) => error.fmt(f),
            Self::Table(error) => error.fmt(f),
            Self::Read { path, why } => write!(f, "{}: {why}", path.display()),
        }
    }
}

impl std::error::Error for ConfigTableCommandError {}

impl Coded for ConfigTableCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(error) => error.code(),
            Self::Backup(error) => error.code(),
            Self::Table(error) => error.code(),
            Self::Read { .. } => ErrorCode::IoError,
        }
    }
}

/// Read, back up, compare, or plan or restore a configuration table. The only local write is a
/// caller-named backup, so this operation never owns the workspace lock.
pub fn execute<R, F>(
    solution: &Solution,
    request: &ConfigTableRequest,
    open: F,
    _: &mut Notices,
) -> Result<ConfigTableOutcome, ConfigTableCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile = profile::load(&solution.root, &request.profile)
        .map_err(ConfigTableCommandError::Profile)?;
    let remote = open(profile);
    match &request.action {
        ConfigTableAction::Restore { path, mode } => {
            let saved = config_table::read_backup(path, &request.thing, &request.table)
                .map_err(ConfigTableCommandError::Backup)?;
            let apply = matches!(mode, Mode::Apply);
            let plan =
                config_table::restore(&remote, &request.thing, &request.table, &saved, apply)
                    .map_err(ConfigTableCommandError::Table)?;
            Ok(ConfigTableOutcome::Restored {
                plan,
                effects: Effects::new(
                    Access::Read,
                    if apply { Access::Write } else { Access::Read },
                ),
            })
        }
        ConfigTableAction::Read
        | ConfigTableAction::Backup { .. }
        | ConfigTableAction::Diff { .. } => {
            let table = config_table::fetch(&remote, &request.thing, &request.table)
                .map_err(ConfigTableCommandError::Table)?;
            match &request.action {
                ConfigTableAction::Read => Ok(ConfigTableOutcome::Read {
                    table,
                    effects: Effects::new(Access::None, Access::Read),
                }),
                ConfigTableAction::Backup { path } => {
                    config_table::write_backup(path, &request.thing, &request.table, &table)
                        .map_err(ConfigTableCommandError::Table)?;
                    Ok(ConfigTableOutcome::BackedUp {
                        table,
                        effects: Effects::new(Access::Write, Access::Read),
                    })
                }
                ConfigTableAction::Diff { entity } => {
                    let source =
                        std::fs::read(entity).map_err(|why| ConfigTableCommandError::Read {
                            path: entity.clone(),
                            why,
                        })?;
                    let rows = config_table::repository_rows(&source, &request.table)
                        .map_err(ConfigTableCommandError::Table)?;
                    let key = config_table::primary_key(&table.data_shape);
                    let differences = config_table::differences(
                        "server",
                        &table.rows,
                        "source control",
                        &rows,
                        &key,
                    );
                    Ok(ConfigTableOutcome::Diffed {
                        table,
                        differences,
                        effects: Effects::new(Access::Read, Access::Read),
                    })
                }
                ConfigTableAction::Restore { .. } => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity_key::ServiceTarget;
    use crate::core::server::ServerError;
    use serde_json::{json, Value};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A remote that serves one table and records every service it is asked to run.
    #[derive(Clone)]
    struct Recorder {
        rows: Rc<RefCell<Vec<Value>>>,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl config_table::Remote for Recorder {
        fn call(
            &self,
            _: &ServiceTarget,
            service: &str,
            _: &Value,
        ) -> Result<Option<Value>, ServerError> {
            self.calls.borrow_mut().push(service.to_string());
            Ok(Some(json!({
                "dataShape": { "fieldDefinitions": {
                    "Key": { "name": "Key", "aspects": { "isPrimaryKey": true } },
                    "Value": { "name": "Value", "aspects": {} }
                } },
                "rows": self.rows.borrow().clone()
            })))
        }
    }

    fn workspace() -> (tempfile::TempDir, std::path::PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-config-table-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution)
    }

    fn request(action: ConfigTableAction) -> ConfigTableRequest {
        ConfigTableRequest {
            thing: "P.Thing".to_string(),
            table: "Settings".to_string(),
            action,
            profile: "default".to_string(),
        }
    }

    fn run(
        solution: &Solution,
        remote: &Recorder,
        action: ConfigTableAction,
    ) -> Result<ConfigTableOutcome, ConfigTableCommandError> {
        let remote = remote.clone();
        execute(
            solution,
            &request(action),
            move |_| remote,
            &mut Notices::default(),
        )
    }

    #[test]
    fn reading_and_backing_up_report_their_effects_and_only_a_backup_writes_a_file() {
        let (_dir, root, solution) = workspace();
        let remote = Recorder {
            rows: Rc::new(RefCell::new(vec![json!({ "Key": "a", "Value": "1" })])),
            calls: Rc::default(),
        };
        let read = run(&solution, &remote, ConfigTableAction::Read).unwrap();
        assert_eq!(read.effects(), Effects::new(Access::None, Access::Read));
        let path = root.join("backup.json");
        assert!(!path.exists());
        let saved = run(
            &solution,
            &remote,
            ConfigTableAction::Backup { path: path.clone() },
        )
        .unwrap();
        assert_eq!(saved.effects(), Effects::new(Access::Write, Access::Read));
        assert!(path.exists());
        assert!(remote
            .calls
            .borrow()
            .iter()
            .all(|call| call == "GetConfigurationTable"));
    }

    #[test]
    fn a_restore_plan_changes_nothing_on_the_server_and_an_apply_does() {
        let (_dir, root, solution) = workspace();
        let remote = Recorder {
            rows: Rc::new(RefCell::new(vec![json!({ "Key": "a", "Value": "1" })])),
            calls: Rc::default(),
        };
        let path = root.join("backup.json");
        run(
            &solution,
            &remote,
            ConfigTableAction::Backup { path: path.clone() },
        )
        .unwrap();
        // The server's table moves on; the backup holds the earlier rows.
        *remote.rows.borrow_mut() = vec![json!({ "Key": "a", "Value": "2" })];
        remote.calls.borrow_mut().clear();
        let plan = run(
            &solution,
            &remote,
            ConfigTableAction::Restore {
                path: path.clone(),
                mode: Mode::Plan,
            },
        )
        .unwrap();
        assert_eq!(plan.effects(), Effects::new(Access::Read, Access::Read));
        assert!(
            remote
                .calls
                .borrow()
                .iter()
                .all(|call| call == "GetConfigurationTable"),
            "{:?}",
            remote.calls.borrow()
        );
        remote.calls.borrow_mut().clear();
        // This remote does not keep what it is sent, so the read-back after the write differs
        // from the backup and the restore is refused: the apply wrote, then checked.
        let applied = run(
            &solution,
            &remote,
            ConfigTableAction::Restore {
                path,
                mode: Mode::Apply,
            },
        )
        .unwrap_err();
        assert!(
            matches!(
                applied,
                ConfigTableCommandError::Table(config_table::TableError::NotRestored(_))
            ),
            "{applied}"
        );
        assert!(
            remote
                .calls
                .borrow()
                .iter()
                .any(|call| call == "SetConfigurationTableRows"),
            "{:?}",
            remote.calls.borrow()
        );
    }

    #[test]
    fn a_missing_profile_is_refused_before_any_call_with_its_own_code() {
        let (_dir, _, solution) = workspace();
        let remote = Recorder {
            rows: Rc::default(),
            calls: Rc::default(),
        };
        let mut missing = request(ConfigTableAction::Read);
        missing.profile = "nope".to_string();
        let handle = remote.clone();
        let error = execute(
            &solution,
            &missing,
            move |_| handle,
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(
            matches!(error, ConfigTableCommandError::Profile(_)),
            "{error}"
        );
        assert!(remote.calls.borrow().is_empty());
        let _ = error.code();
    }
}
