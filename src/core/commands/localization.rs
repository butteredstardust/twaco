//! Command policy for localization tables.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::localization::{self, Remote};
use crate::core::lock;
use crate::core::profile;
use crate::core::progress::{self, Progress};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalizationAction {
    Status {
        table: Option<String>,
    },
    Pull {
        table: Option<String>,
        prune: bool,
        mode: Mode,
    },
    Push {
        table: Option<String>,
        prune: bool,
        mode: Mode,
    },
    New {
        table: String,
        project: Option<String>,
        header: localization::Header,
        mode: Mode,
    },
    Set {
        name: String,
        value: String,
        table: Option<String>,
        usage: Option<String>,
        context: Option<String>,
        project: Option<String>,
        mode: Mode,
    },
    Remove {
        name: String,
        table: Option<String>,
        mode: Mode,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalizationRequest {
    pub action: LocalizationAction,
    pub profile: String,
}

#[derive(Debug)]
pub enum LocalizationOutcome {
    Status {
        status: localization::Status,
        effects: Effects,
    },
    Pulled {
        pulled: localization::Pulled,
        effects: Effects,
    },
    Pushed {
        pushed: localization::Pushed,
        effects: Effects,
    },
    Edited {
        edited: localization::Edited,
        effects: Effects,
    },
}

impl LocalizationOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Status { effects, .. }
            | Self::Pulled { effects, .. }
            | Self::Pushed { effects, .. }
            | Self::Edited { effects, .. } => *effects,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LocalizationCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Localization(localization::LocalizationError),
}
impl Coded for LocalizationCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Profile(error) => error.code(),
            Self::Localization(error) => error.code(),
        }
    }
}

/// Execute a localization action with no progress reporter.
pub fn execute<R, F>(
    solution: &Solution,
    request: &LocalizationRequest,
    open: F,
    notices: &mut Notices,
) -> Result<LocalizationOutcome, LocalizationCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    execute_with_progress(solution, request, open, notices, &progress::NONE)
}

/// Execute a localization action, taking a workspace lock before an applied local mutation.
pub fn execute_with_progress<R, F>(
    solution: &Solution,
    request: &LocalizationRequest,
    open: F,
    notices: &mut Notices,
    progress: &dyn Progress,
) -> Result<LocalizationOutcome, LocalizationCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let lock_label = match &request.action {
        LocalizationAction::Pull {
            mode: Mode::Apply, ..
        } => Some("localization pull"),
        LocalizationAction::New {
            mode: Mode::Apply, ..
        } => Some("localization new"),
        LocalizationAction::Set {
            mode: Mode::Apply, ..
        } => Some("localization set"),
        LocalizationAction::Remove {
            mode: Mode::Apply, ..
        } => Some("localization remove"),
        _ => None,
    };
    let _lock = lock_label
        .map(|label| lock_workspace(solution, label, notices))
        .transpose()
        .map_err(LocalizationCommandError::Lock)?;
    let mut open = Some(open);
    let remote = |open: &mut Option<F>| -> Result<R, LocalizationCommandError> {
        let profile = profile::load(&solution.root, &request.profile)
            .map_err(LocalizationCommandError::Profile)?;
        Ok(open.take().expect("remote opened once")(profile))
    };
    match &request.action {
        LocalizationAction::Status { table } => {
            let remote = remote(&mut open)?;
            let _phase = progress::phase(progress, "reading tables", None);
            let status = localization::status(solution, &remote, table.as_deref())
                .map_err(LocalizationCommandError::Localization)?;
            Ok(LocalizationOutcome::Status {
                status,
                effects: Effects::new(Access::Read, Access::Read),
            })
        }
        LocalizationAction::Pull { table, prune, mode } => {
            let remote = remote(&mut open)?;
            let _phase = progress::phase(progress, "reading tables", None);
            let pulled = localization::pull(
                solution,
                &remote,
                table.as_deref(),
                *prune,
                *mode == Mode::Apply,
            )
            .map_err(LocalizationCommandError::Localization)?;
            Ok(LocalizationOutcome::Pulled {
                pulled,
                effects: Effects::new(
                    if *mode == Mode::Apply {
                        Access::Write
                    } else {
                        Access::Read
                    },
                    Access::Read,
                ),
            })
        }
        LocalizationAction::Push { table, prune, mode } => {
            let remote = remote(&mut open)?;
            let pushed = localization::push(
                solution,
                &remote,
                table.as_deref(),
                *prune,
                *mode == Mode::Apply,
                progress,
            )
            .map_err(LocalizationCommandError::Localization)?;
            Ok(LocalizationOutcome::Pushed {
                pushed,
                effects: Effects::new(
                    Access::Read,
                    if *mode == Mode::Apply {
                        Access::Write
                    } else {
                        Access::Read
                    },
                ),
            })
        }
        LocalizationAction::New {
            table,
            project,
            header,
            mode,
        } => {
            let edited = localization::new(
                solution,
                table,
                project.as_deref(),
                header.clone(),
                *mode == Mode::Apply,
            )
            .map_err(LocalizationCommandError::Localization)?;
            Ok(LocalizationOutcome::Edited {
                edited,
                effects: Effects::new(
                    if *mode == Mode::Apply {
                        Access::Write
                    } else {
                        Access::Read
                    },
                    Access::None,
                ),
            })
        }
        LocalizationAction::Set {
            name,
            value,
            table,
            usage,
            context,
            project,
            mode,
        } => {
            let edited = localization::set(
                solution,
                name,
                value,
                table.as_deref(),
                usage.as_deref(),
                context.as_deref(),
                project.as_deref(),
                *mode == Mode::Apply,
            )
            .map_err(LocalizationCommandError::Localization)?;
            Ok(LocalizationOutcome::Edited {
                edited,
                effects: Effects::new(
                    if *mode == Mode::Apply {
                        Access::Write
                    } else {
                        Access::Read
                    },
                    Access::None,
                ),
            })
        }
        LocalizationAction::Remove { name, table, mode } => {
            let edited =
                localization::remove(solution, name, table.as_deref(), *mode == Mode::Apply)
                    .map_err(LocalizationCommandError::Localization)?;
            Ok(LocalizationOutcome::Edited {
                edited,
                effects: Effects::new(
                    if *mode == Mode::Apply {
                        Access::Write
                    } else {
                        Access::Read
                    },
                    Access::None,
                ),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity_key::EntityKey;
    use crate::core::imports;
    use crate::core::server::ServerError;

    struct Never;
    impl imports::Remote for Never {
        fn exists(&self, _: &EntityKey) -> Result<bool, ServerError> {
            unreachable!()
        }
        fn import_file(&self, _: &str, _: &[u8], _: bool, _: bool) -> Result<(), ServerError> {
            unreachable!()
        }
        fn source_control(
            &self,
            _: &str,
            _: &serde_json::Value,
        ) -> Result<Option<serde_json::Value>, ServerError> {
            unreachable!()
        }
    }
    impl localization::Remote for Never {
        fn tables(&self) -> Result<Vec<String>, ServerError> {
            unreachable!()
        }
        fn tokens(&self, _: &str) -> Result<Vec<localization::Token>, ServerError> {
            unreachable!()
        }
        fn header(&self, _: &str) -> Result<localization::Header, ServerError> {
            unreachable!()
        }
        fn delete_token(&self, _: &str, _: &str) -> Result<(), ServerError> {
            unreachable!()
        }
    }

    /// A server with no localization tables at all.
    struct Empty;
    impl imports::Remote for Empty {
        fn exists(&self, _: &EntityKey) -> Result<bool, ServerError> {
            Ok(false)
        }
        fn import_file(&self, _: &str, _: &[u8], _: bool, _: bool) -> Result<(), ServerError> {
            unreachable!()
        }
        fn source_control(
            &self,
            _: &str,
            _: &serde_json::Value,
        ) -> Result<Option<serde_json::Value>, ServerError> {
            unreachable!()
        }
    }
    impl localization::Remote for Empty {
        fn tables(&self) -> Result<Vec<String>, ServerError> {
            Ok(Vec::new())
        }
        fn tokens(&self, _: &str) -> Result<Vec<localization::Token>, ServerError> {
            unreachable!()
        }
        fn header(&self, _: &str) -> Result<localization::Header, ServerError> {
            unreachable!()
        }
        fn delete_token(&self, _: &str, _: &str) -> Result<(), ServerError> {
            unreachable!()
        }
    }

    fn solution(profile: bool) -> (tempfile::TempDir, Solution) {
        let temp = tempfile::Builder::new()
            .prefix("twaco-command-localization-")
            .tempdir()
            .unwrap();
        std::fs::write(
            temp.path().join("twaco.toml"),
            "[[project]]
name = \"P\"
",
        )
        .unwrap();
        if profile {
            std::fs::create_dir_all(temp.path().join(".twaco/profiles")).unwrap();
            std::fs::write(
                temp.path().join(".twaco/profiles/default.toml"),
                "url = \"http://example.invalid/Thingworx/\"
username = \"u\"
password = \"p\"
",
            )
            .unwrap();
        }
        let solution = Solution::load(&temp.path().join("twaco.toml")).unwrap();
        (temp, solution)
    }

    fn every_action(mode: Mode) -> Vec<LocalizationAction> {
        vec![
            LocalizationAction::Status { table: None },
            LocalizationAction::Pull {
                table: None,
                prune: false,
                mode,
            },
            LocalizationAction::Push {
                table: None,
                prune: false,
                mode,
            },
            LocalizationAction::New {
                table: "de".to_string(),
                project: None,
                header: localization::Header::default(),
                mode,
            },
            LocalizationAction::Set {
                name: "P.Title".to_string(),
                value: "Title".to_string(),
                table: None,
                usage: None,
                context: None,
                project: None,
                mode,
            },
        ]
    }

    #[test]
    fn only_applied_workspace_writes_take_the_lock() {
        let (_temp, solution) = solution(true);
        let held = lock::acquire_for(&solution, "holder").unwrap();
        let run = |action| {
            execute(
                &solution,
                &LocalizationRequest {
                    action,
                    profile: "default".to_string(),
                },
                |_| Empty,
                &mut Notices::default(),
            )
        };
        let locked = |result: Result<LocalizationOutcome, LocalizationCommandError>| {
            matches!(result, Err(LocalizationCommandError::Lock(_)))
        };
        for action in every_action(Mode::Plan) {
            assert!(!locked(run(action.clone())), "{action:?}");
        }
        let applied: Vec<bool> = every_action(Mode::Apply)
            .into_iter()
            .map(|action| locked(run(action)))
            .collect();
        // Status, pull, push, new, set.
        assert_eq!(applied, [false, true, false, true, true]);
        assert!(locked(run(LocalizationAction::Remove {
            name: "P.Title".to_string(),
            table: None,
            mode: Mode::Apply,
        })));
        drop(held);
    }

    #[test]
    fn effects_say_what_each_action_touched() {
        let (_temp, solution) = solution(true);
        let effects = |action| {
            execute(
                &solution,
                &LocalizationRequest {
                    action,
                    profile: "default".to_string(),
                },
                |_| Empty,
                &mut Notices::default(),
            )
            .unwrap()
            .effects()
        };
        let plans: Vec<Effects> = every_action(Mode::Plan).into_iter().map(effects).collect();
        assert_eq!(
            plans,
            [
                Effects::new(Access::Read, Access::Read),
                Effects::new(Access::Read, Access::Read),
                Effects::new(Access::Read, Access::Read),
                Effects::new(Access::Read, Access::None),
                Effects::new(Access::Read, Access::None),
            ]
        );
        let applies: Vec<Effects> = every_action(Mode::Apply).into_iter().map(effects).collect();
        assert_eq!(
            applies,
            [
                Effects::new(Access::Read, Access::Read),
                Effects::new(Access::Write, Access::Read),
                Effects::new(Access::Read, Access::Write),
                Effects::new(Access::Write, Access::None),
                Effects::new(Access::Write, Access::None),
            ]
        );
    }

    #[test]
    fn server_actions_need_the_profile_and_local_ones_never_open_a_remote() {
        let (_temp, solution) = solution(false);
        for action in every_action(Mode::Plan) {
            let server = matches!(
                action,
                LocalizationAction::Status { .. }
                    | LocalizationAction::Pull { .. }
                    | LocalizationAction::Push { .. }
            );
            let result = execute(
                &solution,
                &LocalizationRequest {
                    action: action.clone(),
                    profile: "missing".to_string(),
                },
                |_| -> Never { panic!("opened a remote for {action:?}") },
                &mut Notices::default(),
            );
            if server {
                assert!(
                    matches!(result, Err(LocalizationCommandError::Profile(_))),
                    "{action:?}"
                );
            } else {
                result.unwrap();
            }
        }
    }
}
