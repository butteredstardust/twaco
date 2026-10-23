//! The command policy around application keys: plan a key, create it, or read one back, and put
//! the key only where the caller asked: on screen (`show`) or into a profile (`save`).

use super::{Access, Effects, Mode};
use crate::core::appkey::{self, AppKeyError, Key, NewKey};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::lock;
use crate::core::profile;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum AppKeyAction {
    Create { key: NewKey, mode: Mode },
    Show { name: String },
}

#[derive(Clone, Debug)]
pub struct AppKeyRequest {
    pub action: AppKeyAction,
    pub profile: String,
    /// Write the key as this profile's `app_key`.
    pub save: Option<String>,
}

pub enum AppKeyOutcome {
    /// What `create` would send; nothing was.
    Planned { key: NewKey, effects: Effects },
    /// A key created or read, and the profile file it was saved to.
    Done {
        created: bool,
        key: Key,
        saved: Option<PathBuf>,
        effects: Effects,
    },
}

impl AppKeyOutcome {
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); no adapter reads it yet"
    )]
    pub(crate) const fn effects(&self) -> Effects {
        match self {
            Self::Planned { effects, .. } | Self::Done { effects, .. } => *effects,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppKeyCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Key(AppKeyError),
    /// The key exists on the server but could not be saved; it is not lost.
    #[error(
        "application key {name} {} on the server, but it could not be saved to profile {profile}: \
         {why}; `twaco appkey show {name} --save {profile}` tries again",
        if *.created { "was created" } else { "is" }
    )]
    Save {
        name: String,
        profile: String,
        created: bool,
        why: profile::ProfileError,
    },
}

impl Coded for AppKeyCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Profile(error) | Self::Save { why: error, .. } => error.code(),
            Self::Key(error) => error.code(),
        }
    }
}

/// Plan, create or read a key. A plan checks the name and the profile to save to without
/// contacting the server. Saving writes the user's profile, not the workspace's managed files,
/// so it takes no workspace lock.
pub fn execute<R, F>(
    solution: &Solution,
    request: &AppKeyRequest,
    open: F,
) -> Result<AppKeyOutcome, AppKeyCommandError>
where
    R: appkey::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    // Where the key would go is checked first: a key the server made but no profile could take
    // would only be readable again through `show`.
    if let Some(name) = &request.save {
        lock::reject_unlocked_write_links(solution).map_err(AppKeyCommandError::Lock)?;
        profile::existing_file(&solution.root, name).map_err(AppKeyCommandError::Profile)?;
    }
    if let AppKeyAction::Create { key, mode } = &request.action {
        appkey::check_name(&key.name).map_err(AppKeyCommandError::Key)?;
        if key.user.trim().is_empty() {
            return Err(AppKeyCommandError::Key(AppKeyError::Invalid(
                "a key acts as a user: name it with --user".to_string(),
            )));
        }
        if *mode == Mode::Plan {
            return Ok(AppKeyOutcome::Planned {
                key: key.clone(),
                effects: Effects::new(Access::None, Access::None),
            });
        }
    }
    let profile =
        profile::load(&solution.root, &request.profile).map_err(AppKeyCommandError::Profile)?;
    let remote = open(profile);
    let (created, key) = match &request.action {
        AppKeyAction::Create { key, .. } => (true, appkey::create(&remote, key)),
        AppKeyAction::Show { name } => (false, appkey::show(&remote, name)),
    };
    let key = key.map_err(AppKeyCommandError::Key)?;
    let saved = match &request.save {
        None => None,
        Some(name) => Some(
            profile::save_app_key(&solution.root, name, &key.value).map_err(|why| {
                AppKeyCommandError::Save {
                    name: key.name.clone(),
                    profile: name.clone(),
                    created,
                    why,
                }
            })?,
        ),
    };
    let effects = Effects::new(
        if saved.is_some() {
            Access::Write
        } else {
            Access::None
        },
        if created { Access::Write } else { Access::Read },
    );
    Ok(AppKeyOutcome::Done {
        created,
        key,
        saved,
        effects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity::key::{EntityKey, ServiceTarget};
    use crate::core::server::ServerError;
    use serde_json::{json, Value};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Fake(Rc<RefCell<Vec<String>>>);

    impl appkey::Remote for Fake {
        fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
            self.0.borrow_mut().push(format!("exists {}", key.name()));
            Ok(false)
        }

        fn call(
            &self,
            target: &ServiceTarget,
            service: &str,
            _: &Value,
        ) -> Result<Option<Value>, ServerError> {
            self.0.borrow_mut().push(format!("{target}.{service}"));
            Ok(Some(json!({"rows": [{"KeyID": "secret-key-id"}]})))
        }
    }

    fn solution() -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::create_dir_all(dir.path().join(".twaco/profiles")).unwrap();
        std::fs::write(
            dir.path().join(".twaco/profiles/default.toml"),
            "url = \"http://server/Thingworx\"\nusername = \"Administrator\"\npassword = \"admin\"\n",
        )
        .unwrap();
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn create(mode: Mode, save: Option<&str>) -> AppKeyRequest {
        AppKeyRequest {
            action: AppKeyAction::Create {
                key: NewKey {
                    name: "Acme.Kiosk_AK".into(),
                    user: "kiosk".into(),
                    expires: "2027-01-01T00:00:00Z".parse().unwrap(),
                    project: None,
                    description: String::new(),
                    whitelist: Vec::new(),
                },
                mode,
            },
            profile: "default".into(),
            save: save.map(str::to_string),
        }
    }

    #[test]
    fn a_plan_sends_nothing_and_a_missing_profile_to_save_to_is_refused_first() {
        let (_dir, solution) = solution();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let planned = execute(&solution, &create(Mode::Plan, None), |_| {
            Fake(calls.clone())
        })
        .unwrap();
        assert!(matches!(planned, AppKeyOutcome::Planned { .. }));
        let refused = execute(&solution, &create(Mode::Apply, Some("kiosk")), |_| {
            Fake(calls.clone())
        });
        assert!(matches!(refused, Err(AppKeyCommandError::Profile(_))));
        assert!(calls.borrow().is_empty(), "{:?}", calls.borrow());
    }

    #[test]
    fn create_saves_the_key_into_the_profile_and_never_into_a_message() {
        let (dir, solution) = solution();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let outcome = execute(&solution, &create(Mode::Apply, Some("default")), |_| {
            Fake(calls.clone())
        })
        .unwrap();
        let AppKeyOutcome::Done {
            created,
            key,
            saved,
            ..
        } = outcome
        else {
            panic!("created");
        };
        assert!(created);
        assert_eq!(key.reveal(), "secret-key-id");
        assert_eq!(
            saved.unwrap(),
            dir.path().join(".twaco/profiles/default.toml")
        );
        let profile =
            std::fs::read_to_string(dir.path().join(".twaco/profiles/default.toml")).unwrap();
        assert!(
            profile.ends_with("app_key = \"secret-key-id\"\n"),
            "{profile}"
        );
        assert_eq!(
            calls.borrow().as_slice(),
            [
                "exists Acme.Kiosk_AK",
                "Resources/EntityServices.CreateApplicationKey",
                "ApplicationKeys/Acme.Kiosk_AK.GetKeyID"
            ]
        );
        let error = AppKeyCommandError::Save {
            name: "Acme.Kiosk_AK".into(),
            profile: "default".into(),
            created: true,
            why: profile::ProfileError::InvalidName("x".into()),
        };
        assert!(!error.to_string().contains("secret-key-id"));
        assert!(error
            .to_string()
            .contains("appkey show Acme.Kiosk_AK --save default"));
    }
}
