//! Server profiles, deliberately separate from committed solution configuration.
//!
//! Loading is an operation, not part of [`Solution`](super::config::Solution) discovery. That
//! separation is the lazy-credential guarantee: an offline command never visits a profile path
//! and never requires a credential-shaped environment variable merely to start.

use super::codes::{Coded, ErrorCode};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Everything the read-only HTTP client needs. App keys are accepted for future endpoints but
/// Basic authentication remains the default because ThingWorx's Importer rejects app keys.
#[derive(Clone, Deserialize, PartialEq)]
pub struct Profile {
    #[serde(alias = "server_url")]
    pub(crate) url: String,
    pub(crate) username: String,
    pub(crate) password: Secret,
    #[serde(default, alias = "appKey")]
    pub(crate) app_key: Option<Secret>,
    /// An optional local ThingWorx Extension SDK, used for its matching Javadoc.
    #[serde(default)]
    pub(crate) sdk: Option<PathBuf>,
    /// Project-specific secrets and values. They remain outside committed solution config.
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, toml::Value>,
}

/// A credential. It has no `Display` and its `Debug` is redacted, so it is read only through
/// [`Secret::expose`], and its memory is zeroed when it is dropped.
#[derive(Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// An empty secret with room for `capacity` bytes, so that text pushed into it up to that
    /// size is never moved to a new allocation and left behind unzeroed in the old one.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self(String::with_capacity(capacity))
    }

    /// Append `text`. When it does not fit, the secret moves to a larger buffer and the old one
    /// is zeroed, which `String::push_str` alone would free with the secret still in it.
    pub(crate) fn push_str(&mut self, text: &str) {
        if self.0.capacity() - self.0.len() < text.len() {
            let needed = self.0.len() + text.len();
            let mut grown = Self::with_capacity(needed.max(self.0.capacity() * 2));
            grown.0.push_str(&self.0);
            std::mem::swap(self, &mut grown);
        }
        self.0.push_str(text);
    }

    /// The text, leaving this secret empty. For moving a value into another zeroing holder.
    fn take(&mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

/// Zero every string inside `value`, for a TOML value that held profile text.
pub(crate) fn zeroize_toml(value: &mut toml::Value) {
    match value {
        toml::Value::String(text) => zeroize::Zeroize::zeroize(text),
        toml::Value::Array(values) => values.iter_mut().for_each(zeroize_toml),
        toml::Value::Table(table) => table.iter_mut().for_each(|(_, value)| zeroize_toml(value)),
        toml::Value::Integer(_)
        | toml::Value::Float(_)
        | toml::Value::Boolean(_)
        | toml::Value::Datetime(_) => {}
    }
}

/// Zero every string inside `value`, for a request parameter that held a credential.
pub(crate) fn zeroize_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => zeroize::Zeroize::zeroize(text),
        serde_json::Value::Array(values) => values.iter_mut().for_each(zeroize_json),
        serde_json::Value::Object(map) => map.iter_mut().for_each(|(_, value)| zeroize_json(value)),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

/// The address may carry `user:password`, and free-form values hold database passwords and other
/// secrets, so they are zeroed like the credentials themselves.
impl Drop for Profile {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.url);
        zeroize::Zeroize::zeroize(&mut self.username);
        self.extra.values_mut().for_each(zeroize_toml);
    }
}

/// Never expose credentials or free-form profile values through diagnostic formatting.
impl fmt::Debug for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Profile")
            .field("url", &"<redacted>")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .field("app_key", &self.app_key.as_ref().map(|_| "<redacted>"))
            .field("sdk", &self.sdk)
            .field("extra", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Profile {
    /// The optional Extension SDK path kept in this local profile.
    pub fn sdk(&self) -> Option<&Path> {
        self.sdk.as_deref()
    }

    /// Look up a placeholder value, including the standard profile fields.
    pub(crate) fn value(&self, key: &str) -> Option<toml::Value> {
        match key {
            "url" | "server_url" => Some(toml::Value::String(self.url.clone())),
            "username" => Some(toml::Value::String(self.username.clone())),
            "password" => Some(toml::Value::String(self.password.expose().to_string())),
            "app_key" | "appKey" => self
                .app_key
                .as_ref()
                .map(|key| toml::Value::String(key.expose().to_string())),
            _ => self.extra.get(key).cloned(),
        }
    }

    /// The database login override kept in the uncommitted server profile.
    pub(crate) fn database_user(&self) -> Option<&str> {
        self.extra
            .get("database_user")
            .and_then(toml::Value::as_str)
            .filter(|value| !value.is_empty())
    }

    /// The database secret kept in the uncommitted server profile.
    pub(crate) fn database_password(&self) -> Option<&str> {
        self.extra
            .get("database_password")
            .and_then(toml::Value::as_str)
            .filter(|value| !value.is_empty())
    }
}

#[derive(Debug)]
pub enum ProfileError {
    InvalidName(String),
    Missing {
        name: String,
        searched: Vec<PathBuf>,
    },
    Unreadable {
        path: PathBuf,
        why: String,
    },
    Invalid {
        path: PathBuf,
        why: String,
    },
    Incomplete {
        source: String,
        missing: Vec<&'static str>,
    },
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::InvalidName(name) => write!(f, "invalid profile name {name:?}"),
            ProfileError::Missing { name, searched } => {
                write!(
                f,
                "profile {name:?} was not found (looked in {}) and the environment is incomplete",
                searched.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
            )
            }
            ProfileError::Unreadable { path, why } => {
                write!(f, "cannot read profile {}: {why}", path.display())
            }
            ProfileError::Invalid { path, why } => write!(f, "profile {}: {why}", path.display()),
            ProfileError::Incomplete { source, missing } => {
                write!(f, "{source} is missing {}", missing.join(", "))
            }
        }
    }
}

impl std::error::Error for ProfileError {}

/// Load `name` with workspace-over-global shadowing and environment overrides.
pub fn load(solution_root: &Path, name: &str) -> Result<Profile, ProfileError> {
    let environment: BTreeMap<String, Secret> = std::env::vars()
        .filter(|(key, _)| key.starts_with("TWACO_") || key.starts_with("TWX_"))
        .map(|(key, value)| (key, Secret(value)))
        .collect();
    let home = home();
    load_from(solution_root, home.as_deref(), name, &environment)
}

/// The user's home: `USERPROFILE`, then `HOME`, and none when neither is set. Not
/// `dirs::home_dir`, which falls back to the password database, so that clearing both
/// variables really means "no user profiles" (as the snapshot tests do).
fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Warn, as ssh does, when a profile file that holds a password can be read by other users.
/// It is a warning, not a refusal: the file may sit on a share whose modes mean nothing.
fn warn_if_shared(path: &Path) {
    if let Some(mode) = shared_mode(path) {
        super::diagnostics::notice(&shared_warning(path, mode));
    }
}

fn shared_warning(path: &Path, mode: u32) -> String {
    format!(
        "the profile {} can be read by other users (mode {mode:o}); run chmod 600 on it",
        path.display()
    )
}

/// The file's permission bits when group or others may read or write it.
#[cfg(unix)]
fn shared_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then_some(mode)
}

#[cfg(not(unix))]
fn shared_mode(_path: &Path) -> Option<u32> {
    None
}

/// A server address fit to show: credentials written into it (`https://user:token@host/`) are
/// replaced by `***`, whatever the profile's own fields hold.
pub(crate) fn shown_url(url: &str) -> String {
    hide_url_credentials(url)
}

/// `text` with the credentials of every address in it (`scheme://user:secret@host`) replaced by
/// `***`: for messages that quote a URL, such as a server error, wherever it came from.
pub(crate) fn hide_url_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (before, after) = rest.split_at(at + 3);
        out.push_str(before);
        let end = after
            .find(|c: char| {
                matches!(c, '/' | '?' | '#' | '"' | '\'' | '<' | '>') || c.is_whitespace()
            })
            .unwrap_or(after.len());
        let (authority, tail) = after.split_at(end);
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***@");
                out.push_str(&authority[at + 1..]);
            }
            None => out.push_str(authority),
        }
        // The rest of this address (path, query) is copied as it is: a `://` inside it is not
        // another address.
        let url_end = tail
            .find(|c: char| matches!(c, '"' | '\'' | '<' | '>') || c.is_whitespace())
            .unwrap_or(tail.len());
        out.push_str(&tail[..url_end]);
        rest = &tail[url_end..];
    }
    out.push_str(rest);
    out
}

/// The `user:secret` written into an address, if any.
pub(crate) fn url_credentials(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    authority.rfind('@').map(|at| &authority[..at])
}

/// Where `name` would be read from, for `doctor`: the first profile file that exists (the
/// workspace's, then the user's), else the environment, and whether environment variables
/// override any of its fields. Mirrors [`load`]'s selection; it never reads a secret.
pub(crate) fn source(solution_root: &Path, name: &str) -> String {
    let home = home();
    let file = format!("{name}.toml");
    let local = solution_root.join(".twaco").join("profiles").join(&file);
    let global = home.map(|home| home.join(".twaco").join("profiles").join(&file));
    let base = if local.is_file() {
        local.display().to_string()
    } else if let Some(global) = global.filter(|g| g.is_file()) {
        global.display().to_string()
    } else {
        "the environment".to_string()
    };
    let overridden: Vec<&str> = ["URL", "USERNAME", "PASSWORD", "APP_KEY"]
        .into_iter()
        .filter(|suffix| {
            [format!("TWACO_{suffix}"), format!("TWX_{suffix}")]
                .iter()
                .any(|key| std::env::var(key).is_ok_and(|v| !v.is_empty()))
        })
        .collect();
    if overridden.is_empty() || base == "the environment" {
        base
    } else {
        format!(
            "{base}, with {} from the environment",
            overridden.join(", ").to_lowercase()
        )
    }
}

/// Write `key` as the `app_key` of the profile file `name` resolves to (the same file `load`
/// reads), replacing an `app_key` or `appKey` line or adding one, and keeping every other line
/// and the file's permissions. The file must exist: a key alone is not a profile. Returns the
/// file written.
pub(crate) fn save_app_key(
    solution_root: &Path,
    name: &str,
    key: &Secret,
) -> Result<PathBuf, ProfileError> {
    save_app_key_in(solution_root, home().as_deref(), name, key)
}

/// The profile file `name` resolves to; refused when there is none.
pub(crate) fn existing_file(solution_root: &Path, name: &str) -> Result<PathBuf, ProfileError> {
    existing_file_in(solution_root, home().as_deref(), name)
}

fn existing_file_in(
    solution_root: &Path,
    home: Option<&Path>,
    name: &str,
) -> Result<PathBuf, ProfileError> {
    let searched = files(solution_root, home, name)?;
    searched
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or_else(|| ProfileError::Missing {
            name: name.to_string(),
            searched,
        })
}

fn save_app_key_in(
    solution_root: &Path,
    home: Option<&Path>,
    name: &str,
    key: &Secret,
) -> Result<PathBuf, ProfileError> {
    let path = existing_file_in(solution_root, home, name)?;
    let unreadable = |why: String| ProfileError::Unreadable {
        path: path.clone(),
        why,
    };
    let text = Secret::new(std::fs::read_to_string(&path).map_err(|e| unreadable(e.to_string()))?);
    let newline = if text.expose().contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut quoted = toml::Value::String(key.expose().to_string());
    let rendered = Secret::new(quoted.to_string());
    zeroize_toml(&mut quoted);
    let mut line = Secret::with_capacity("app_key = ".len() + rendered.expose().len());
    line.push_str("app_key = ");
    line.push_str(rendered.expose());
    // Top-level keys come before the first table; app_key goes where the old one was, or after
    // the last top-level line.
    let mut lines: Vec<&str> = text.expose().lines().collect();
    let first_table = lines
        .iter()
        .position(|line| line.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    let is_key = |line: &str| {
        line.split_once('=').is_some_and(|(left, _)| {
            matches!(
                left.trim(),
                "app_key" | "appKey" | "\"app_key\"" | "\"appKey\""
            )
        })
    };
    let existing: Vec<usize> = (0..first_table).filter(|at| is_key(lines[*at])).collect();
    let at = existing.first().copied().unwrap_or_else(|| {
        (0..first_table)
            .rev()
            .find(|at| !lines[*at].trim().is_empty())
            .map_or(0, |at| at + 1)
    });
    for remove in existing.iter().rev() {
        lines.remove(*remove);
    }
    lines.insert(at, line.expose());
    let mut out = Secret::new(lines.join(newline));
    if text.expose().ends_with('\n') || text.expose().is_empty() {
        out.push_str(newline);
    }
    let written: Profile = toml::from_str(out.expose()).map_err(|e| ProfileError::Invalid {
        path: path.clone(),
        why: format!("adding app_key would make it invalid: {e}"),
    })?;
    if written.app_key.as_ref().map(Secret::expose) != Some(key.expose()) {
        return Err(ProfileError::Invalid {
            path: path.clone(),
            why: "app_key did not read back as written".to_string(),
        });
    }
    // The replacement keeps the old file's permissions, which for a profile are usually
    // owner-only, from before it holds any byte (see `atomic_replace`).
    super::workspace::atomic_replace(&path, out.expose().as_bytes())
        .map_err(|e| unreadable(format!("could not write it: {e}")))?;
    Ok(path)
}

/// The deterministic half, injectable because process-global environment mutation makes tests
/// race each other. `TWACO_*` wins over its legacy `TWX_*` alias when both are present.
/// The files a profile name can live in, in the order they are tried: the workspace's, then the
/// user's.
fn files(
    solution_root: &Path,
    home: Option<&Path>,
    name: &str,
) -> Result<Vec<PathBuf>, ProfileError> {
    validate_name(name)?;
    let file = |root: &Path| {
        root.join(".twaco")
            .join("profiles")
            .join(format!("{name}.toml"))
    };
    Ok(std::iter::once(file(solution_root))
        .chain(home.map(file))
        .collect())
}

/// Refuse names which cannot safely be used as one profile file name.
pub(crate) fn validate_name(name: &str) -> Result<(), ProfileError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
    {
        return Err(ProfileError::InvalidName(name.to_string()));
    }
    Ok(())
}

fn load_from(
    solution_root: &Path,
    home: Option<&Path>,
    name: &str,
    environment: &BTreeMap<String, Secret>,
) -> Result<Profile, ProfileError> {
    let searched = files(solution_root, home, name)?;
    // First existing file wins; an invalid local profile is an error, not permission to fall
    // through to a different server in the user's global profile.
    let selected = searched
        .iter()
        .map(PathBuf::as_path)
        .find(|path| path.is_file());
    // Every standard field is held as a secret until it is placed: the URL may carry a password,
    // and an error return drops whatever was read so far.
    let mut values = BTreeMap::<&'static str, Secret>::new();
    let mut extra = BTreeMap::<String, toml::Value>::new();
    let mut sdk = None;
    let mut source = "environment".to_string();
    if let Some(path) = selected {
        warn_if_shared(path);
        let text =
            Secret::new(
                std::fs::read_to_string(path).map_err(|e| ProfileError::Unreadable {
                    path: path.to_path_buf(),
                    why: e.to_string(),
                })?,
            );
        let mut profile: Profile =
            toml::from_str(text.expose()).map_err(|e| ProfileError::Invalid {
                path: path.to_path_buf(),
                why: e.to_string(),
            })?;
        values.insert("url", Secret(std::mem::take(&mut profile.url)));
        values.insert("username", Secret(std::mem::take(&mut profile.username)));
        values.insert("password", Secret(profile.password.take()));
        if let Some(app_key) = profile.app_key.take() {
            values.insert("app_key", app_key);
        }
        sdk = profile.sdk.take();
        extra = std::mem::take(&mut profile.extra);
        source = path.display().to_string();
    }

    for (field, suffix) in [
        ("url", "URL"),
        ("username", "USERNAME"),
        ("password", "PASSWORD"),
        ("app_key", "APP_KEY"),
    ] {
        let value = environment
            .get(&format!("TWACO_{suffix}"))
            .or_else(|| environment.get(&format!("TWX_{suffix}")));
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            values.insert(field, value.clone());
        }
    }

    // Free-form keys use their TOML spelling in a file and the conventional uppercase,
    // underscore-separated spelling in the environment. Standard fields were handled above.
    for (key, value) in environment {
        let Some(suffix) = key.strip_prefix("TWACO_") else {
            continue;
        };
        if matches!(suffix, "URL" | "USERNAME" | "PASSWORD" | "APP_KEY") || value.is_empty() {
            continue;
        }
        if let Some(mut replaced) = extra.insert(
            suffix.to_ascii_lowercase(),
            toml::Value::String(value.expose().to_string()),
        ) {
            zeroize_toml(&mut replaced);
        }
    }

    let missing: Vec<&'static str> = ["url", "username", "password"]
        .into_iter()
        .filter(|field| {
            values
                .get(field)
                .is_none_or(|value| value.expose().trim().is_empty())
        })
        .collect();
    if !missing.is_empty() {
        extra.values_mut().for_each(zeroize_toml);
        if selected.is_none() && values.is_empty() {
            return Err(ProfileError::Missing {
                name: name.to_string(),
                searched,
            });
        }
        return Err(ProfileError::Incomplete { source, missing });
    }

    let mut field = |name| values.remove(name);
    Ok(Profile {
        url: field("url").expect("checked above").take(),
        username: field("username").expect("checked above").take(),
        password: field("password").expect("checked above"),
        app_key: field("app_key"),
        sdk,
        extra,
    })
}

impl Coded for ProfileError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidName(_) => ErrorCode::InvalidArguments,
            Self::Missing { .. }
            | Self::Incomplete { .. }
            | Self::Unreadable { .. }
            | Self::Invalid { .. } => ErrorCode::InvalidData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_redacted_in_debug_and_read_only_by_name() {
        let secret = Secret::from("hunter2");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(format!("{:?}", Some(&secret)), "Some(<redacted>)");
        assert_eq!(secret.expose(), "hunter2");
    }

    /// Growing past the reserved room must keep every byte, in order, across the move.
    #[test]
    fn a_secret_grows_without_losing_text() {
        let mut secret = Secret::with_capacity(3);
        for part in ["abc", "d", "efghijklmnop", "", "q"] {
            secret.push_str(part);
        }
        assert_eq!(secret.expose(), "abcdefghijklmnopq");
        let room = secret.0.capacity();
        secret.push_str(&"x".repeat(room));
        assert_eq!(secret.expose().len(), 17 + room);
    }

    #[test]
    fn zeroing_a_value_clears_every_nested_string() {
        let mut toml_value: toml::Value =
            toml::from_str("a = 'one'\nb = ['two', 3]\nc = { d = 'four' }").unwrap();
        zeroize_toml(&mut toml_value);
        assert_eq!(
            toml_value.to_string().matches("\"\"").count(),
            3,
            "{toml_value}"
        );
        let mut json_value =
            serde_json::json!({ "a": "one", "b": ["two", 3], "c": { "d": "four" } });
        zeroize_json(&mut json_value);
        assert_eq!(
            json_value,
            serde_json::json!({ "a": "", "b": ["", 3], "c": { "d": "" } })
        );
    }

    #[test]
    fn an_app_key_replaces_the_old_one_or_joins_the_top_level_keys() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join(".twaco/profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        let path = profiles.join("dev.toml");
        let key = Secret::new("2d7c0b6e-1111");
        let save = |text: &str| {
            std::fs::write(&path, text).unwrap();
            save_app_key_in(dir.path(), None, "dev", &key).unwrap();
            std::fs::read_to_string(&path).unwrap()
        };
        assert_eq!(
            save("url = \"u\"\nusername = \"a\"\npassword = \"p\"\n"),
            "url = \"u\"\nusername = \"a\"\npassword = \"p\"\napp_key = \"2d7c0b6e-1111\"\n"
        );
        assert_eq!(
            save("url = \"u\"\r\nappKey = \"old\"\r\nusername = \"a\"\r\npassword = \"p\"\r\n"),
            "url = \"u\"\r\napp_key = \"2d7c0b6e-1111\"\r\nusername = \"a\"\r\npassword = \"p\"\r\n"
        );
        assert_eq!(
            save("url = \"u\"\nusername = \"a\"\npassword = \"p\"\n\n[deploy]\nx = 1\n"),
            "url = \"u\"\nusername = \"a\"\npassword = \"p\"\napp_key = \"2d7c0b6e-1111\"\n\n[deploy]\nx = 1\n"
        );
        // A table's own app_key is not the profile's.
        let tabled =
            save("url = \"u\"\nusername = \"a\"\npassword = \"p\"\n[other]\napp_key = \"t\"\n");
        assert!(tabled.contains("[other]\napp_key = \"t\""), "{tabled}");
        assert!(
            tabled.starts_with(
                "url = \"u\"\nusername = \"a\"\npassword = \"p\"\napp_key = \"2d7c0b6e-1111\"\n"
            ),
            "{tabled}"
        );
        // No profile file: refused, nothing created.
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            save_app_key_in(dir.path(), None, "dev", &key),
            Err(ProfileError::Missing { .. })
        ));
        assert!(!path.exists());
        assert!(matches!(
            save_app_key_in(dir.path(), None, "../dev", &key),
            Err(ProfileError::InvalidName(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn saving_an_app_key_keeps_the_profile_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join(".twaco/profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        let path = profiles.join("dev.toml");
        std::fs::write(&path, "url = \"u\"\nusername = \"a\"\npassword = \"p\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        save_app_key_in(dir.path(), None, "dev", &Secret::new("k")).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_profile_others_can_read_is_reported_and_a_private_one_is_not() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dev.toml");
        std::fs::write(&path, "password = 'p'\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(shared_mode(&path), Some(0o644));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(shared_mode(&path), None);
    }

    #[test]
    fn a_shown_url_never_holds_the_credentials_written_into_it() {
        for (url, shown) in [
            (
                "http://localhost:8080/Thingworx/",
                "http://localhost:8080/Thingworx/",
            ),
            (
                "https://user:t0k3n@host/Thingworx/",
                "https://***@host/Thingworx/",
            ),
            ("https://a@b:pw@host:443", "https://***@host:443"),
            (
                "https://host/Thingworx/?u=a@b",
                "https://host/Thingworx/?u=a@b",
            ),
            ("not a url", "not a url"),
        ] {
            assert_eq!(shown_url(url), shown, "{url}");
        }
        assert_eq!(
            hide_url_credentials(
                "GET https://u:s3cret@host/Thingworx/Things/T failed; see http://a@b/x and ftp://plain/"
            ),
            "GET https://***@host/Thingworx/Things/T failed; see http://***@b/x and ftp://plain/"
        );
        assert_eq!(
            hide_url_credentials("see https://host/a://b@c and http://u:p@h/"),
            "see https://host/a://b@c and http://***@h/",
            "a :// in a path is not another address"
        );
        assert_eq!(url_credentials("https://u:s3cret@host/x"), Some("u:s3cret"));
        assert_eq!(url_credentials("https://host/x?a=b@c"), None);
    }

    /// A profile's `Debug` is what ends up in a panic message, a log line or an error chain, so it
    /// must show no secret however awkward: quotes, a newline, URL delimiters, non-ASCII.
    #[test]
    fn a_profile_never_prints_its_secrets_whatever_they_contain() {
        for password in [
            "plain-secret-1",
            "q\"u'o\\te",
            "line\nbreak",
            "a?b#c&d=e@f/g:h",
            "p\u{e4}ss\u{65e5}\u{672c}w\u{f6}rd",
            "]]>&amp;<x>",
        ] {
            let mut extra = BTreeMap::new();
            extra.insert(
                "database_password".to_string(),
                toml::Value::String(format!("db-{password}")),
            );
            let profile = Profile {
                url: format!("https://twx.example.test/{password}"),
                username: format!("user-{password}"),
                password: password.into(),
                app_key: Some(format!("key-{password}").into()),
                sdk: None,
                extra,
            };
            for shown in [format!("{profile:?}"), format!("{profile:#?}")] {
                for secret in [
                    password.to_string(),
                    format!("user-{password}"),
                    format!("key-{password}"),
                    format!("db-{password}"),
                    "twx.example.test".to_string(),
                ] {
                    assert!(!shown.contains(&secret), "{secret:?} was printed: {shown}");
                }
                assert!(
                    shown.contains("database_password"),
                    "the key names stay: {shown}"
                );
            }
        }
    }

    fn temp(label: &str) -> (tempfile::TempDir, PathBuf) {
        let path_guard = tempfile::Builder::new()
            .prefix(&format!("twaco-profile-{label}-"))
            .tempdir()
            .unwrap();
        let path = path_guard.path().to_path_buf();
        std::fs::create_dir_all(&path).unwrap();
        (path_guard, path)
    }

    fn write(path: &Path, url: &str, user: &str, password: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("url = {url:?}\nusername = {user:?}\npassword = {password:?}\n"),
        )
        .unwrap();
    }

    #[test]
    fn local_shadows_global_and_environment_overrides_the_selected_file() {
        let (_dir, root) = temp("precedence");
        let home = root.join("home");
        let local = root.join("repo/.twaco/profiles/default.toml");
        let global = home.join(".twaco/profiles/default.toml");
        write(&global, "https://global/", "global-user", "global-pass");
        write(&local, "https://local/", "local-user", "local-pass");

        let mut env = BTreeMap::new();
        env.insert("TWX_USERNAME".to_string(), Secret::from("legacy-user"));
        env.insert("TWACO_USERNAME".to_string(), Secret::from("override-user"));
        let profile = load_from(&root.join("repo"), Some(&home), "default", &env).unwrap();
        assert_eq!(profile.url, "https://local/");
        assert_eq!(profile.username, "override-user");
        assert_eq!(profile.password.expose(), "local-pass");

        std::fs::remove_file(local).unwrap();
        let profile =
            load_from(&root.join("repo"), Some(&home), "default", &BTreeMap::new()).unwrap();
        assert_eq!(profile.url, "https://global/");
    }

    #[test]
    fn free_form_values_are_flattened_and_twaco_environment_wins() {
        let (_dir, root) = temp("extra");
        let local = root.join("repo/.twaco/profiles/default.toml");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(
            &local,
            "url='http://server'\nusername='u'\npassword='p'\nprobe_marker='file'\ncount=2\n",
        )
        .unwrap();
        let env = BTreeMap::from([("TWACO_PROBE_MARKER".into(), "environment".into())]);
        let profile = load_from(&root.join("repo"), None, "default", &env).unwrap();
        assert_eq!(
            profile
                .value("probe_marker")
                .and_then(|v| v.as_str().map(str::to_owned)),
            Some("environment".into())
        );
        assert_eq!(profile.value("count").and_then(|v| v.as_integer()), Some(2));
        assert!(!format!("{profile:?}").contains("environment"));
    }

    #[test]
    fn environment_alone_can_form_a_profile() {
        let (_dir, root) = temp("environment");
        let env = BTreeMap::from([
            (
                "TWX_URL".to_string(),
                Secret::from("http://server/Thingworx/"),
            ),
            ("TWX_USERNAME".to_string(), Secret::from("user")),
            ("TWX_PASSWORD".to_string(), Secret::from("pass")),
        ]);
        assert_eq!(
            load_from(&root, None, "default", &env).unwrap().username,
            "user"
        );
    }

    #[test]
    fn a_profile_name_cannot_escape_its_directory() {
        assert!(matches!(
            load_from(Path::new("."), None, "../secret", &BTreeMap::new()),
            Err(ProfileError::InvalidName(_))
        ));
    }
}
