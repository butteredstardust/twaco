//! The command policy around restoring one backup set.

use super::{Access, Effects, Mode, Notices};
use crate::core::backup;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::profile;
use std::fmt;

/// The arguments that affect an entity restore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreRequest {
    pub set: Option<String>,
    pub only: Vec<String>,
    pub mode: Mode,
    pub profile: String,
}

/// One completed backup listing or restore, before either adapter projects it to its wire format.
#[derive(Debug)]
pub enum RestoreOutcome {
    Sets {
        sets: Vec<backup::Set>,
        effects: Effects,
    },
    Plan {
        set: backup::Set,
        entities: Vec<backup::Restored>,
        effects: Effects,
    },
    Applied {
        set: backup::Set,
        entities: Vec<backup::Restored>,
        effects: Effects,
    },
}

impl RestoreOutcome {
    /// The access this specific outcome implies.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Sets { effects, .. }
            | Self::Plan { effects, .. }
            | Self::Applied { effects, .. } => *effects,
        }
    }
}

/// A remote that can import and confirm a backup set.
pub trait Remote: backup::Remote {}

impl<T: backup::Remote + ?Sized> Remote for T {}

/// A failure before a typed restore outcome could be produced.
#[derive(Debug)]
pub enum RestoreCommandError {
    Profile(profile::ProfileError),
    Restore(backup::BackupError),
}

impl fmt::Display for RestoreCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile(error) => error.fmt(f),
            Self::Restore(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RestoreCommandError {}

impl Coded for RestoreCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(error) => error.code(),
            Self::Restore(error) => error.code(),
        }
    }
}

/// List backup sets, or plan or apply importing one set. Listing needs neither a profile nor a
/// remote; restoring only changes the server, so it never takes the workspace lock.
pub fn execute<R, F>(
    solution: &Solution,
    request: &RestoreRequest,
    open: F,
    _: &mut Notices,
) -> Result<RestoreOutcome, RestoreCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let Some(id) = &request.set else {
        return Ok(RestoreOutcome::Sets {
            sets: backup::list(solution),
            effects: Effects::new(Access::Read, Access::None),
        });
    };
    let set = backup::find(solution, id).map_err(RestoreCommandError::Restore)?;
    let profile =
        profile::load(&solution.root, &request.profile).map_err(RestoreCommandError::Profile)?;
    let entities = backup::restore(
        &open(profile),
        &set,
        &request.only,
        matches!(request.mode, Mode::Apply),
    )
    .map_err(RestoreCommandError::Restore)?;
    Ok(match request.mode {
        Mode::Plan => RestoreOutcome::Plan {
            set,
            entities,
            effects: Effects::new(Access::Read, Access::Read),
        },
        Mode::Apply => RestoreOutcome::Applied {
            set,
            entities,
            effects: Effects::new(Access::Read, Access::Write),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity_key::EntityKey;
    use crate::core::server::ServerError;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::rc::Rc;

    type Entities = BTreeMap<(String, String), Vec<u8>>;

    #[derive(Clone)]
    struct Fake {
        entities: Rc<RefCell<Entities>>,
        imports: Rc<RefCell<Vec<String>>>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                entities: Rc::new(RefCell::new(BTreeMap::from([(
                    ("Things".to_string(), "A".to_string()),
                    b"<Entities><Things><Thing name=\"A\"/></Things></Entities>".to_vec(),
                )]))),
                imports: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl backup::Remote for Fake {
        fn export(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(self
                .entities
                .borrow()
                .get(&(key.collection().to_string(), key.name().to_string()))
                .cloned())
        }

        fn import(&self, file_name: &str, _: &[u8]) -> Result<(), ServerError> {
            self.imports.borrow_mut().push(file_name.to_string());
            Ok(())
        }

        fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
            Ok(self
                .entities
                .borrow()
                .contains_key(&(key.collection().to_string(), key.name().to_string())))
        }
    }

    fn setup() -> (tempfile::TempDir, PathBuf, Solution, backup::Set, Fake) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-restore-")
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
        let fake = Fake::new();
        let set = backup::save(
            &fake,
            &solution,
            "test",
            &[EntityKey::new("Things", "A").unwrap()],
            "20261005-120000",
        )
        .unwrap()
        .unwrap();
        (root_guard, root, solution, set, fake)
    }

    #[test]
    fn plans_and_applies_restore_through_the_executor_without_a_workspace_lock() {
        let (_dir, _, solution, set, fake) = setup();
        let held = crate::core::lock::acquire_for(&solution, "holder").unwrap();
        let plan = RestoreRequest {
            set: Some(set.id.clone()),
            only: Vec::new(),
            mode: Mode::Plan,
            profile: "default".to_string(),
        };
        let outcome = execute(
            &solution,
            &plan,
            {
                let fake = fake.clone();
                move |_| fake
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.effects(), Effects::new(Access::Read, Access::Read));
        assert!(fake.imports.borrow().is_empty());
        let apply = RestoreRequest {
            mode: Mode::Apply,
            ..plan
        };
        let outcome = execute(
            &solution,
            &apply,
            {
                let fake = fake.clone();
                move |_| fake
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.effects(), Effects::new(Access::Read, Access::Write));
        assert_eq!(*fake.imports.borrow(), ["A.xml"]);
        drop(held);
    }
}
