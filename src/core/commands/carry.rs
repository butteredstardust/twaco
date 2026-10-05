//! The command policy around carrying permissions between renamed entities.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{entity_carry, lock, profile};
use std::fmt;

/// The arguments that affect an entity carry.
#[derive(Clone, Debug)]
pub struct CarryRequest {
    pub pairs: Vec<entity_carry::Pair>,
    pub renamed: bool,
    pub mode: Mode,
    pub detail: bool,
    pub profile: String,
    pub lock_label: &'static str,
}

/// One completed entity carry, before either adapter projects it to its wire format.
#[derive(Debug)]
pub enum CarryOutcome {
    Plan {
        report: entity_carry::Report,
        effects: Effects,
    },
    Applied {
        report: entity_carry::Report,
        date: String,
        effects: Effects,
    },
}

impl CarryOutcome {
    /// The access this specific outcome implies.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }
}

/// A remote that can compare and carry permissions.
pub trait Remote: entity_carry::Remote {}

impl<T: entity_carry::Remote + ?Sized> Remote for T {}

/// A failure before a typed carry outcome could be produced.
#[derive(Debug)]
pub enum CarryCommandError {
    Lock(lock::LockError),
    Profile(profile::ProfileError),
    Carry(entity_carry::CarryError),
}

impl fmt::Display for CarryCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(f),
            Self::Profile(error) => error.fmt(f),
            Self::Carry(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CarryCommandError {}

impl Coded for CarryCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Profile(error) => error.code(),
            Self::Carry(error) => error.code(),
        }
    }
}

/// Plan or apply carrying permissions. Applying pending ledger entries locks before loading the
/// profile or reading the ledger, because a successful carry records those entries locally.
pub fn execute<R, F>(
    solution: &Solution,
    request: &CarryRequest,
    open: F,
    notices: &mut Notices,
) -> Result<CarryOutcome, CarryCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let _lock = if matches!(request.mode, Mode::Apply) && request.renamed {
        Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(CarryCommandError::Lock)?,
        )
    } else {
        None
    };
    let profile =
        profile::load(&solution.root, &request.profile).map_err(CarryCommandError::Profile)?;
    let date = jiff::Zoned::now().strftime("%Y-%m-%d").to_string();
    let domain_request = entity_carry::Request {
        pairs: request.pairs.clone(),
        renamed: request.renamed,
        apply: matches!(request.mode, Mode::Apply),
        detail: request.detail,
    };
    let report = entity_carry::run(&open(profile), solution, &domain_request, &date)
        .map_err(CarryCommandError::Carry)?;
    Ok(match request.mode {
        Mode::Plan => CarryOutcome::Plan {
            report,
            effects: Effects::new(Access::Read, Access::Read),
        },
        Mode::Apply => {
            let workspace = if report.ledger_changed {
                Access::Write
            } else {
                Access::Read
            };
            CarryOutcome::Applied {
                report,
                date,
                effects: Effects::new(workspace, Access::Write),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::codes::ErrorCode;
    use crate::core::server::ServerError;
    use serde_json::Value;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fake;

    impl entity_carry::Remote for Fake {
        fn exists(&self, _: &str, _: &str) -> Result<bool, ServerError> {
            Ok(false)
        }

        fn get(&self, _: &str, _: &str, _: entity_carry::Kind) -> Result<Value, ServerError> {
            unreachable!("a missing entity is reported without reading permissions")
        }

        fn set(
            &self,
            _: &str,
            _: &str,
            _: entity_carry::Kind,
            _: &Value,
        ) -> Result<(), ServerError> {
            unreachable!("a missing entity is never written")
        }

        fn differences(&self, _: &str, _: &str, _: &str) -> Result<usize, ServerError> {
            unreachable!("detail is disabled")
        }
    }

    fn root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "twaco-command-carry-{}-{nonce}",
            std::process::id()
        ))
    }

    fn setup(profile: bool) -> (PathBuf, Solution) {
        let root = root();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\nroot = \".\"\n",
        )
        .unwrap();
        if profile {
            std::fs::write(
                root.join(".twaco/profiles/default.toml"),
                "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
            )
            .unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    fn request(mode: Mode, renamed: bool) -> CarryRequest {
        CarryRequest {
            pairs: entity_carry::pairs_from_names(&[
                "Things/Old".to_string(),
                "Things/New".to_string(),
            ])
            .unwrap(),
            renamed,
            mode,
            detail: false,
            profile: "default".to_string(),
            lock_label: "test carry",
        }
    }

    #[test]
    fn plans_do_not_lock_and_an_apply_locks_before_loading_its_profile() {
        let (root, solution) = setup(true);
        let held = lock::acquire_for(&solution, "holder").unwrap();
        let outcome = execute(
            &solution,
            &request(Mode::Plan, false),
            |_| Fake,
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.effects(), Effects::new(Access::Read, Access::Read));
        drop(held);
        std::fs::remove_dir_all(root).unwrap();

        let (root, solution) = setup(false);
        let held = lock::acquire_for(&solution, "holder").unwrap();
        let error = execute(
            &solution,
            &request(Mode::Apply, true),
            |_| Fake,
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(matches!(error, CarryCommandError::Lock(_)), "{error}");
        drop(held);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn applies_report_lock_recovery_notices_and_keep_error_codes() {
        let (root, solution) = setup(true);
        std::fs::write(root.join(".twaco/.baseline.json.1.twaco-tmp"), b"half").unwrap();
        let mut notices = Notices::default();
        let outcome = execute(
            &solution,
            &request(Mode::Apply, true),
            |_| Fake,
            &mut notices,
        )
        .unwrap();
        assert_eq!(outcome.effects(), Effects::new(Access::Read, Access::Write));
        assert_eq!(notices.lines().len(), 1, "{:?}", notices.lines());
        assert!(notices.lines()[0].contains("left by an interrupted write"));
        let error = CarryCommandError::Carry(entity_carry::CarryError::Arguments {
            why: "bad arguments".to_string(),
        });
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        std::fs::remove_dir_all(root).unwrap();
    }
}
