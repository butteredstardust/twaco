//! The command policy around one entity push.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::backup;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity_key::{EntityKey, KeyError};
use crate::core::{lock, profile, push, workspace};
use std::fmt;
use std::path::PathBuf;

/// The arguments that affect an entity push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushRequest {
    /// The entity spelling supplied by the caller.
    pub entity: String,
    pub mode: Mode,
    pub force: bool,
    pub backup: bool,
    pub profile: String,
}

/// One completed entity push, before either adapter projects it to its wire format.
#[derive(Debug)]
pub enum PushOutcome {
    Plan {
        entity: EntityKey,
        decision: push::Decision,
        effects: Effects,
    },
    Applied {
        entity: EntityKey,
        result: push::Outcome,
        backup: Option<String>,
        effects: Effects,
    },
}

impl PushOutcome {
    /// The access this specific outcome implies.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }
}

const PLAN_EFFECTS: Effects = Effects::new(Access::Read, Access::Read);
const ALREADY_THERE_EFFECTS: Effects = Effects::new(Access::Write, Access::Read);
const PUSHED_EFFECTS: Effects = Effects::new(Access::Write, Access::Write);

/// A remote that can both push an entity and save the current server copy before a forced push.
pub trait Remote: push::Remote + backup::Remote {}

impl<T: push::Remote + backup::Remote + ?Sized> Remote for T {}

/// A failure before a typed push outcome could be produced.
#[derive(Debug)]
pub enum PushCommandError {
    Lock(lock::LockError),
    Resolve(workspace::WorkspaceError),
    Unreadable(Vec<String>),
    Read {
        path: PathBuf,
        why: std::io::Error,
    },
    Profile(profile::ProfileError),
    Key {
        label: String,
        why: KeyError,
    },
    Backup {
        label: String,
        why: backup::BackupError,
    },
    Push {
        label: String,
        why: push::PushError,
        backup: Option<String>,
    },
}

impl PushCommandError {
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Key { label, .. } | Self::Backup { label, .. } | Self::Push { label, .. } => {
                Some(label)
            }
            Self::Lock(_)
            | Self::Resolve(_)
            | Self::Unreadable(_)
            | Self::Read { .. }
            | Self::Profile(_) => None,
        }
    }

    /// The backup already made before a later push failure, if there was one.
    pub fn backup(&self) -> Option<&str> {
        match self {
            Self::Push { backup, .. } => backup.as_deref(),
            _ => None,
        }
    }
}

impl fmt::Display for PushCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(why) => why.fmt(f),
            Self::Resolve(why) => why.fmt(f),
            Self::Unreadable(unreadable) => write!(f, "{}", unreadable.join("\n")),
            Self::Read { path, why } => write!(f, "{}: {why}", path.display()),
            Self::Profile(why) => why.fmt(f),
            Self::Key { label, why } => write!(f, "{label}: {why}"),
            Self::Backup { label, why } => write!(f, "{label}: {why}"),
            Self::Push { label, why, .. } => write!(f, "{label}: {why}"),
        }
    }
}

impl std::error::Error for PushCommandError {}

impl Coded for PushCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
            Self::Resolve(why) => why.code(),
            Self::Unreadable(_) => ErrorCode::InvalidData,
            Self::Read { .. } => ErrorCode::IoError,
            Self::Profile(why) => why.code(),
            Self::Key { .. } => ErrorCode::InvalidArguments,
            Self::Backup { why, .. } => why.code(),
            Self::Push { why, .. } => why.code(),
        }
    }
}

/// Execute one push. Applying takes the workspace lock before discovery because recording the
/// baseline is a workspace write; planning reads without taking it.
pub fn execute<R, F>(
    solution: &Solution,
    request: &PushRequest,
    open: F,
    notices: &mut Notices,
) -> Result<PushOutcome, PushCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => {
            Some(lock_workspace(solution, "entity push", notices).map_err(PushCommandError::Lock)?)
        }
    };
    let found = workspace::discover(solution);
    let entity =
        workspace::resolve(&found.entities, &request.entity).map_err(PushCommandError::Resolve)?;
    if !found.unreadable.is_empty() {
        return Err(PushCommandError::Unreadable(found.unreadable));
    }
    let bytes = std::fs::read(&entity.path).map_err(|why| PushCommandError::Read {
        path: entity.path.clone(),
        why,
    })?;
    let profile =
        profile::load(&solution.root, &request.profile).map_err(PushCommandError::Profile)?;
    let remote = open(profile);
    let label = format!("{}/{}", entity.info.collection, entity.info.name);
    let key = EntityKey::new(&entity.info.collection, &entity.info.name).map_err(|why| {
        PushCommandError::Key {
            label: label.clone(),
            why,
        }
    })?;
    let file_name = entity
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{}.xml", entity.info.name));
    let target = push::Target {
        key: key.clone(),
        document: push::EntityDocument {
            file_name: &file_name,
            bytes: &bytes,
        },
    };
    let backup = if matches!(request.mode, Mode::Apply) && request.force && request.backup {
        backup::before_forced_push(&remote, solution, &target, &backup::new_stamp()).map_err(
            |why| PushCommandError::Backup {
                label: label.clone(),
                why,
            },
        )?
    } else {
        None
    };
    let result = push::push(
        &remote,
        &solution.root,
        &target,
        matches!(request.mode, Mode::Apply),
        request.force,
    )
    .map_err(|why| PushCommandError::Push {
        label,
        why,
        backup: backup.clone(),
    })?;
    match result {
        push::Outcome::WouldDo(decision) => Ok(PushOutcome::Plan {
            entity: key,
            decision,
            effects: PLAN_EFFECTS,
        }),
        push::Outcome::AlreadyThere => Ok(PushOutcome::Applied {
            entity: key,
            result,
            backup,
            effects: ALREADY_THERE_EFFECTS,
        }),
        push::Outcome::Pushed { .. } => Ok(PushOutcome::Applied {
            entity: key,
            result,
            backup,
            effects: PUSHED_EFFECTS,
        }),
        push::Outcome::Refused(_) => Ok(PushOutcome::Applied {
            entity: key,
            result,
            backup,
            effects: PLAN_EFFECTS,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execute<R, F>(
        solution: &Solution,
        request: &PushRequest,
        open: F,
    ) -> Result<PushOutcome, PushCommandError>
    where
        R: Remote,
        F: FnOnce(profile::Profile) -> R,
    {
        super::execute(solution, request, open, &mut Notices::default())
    }

    use crate::core::baseline::Baseline;
    use crate::core::normalise;
    use crate::core::server::ServerError;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone)]
    struct Fake {
        state: Rc<RefCell<FakeState>>,
    }

    struct FakeState {
        held: Option<Vec<u8>>,
        imports: usize,
        exports: usize,
        fail_export: bool,
    }

    impl Fake {
        fn new(held: Option<Vec<u8>>) -> Self {
            Self {
                state: Rc::new(RefCell::new(FakeState {
                    held,
                    imports: 0,
                    exports: 0,
                    fail_export: false,
                })),
            }
        }

        fn imports(&self) -> usize {
            self.state.borrow().imports
        }

        fn exports(&self) -> usize {
            self.state.borrow().exports
        }
    }

    impl push::Remote for Fake {
        fn fetch(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(self.state.borrow().held.clone())
        }

        fn import(&self, _: &str, xml: &[u8]) -> Result<(), ServerError> {
            let mut state = self.state.borrow_mut();
            state.imports += 1;
            state.held = Some(xml.to_vec());
            Ok(())
        }
    }

    impl backup::Remote for Fake {
        fn export(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            let mut state = self.state.borrow_mut();
            state.exports += 1;
            if state.fail_export {
                return Err(ServerError::InvalidUrl("backup failed".to_string()));
            }
            Ok(state.held.clone())
        }

        fn import(&self, _: &str, xml: &[u8]) -> Result<(), ServerError> {
            push::Remote::import(self, "", xml)
        }

        fn exists(&self, _: &EntityKey) -> Result<bool, ServerError> {
            Ok(self.state.borrow().held.is_some())
        }
    }

    fn document(script: &str) -> Vec<u8> {
        format!(
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><code><![CDATA[{script}]]></code></Thing></Things></Entities>"
        )
        .into_bytes()
    }

    fn setup(
        server: Option<Vec<u8>>,
        baseline: Option<(&[u8], &[u8])>,
    ) -> (tempfile::TempDir, PathBuf, Solution, Fake) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-push-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        let working = document("working();");
        std::fs::write(root.join("Things/P.T.xml"), &working).unwrap();
        if let Some((local, remote)) = baseline {
            let mut stored = Baseline::load(&root).unwrap();
            stored.set(
                "Things",
                "P.T",
                normalise::hash(local).unwrap(),
                normalise::hash(remote).unwrap(),
            );
            stored.write(&root).unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution, Fake::new(server))
    }

    fn request(mode: Mode, force: bool, backup: bool) -> PushRequest {
        PushRequest {
            entity: "P.T".to_string(),
            mode,
            force,
            backup,
            profile: "default".to_string(),
        }
    }

    #[test]
    fn outcome_effects_match_the_push_result() {
        let key = EntityKey::new("Things", "P.T").unwrap();
        let plan = PushOutcome::Plan {
            entity: key.clone(),
            decision: push::Decision::Create,
            effects: PLAN_EFFECTS,
        };
        assert_eq!(plan.effects(), Effects::new(Access::Read, Access::Read));
        for (result, effects) in [
            (push::Outcome::AlreadyThere, ALREADY_THERE_EFFECTS),
            (push::Outcome::Pushed { created: false }, PUSHED_EFFECTS),
            (
                push::Outcome::Refused(push::Refusal::DeletedOnServer),
                PLAN_EFFECTS,
            ),
        ] {
            let outcome = PushOutcome::Applied {
                entity: key.clone(),
                result,
                backup: None,
                effects,
            };
            assert_eq!(outcome.effects(), effects);
        }
    }

    #[test]
    fn every_mode_force_backup_and_decision_combination_keeps_its_policy() {
        let working = document("working();");
        let old = document("old();");
        let changed = document("changed();");
        let cases = [
            (
                "already there",
                Some(working.clone()),
                None,
                push::Decision::AlreadyThere,
            ),
            ("create", None, None, push::Decision::Create),
            (
                "update",
                Some(old.clone()),
                Some((old.as_slice(), old.as_slice())),
                push::Decision::Update,
            ),
            (
                "deleted",
                None,
                Some((old.as_slice(), old.as_slice())),
                push::Decision::Refuse(push::Refusal::DeletedOnServer),
            ),
            (
                "unknown ancestor",
                Some(changed.clone()),
                None,
                push::Decision::Refuse(push::Refusal::UnknownAncestor {
                    server: normalise::hash(&changed).unwrap(),
                }),
            ),
            (
                "conflict",
                Some(changed.clone()),
                Some((old.as_slice(), old.as_slice())),
                push::Decision::Refuse(push::Refusal::Conflict {
                    server: normalise::hash(&changed).unwrap(),
                    baseline: normalise::hash(&old).unwrap(),
                }),
            ),
        ];
        for (name, server, baseline, expected) in cases {
            for mode in [Mode::Plan, Mode::Apply] {
                for force in [false, true] {
                    for backup in [false, true] {
                        let (_dir, _, solution, remote) = setup(server.clone(), baseline);
                        let outcome = execute(&solution, &request(mode, force, backup), {
                            let remote = remote.clone();
                            move |_| remote
                        })
                        .unwrap_or_else(|error| {
                            panic!("{name}: {mode:?}, {force}, {backup}: {error}")
                        });
                        match outcome {
                            PushOutcome::Plan { decision, .. } => {
                                assert_eq!(mode, Mode::Plan, "{name}");
                                assert_eq!(decision, expected, "{name}");
                                assert_eq!(remote.imports(), 0, "{name}");
                                assert_eq!(remote.exports(), 0, "{name}");
                            }
                            PushOutcome::Applied {
                                result,
                                backup: saved,
                                ..
                            } => {
                                assert_eq!(mode, Mode::Apply, "{name}");
                                let refused = matches!(expected, push::Decision::Refuse(_));
                                match result {
                                    push::Outcome::AlreadyThere => {
                                        assert_eq!(expected, push::Decision::AlreadyThere)
                                    }
                                    push::Outcome::Pushed { .. } => {
                                        assert!(!refused || force, "{name}")
                                    }
                                    push::Outcome::Refused(refusal) => {
                                        assert!(!force, "{name}");
                                        assert_eq!(expected, push::Decision::Refuse(refusal));
                                    }
                                    push::Outcome::WouldDo(_) => unreachable!(),
                                }
                                let imported = match expected {
                                    push::Decision::AlreadyThere => 0,
                                    push::Decision::Refuse(_) if !force => 0,
                                    _ => 1,
                                };
                                assert_eq!(remote.imports(), imported, "{name}");
                                let saved_backup = matches!(
                                    expected,
                                    push::Decision::Refuse(
                                        push::Refusal::UnknownAncestor { .. }
                                            | push::Refusal::Conflict { .. }
                                    )
                                ) && force
                                    && backup;
                                assert_eq!(remote.exports(), usize::from(saved_backup), "{name}");
                                assert_eq!(saved.is_some(), saved_backup, "{name}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_forced_backup_failure_pushes_nothing_and_no_backup_skips_it() {
        let old = document("old();");
        let changed = document("changed();");
        let (_dir, _, solution, remote) = setup(
            Some(changed.clone()),
            Some((old.as_slice(), old.as_slice())),
        );
        remote.state.borrow_mut().fail_export = true;
        let error = execute(&solution, &request(Mode::Apply, true, true), {
            let remote = remote.clone();
            move |_| remote
        })
        .unwrap_err();
        assert!(matches!(error, PushCommandError::Backup { .. }));
        assert_eq!(remote.imports(), 0);
        assert_eq!(remote.exports(), 1);

        let (_dir, _, solution, remote) =
            setup(Some(changed), Some((old.as_slice(), old.as_slice())));
        execute(&solution, &request(Mode::Apply, true, false), {
            let remote = remote.clone();
            move |_| remote
        })
        .unwrap();
        assert_eq!(remote.imports(), 1);
        assert_eq!(remote.exports(), 0);
    }

    #[test]
    fn applying_locks_before_discovery_and_planning_does_not_lock() {
        let (_dir, root, solution, remote) = setup(None, None);
        let held = lock::acquire(&root, "test holder", &[]).unwrap();
        let plan = execute(&solution, &request(Mode::Plan, false, true), {
            let remote = remote.clone();
            move |_| remote
        });
        assert!(matches!(plan, Ok(PushOutcome::Plan { .. })));
        let applied = execute(&solution, &request(Mode::Apply, false, true), move |_| {
            remote
        });
        assert!(matches!(applied, Err(PushCommandError::Lock(_))));
        drop(held);
    }

    #[test]
    fn command_errors_keep_the_wrapped_stable_code() {
        let remote = ServerError::InvalidUrl("bad URL".to_string());
        let error = PushCommandError::Push {
            label: "Things/P.T".to_string(),
            why: push::PushError::Remote(remote),
            backup: None,
        };
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        let error = PushCommandError::Push {
            label: "Things/P.T".to_string(),
            why: push::PushError::Unverified(ServerError::InvalidUrl("bad URL".to_string())),
            backup: None,
        };
        assert_eq!(error.code(), ErrorCode::NotVerified);
    }
}
