//! The command policy around the test users: which profile, which sessions, which access.

use super::{Access, Effects, Mode};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::profile;
use crate::core::testing::{self, users, TestError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsersAction {
    List,
    Seed { mode: Mode },
    Delete { mode: Mode },
}

#[derive(Clone, Debug)]
pub struct UsersRequest {
    pub action: UsersAction,
    pub profile: String,
}

#[derive(Debug)]
pub enum UsersOutcome {
    Listed {
        users: Vec<users::UserState>,
        effects: Effects,
    },
    Changed {
        changed: users::Changed,
        effects: Effects,
    },
}

impl UsersOutcome {
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); no adapter reads it yet"
    )]
    pub(crate) const fn effects(&self) -> Effects {
        match self {
            Self::Listed { effects, .. } | Self::Changed { effects, .. } => *effects,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UsersCommandError {
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Test(TestError),
}

impl Coded for UsersCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(error) => error.code(),
            Self::Test(error) => error.code(),
        }
    }
}

/// Run a users action. `open` makes a session from a profile: the administrator's, or a test
/// user's (the server of the profile, the user's name and the profile's `test_password`).
pub fn execute<R, F>(
    solution: &Solution,
    request: &UsersRequest,
    open: F,
) -> Result<UsersOutcome, UsersCommandError>
where
    R: testing::Remote,
    F: Fn(profile::Profile) -> R,
{
    let profile =
        profile::load(&solution.root, &request.profile).map_err(UsersCommandError::Profile)?;
    let admin = open(profile.clone());
    let sign_in = |user: &str| testing::as_user(&profile, user).map(&open);
    let written = |mode: &Mode| {
        if *mode == Mode::Apply {
            Access::Write
        } else {
            Access::Read
        }
    };
    match &request.action {
        UsersAction::List => Ok(UsersOutcome::Listed {
            users: users::list(solution, &profile, &admin, sign_in)
                .map_err(UsersCommandError::Test)?,
            effects: Effects::new(Access::None, Access::Read),
        }),
        UsersAction::Seed { mode } => Ok(UsersOutcome::Changed {
            changed: users::seed(solution, &profile, &admin, sign_in, *mode == Mode::Apply)
                .map_err(UsersCommandError::Test)?,
            effects: Effects::new(Access::None, written(mode)),
        }),
        UsersAction::Delete { mode } => Ok(UsersOutcome::Changed {
            changed: users::delete(solution, &profile, &admin, *mode == Mode::Apply)
                .map_err(UsersCommandError::Test)?,
            effects: Effects::new(Access::None, written(mode)),
        }),
    }
}
