//! Application keys on the server: create one, or read an existing one's key.
//!
//! Verified on 10.1: `EntityServices.CreateApplicationKey` takes `name`, `description`, `user`,
//! `projectName`, `whitelist` (one string), `clientName`, `tags` and `expirationDate`, and
//! without `expirationDate` the key expires after one day, so twaco always sends one. A key's
//! value is `ApplicationKeys/<name>.GetKeyID` -> `rows[0].KeyID`. Re-importing a key's XML never
//! changes its expiration, which is why keys are created through the service, not as entities in
//! the repository. A duplicate name or an unknown user is refused by the server in its own words.
//!
//! The key is a credential: it is never logged, never in an error, and printed or written only
//! where the caller asked for it.

use super::entity::key::{EntityKey, ServiceTarget};
use super::profile::Secret;
use super::server::{self, ServerError};
use serde_json::{json, Value};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);

/// What the server is asked for.
pub trait Remote {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError>;
    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError>;
}

impl Remote for server::Client {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        self.entity_exists(key)
    }

    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError> {
        self.call_service(target, service, parameters, TIMEOUT)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppKeyError {
    #[error("{0}")]
    Invalid(String),
    #[error("application key {0} already exists on the server; `twaco appkey show {0}` reads it")]
    Exists(String),
    #[error("there is no application key {0} on the server")]
    Unknown(String),
    #[error("{0}")]
    Server(ServerError),
    #[error("the server answered GetKeyID for {0} without a KeyID")]
    NoKey(String),
}

impl super::codes::Coded for AppKeyError {
    fn code(&self) -> super::codes::ErrorCode {
        use super::codes::ErrorCode;
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Exists(_) => ErrorCode::AlreadyExists,
            Self::Unknown(_) => ErrorCode::UnknownEntity,
            Self::Server(error) => error.code(),
            Self::NoKey(_) => ErrorCode::InvalidData,
        }
    }
}

/// A key to create.
#[derive(Clone, Debug)]
pub struct NewKey {
    pub name: String,
    /// The user the key acts as.
    pub user: String,
    pub expires: jiff::Timestamp,
    pub project: Option<String>,
    pub description: String,
    /// IP addresses or ranges the key may be used from; empty means any.
    pub whitelist: Vec<String>,
}

/// An existing key, read from the server.
pub struct Key {
    pub name: String,
    pub expires: Option<jiff::Timestamp>,
    pub(crate) value: Secret,
}

impl Key {
    /// The key itself: for the caller who asked to see it, and no one else.
    pub fn reveal(&self) -> &str {
        self.value.expose()
    }
}

/// When a key expires: `30d` or `12h` from `now`, a date (midnight UTC that day), or a full
/// RFC 3339 timestamp. It must be in the future.
pub fn parse_expires(text: &str, now: jiff::Timestamp) -> Result<jiff::Timestamp, AppKeyError> {
    let text = text.trim();
    // Whole seconds: an expiration nobody reads to the nanosecond.
    let now = jiff::Timestamp::from_second(now.as_second()).unwrap_or(now);
    let invalid = || {
        AppKeyError::Invalid(format!(
            "--expires {text:?} is not 30d, 12h, a date (2027-01-31) or a timestamp \
             (2027-01-31T12:00:00Z)"
        ))
    };
    let relative = |unit: char, seconds: i64| -> Option<Result<jiff::Timestamp, AppKeyError>> {
        let count = text.strip_suffix(unit)?.parse::<i64>().ok()?;
        Some(
            count
                .checked_mul(seconds)
                .and_then(|span| jiff::SignedDuration::from_secs(span).into())
                .and_then(|span| now.checked_add(span).ok())
                .ok_or_else(invalid),
        )
    };
    let when = if let Some(when) = relative('d', 86_400).or_else(|| relative('h', 3_600)) {
        when?
    } else if let Ok(date) = text.parse::<jiff::civil::Date>() {
        date.to_zoned(jiff::tz::TimeZone::UTC)
            .map_err(|_| invalid())?
            .timestamp()
    } else {
        text.parse::<jiff::Timestamp>().map_err(|_| invalid())?
    };
    if when <= now {
        return Err(AppKeyError::Invalid(format!(
            "--expires {text:?} is not in the future ({when})"
        )));
    }
    Ok(when)
}

/// A name the server can hold: the same rule as any entity key.
pub fn check_name(name: &str) -> Result<EntityKey, AppKeyError> {
    EntityKey::new("ApplicationKeys", name)
        .map_err(|why| AppKeyError::Invalid(format!("application key name {name:?}: {why}")))
}

/// Create the key and read back its value.
pub fn create<R: Remote>(remote: &R, key: &NewKey) -> Result<Key, AppKeyError> {
    let entity = check_name(&key.name)?;
    if remote.exists(&entity).map_err(AppKeyError::Server)? {
        return Err(AppKeyError::Exists(key.name.clone()));
    }
    remote
        .call(
            &ServiceTarget::platform("Resources", "EntityServices"),
            "CreateApplicationKey",
            &json!({
                "name": key.name,
                "description": key.description,
                "user": key.user,
                "projectName": key.project.clone().unwrap_or_default(),
                "whitelist": key.whitelist.join(","),
                "expirationDate": key.expires.as_millisecond(),
            }),
        )
        .map_err(AppKeyError::Server)?;
    Ok(Key {
        name: key.name.clone(),
        expires: Some(key.expires),
        value: key_value(remote, &key.name)?,
    })
}

/// An existing key's value and expiration.
pub fn show<R: Remote>(remote: &R, name: &str) -> Result<Key, AppKeyError> {
    let entity = check_name(name)?;
    if !remote.exists(&entity).map_err(AppKeyError::Server)? {
        return Err(AppKeyError::Unknown(name.to_string()));
    }
    let target = key_target(name)?;
    let expires = remote
        .call(&target, "GetExpirationDate", &json!({}))
        .map_err(AppKeyError::Server)?
        .as_ref()
        .and_then(|reply| reply.pointer("/rows/0/ExpirationDate"))
        .and_then(Value::as_i64)
        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok());
    Ok(Key {
        name: name.to_string(),
        expires,
        value: key_value(remote, name)?,
    })
}

fn key_target(name: &str) -> Result<ServiceTarget, AppKeyError> {
    ServiceTarget::entity("ApplicationKeys", name)
        .map_err(|why| AppKeyError::Invalid(format!("application key name {name:?}: {why}")))
}

fn key_value<R: Remote>(remote: &R, name: &str) -> Result<Secret, AppKeyError> {
    let reply = remote
        .call(&key_target(name)?, "GetKeyID", &json!({}))
        .map_err(AppKeyError::Server)?;
    reply
        .as_ref()
        .and_then(|reply| reply.pointer("/rows/0/KeyID"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(Secret::new)
        .ok_or_else(|| AppKeyError::NoKey(name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records calls; `existing` names the keys the server has.
    struct Fake {
        existing: Vec<&'static str>,
        calls: RefCell<Vec<String>>,
    }

    impl Remote for Fake {
        fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
            Ok(self.existing.iter().any(|name| key.name() == *name))
        }

        fn call(
            &self,
            target: &ServiceTarget,
            service: &str,
            parameters: &Value,
        ) -> Result<Option<Value>, ServerError> {
            self.calls
                .borrow_mut()
                .push(format!("{target}.{service} {parameters}"));
            Ok(Some(match service {
                "GetKeyID" => json!({"rows": [{"KeyID": "2d7c0b6e-1111-2222-3333-444455556666"}]}),
                "GetExpirationDate" => json!({"rows": [{"ExpirationDate": 1_796_119_200_000_i64}]}),
                _ => json!({}),
            }))
        }
    }

    fn now() -> jiff::Timestamp {
        "2026-10-17T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn expires_reads_days_hours_dates_and_timestamps_in_the_future_only() {
        let at = |text: &str| parse_expires(text, now()).map(|when| when.to_string());
        let fraction: jiff::Timestamp = "2026-10-17T12:00:00.734Z".parse().unwrap();
        assert_eq!(
            parse_expires("1d", fraction).unwrap().to_string(),
            "2026-10-18T12:00:00Z"
        );
        assert_eq!(at("30d").unwrap(), "2026-11-16T12:00:00Z");
        assert_eq!(at("12h").unwrap(), "2026-10-18T00:00:00Z");
        assert_eq!(at("2027-01-31").unwrap(), "2027-01-31T00:00:00Z");
        assert_eq!(at("2027-01-31T08:30:00Z").unwrap(), "2027-01-31T08:30:00Z");
        for bad in [
            "",
            "0d",
            "-1d",
            "2026-10-01",
            "tomorrow",
            "30x",
            "99999999999999d",
        ] {
            assert!(at(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn create_sends_an_expiration_and_reads_the_key_back() {
        let fake = Fake {
            existing: Vec::new(),
            calls: RefCell::new(Vec::new()),
        };
        let key = create(
            &fake,
            &NewKey {
                name: "Acme.Kiosk_AK".into(),
                user: "kiosk".into(),
                expires: parse_expires("30d", now()).unwrap(),
                project: Some("Acme".into()),
                description: "kiosk".into(),
                whitelist: vec!["10.0.0.0/8".into(), "127.0.0.1".into()],
            },
        )
        .unwrap();
        assert_eq!(key.value.expose(), "2d7c0b6e-1111-2222-3333-444455556666");
        let calls = fake.calls.borrow();
        assert_eq!(
            calls[0],
            r#"Resources/EntityServices.CreateApplicationKey {"name":"Acme.Kiosk_AK","description":"kiosk","user":"kiosk","projectName":"Acme","whitelist":"10.0.0.0/8,127.0.0.1","expirationDate":1794830400000}"#
        );
        assert_eq!(calls[1], "ApplicationKeys/Acme.Kiosk_AK.GetKeyID {}");
    }

    #[test]
    fn create_refuses_an_existing_key_and_show_an_absent_one_without_calling() {
        let fake = Fake {
            existing: vec!["Acme.Kiosk_AK"],
            calls: RefCell::new(Vec::new()),
        };
        let new = NewKey {
            name: "Acme.Kiosk_AK".into(),
            user: "kiosk".into(),
            expires: parse_expires("1d", now()).unwrap(),
            project: None,
            description: String::new(),
            whitelist: Vec::new(),
        };
        assert!(matches!(create(&fake, &new), Err(AppKeyError::Exists(_))));
        assert!(matches!(
            show(&fake, "Acme.Other_AK"),
            Err(AppKeyError::Unknown(_))
        ));
        assert!(fake.calls.borrow().is_empty());
        let shown = show(&fake, "Acme.Kiosk_AK").unwrap();
        assert_eq!(shown.expires.unwrap().to_string(), "2026-12-01T10:00:00Z");
        assert!(check_name("bad/name").is_err());
    }
}
