//! The command policy around `twaco test`: read and check the cases offline, and call them only
//! when applied, on a profile that allows test users.

use super::{Access, Effects, Mode};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::profile;
use crate::core::testing::{self, cases, TestError};

#[derive(Clone, Debug)]
pub struct TestRequest {
    /// Keep the cases whose name, service or file contains one of these; all without any.
    pub filters: Vec<String>,
    pub mode: Mode,
    pub profile: String,
}

#[derive(Debug)]
pub enum TestOutcome {
    /// The cases that would run, read and checked; nothing was sent.
    Plan {
        cases: Vec<cases::Case>,
        effects: Effects,
    },
    Ran {
        cases: Vec<cases::Case>,
        outcomes: Vec<cases::Outcome>,
        effects: Effects,
    },
}

impl TestOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Ran { effects, .. } => *effects,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TestCommandError {
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Test(TestError),
}

impl Coded for TestCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(error) => error.code(),
            Self::Test(error) => error.code(),
        }
    }
}

/// Read the cases; when applied, call each as each of its users. A case calls its service as
/// written, which may change the server, so the profile must allow test users.
pub fn execute<R, F>(
    solution: &Solution,
    request: &TestRequest,
    open: F,
) -> Result<TestOutcome, TestCommandError>
where
    R: testing::Remote,
    F: Fn(profile::Profile) -> R,
{
    let cases = cases::load(solution, &request.filters).map_err(TestCommandError::Test)?;
    if request.mode == Mode::Plan {
        return Ok(TestOutcome::Plan {
            cases,
            effects: Effects::new(Access::Read, Access::None),
        });
    }
    let profile =
        profile::load(&solution.root, &request.profile).map_err(TestCommandError::Profile)?;
    testing::allowed(&profile).map_err(TestCommandError::Test)?;
    testing::test_password(&profile).map_err(TestCommandError::Test)?;
    let outcomes = cases::run(&cases, |user: &str| {
        testing::as_user(&profile, user).map(&open)
    })
    .map_err(TestCommandError::Test)?;
    Ok(TestOutcome::Ran {
        cases,
        outcomes,
        effects: Effects::new(Access::Read, Access::Write),
    })
}
