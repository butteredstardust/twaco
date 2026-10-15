//! Test users and declarative service tests: the accounts `[[test.user]]` declares, created on a
//! test server with the profile's `test_password`, and the cases `twaco test` runs as them.
//!
//! Both act on a server only when its profile says `allow_test_users = true`: a test account is
//! a way in, and has no place on a customer's server. The password lives in the profile, which
//! is never committed, and is never printed: every error passes through the client's scrubbing.

pub mod cases;
pub mod users;

use super::codes::{Coded, ErrorCode};
use super::entity_key::{EntityKey, ServiceTarget};
use super::profile::Profile;
use super::server::{Client, ServerError};
use serde_json::Value;
use std::time::Duration;

/// How long one service call of a test may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// What test users and cases ask of a server, as a trait so they are tested offline.
pub trait Remote: Sync {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError>;
    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError>;
}

impl Remote for Client {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        self.entity_exists(key)
    }

    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError> {
        self.call_service(target, service, parameters, CALL_TIMEOUT)
    }
}

/// The profile of a test user: the server of `profile`, signed in as `user` with the profile's
/// `test_password`. Nothing else of the profile is carried over, the app key least of all.
pub fn as_user(profile: &Profile, user: &str) -> Result<Profile, TestError> {
    Ok(Profile {
        url: profile.url.clone(),
        username: user.to_string(),
        password: test_password(profile)?.to_string(),
        app_key: None,
        // Kept so the client scrubs the test password (and the rest) from every message.
        extra: profile.extra.clone(),
    })
}

/// The shared password of every test user, from the profile.
pub fn test_password(profile: &Profile) -> Result<&str, TestError> {
    profile
        .extra
        .get("test_password")
        .and_then(toml::Value::as_str)
        .filter(|password| !password.is_empty())
        .ok_or(TestError::NoPassword)
}

/// Refuse unless the profile names its server a test server.
pub fn allowed(profile: &Profile) -> Result<(), TestError> {
    if profile
        .extra
        .get("allow_test_users")
        .and_then(toml::Value::as_bool)
        == Some(true)
    {
        Ok(())
    } else {
        Err(TestError::NotAllowed)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TestError {
    #[error(
        "this profile does not allow test users; add `allow_test_users = true` to it, only for a \
         server meant for tests"
    )]
    NotAllowed,
    #[error(
        "the profile has no test_password; add `test_password = \"...\"` to it (profiles are \
         never committed)"
    )]
    NoPassword,
    #[error("twaco.toml declares no [[test.user]]")]
    NoUsers,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Server(#[from] ServerError),
}

impl Coded for TestError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::NotAllowed | Self::NoPassword | Self::NoUsers => ErrorCode::InvalidArguments,
            Self::Invalid(_) => ErrorCode::InvalidData,
            Self::Server(error) => error.code(),
        }
    }
}

/// The status a call answered with: 200 for a reply, the HTTP status of a refusal. Anything
/// else (no connection, a reply that is not JSON) is not a status and stays an error.
pub fn status(result: &Result<Option<Value>, ServerError>) -> Option<u16> {
    match result {
        Ok(_) => Some(200),
        Err(ServerError::Http { status, .. }) => Some(*status),
        Err(_) => None,
    }
}
