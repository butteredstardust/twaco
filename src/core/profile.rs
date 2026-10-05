//! Server profiles, deliberately separate from committed solution configuration.
//!
//! Loading is an operation, not part of [`Solution`](super::config::Solution) discovery. That
//! separation is the lazy-credential guarantee: an offline command never visits a profile path
//! and never requires a credential-shaped environment variable merely to start.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Everything the read-only HTTP client needs. App keys are accepted for future endpoints but
/// Basic authentication remains the default because ThingWorx's Importer rejects app keys.
#[derive(Clone, Deserialize, PartialEq)]
pub struct Profile {
    #[serde(alias = "server_url")]
    pub url: String,
    pub username: String,
    pub password: String,
    #[serde(default, alias = "appKey")]
    pub app_key: Option<String>,
    /// Project-specific secrets and values. They remain outside committed solution config.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// Never expose credentials or free-form profile values through diagnostic formatting.
impl fmt::Debug for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Profile")
            .field("url", &"<redacted>")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .field("app_key", &self.app_key.as_ref().map(|_| "<redacted>"))
            .field("extra", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Profile {
    /// Look up a placeholder value, including the standard profile fields.
    pub fn value(&self, key: &str) -> Option<toml::Value> {
        match key {
            "url" | "server_url" => Some(toml::Value::String(self.url.clone())),
            "username" => Some(toml::Value::String(self.username.clone())),
            "password" => Some(toml::Value::String(self.password.clone())),
            "app_key" | "appKey" => self.app_key.clone().map(toml::Value::String),
            _ => self.extra.get(key).cloned(),
        }
    }

    /// The database login override kept in the uncommitted server profile.
    pub fn database_user(&self) -> Option<&str> {
        self.extra
            .get("database_user")
            .and_then(toml::Value::as_str)
            .filter(|value| !value.is_empty())
    }

    /// The database secret kept in the uncommitted server profile.
    pub fn database_password(&self) -> Option<&str> {
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
    let environment: BTreeMap<String, String> = std::env::vars()
        .filter(|(key, _)| key.starts_with("TWACO_") || key.starts_with("TWX_"))
        .collect();
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    load_from(solution_root, home.as_deref(), name, &environment)
}

/// Where `name` would be read from, for `doctor`: the first profile file that exists (the
/// workspace's, then the user's), else the environment, and whether environment variables
/// override any of its fields. Mirrors [`load`]'s selection; it never reads a secret.
pub fn source(solution_root: &Path, name: &str) -> String {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
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

/// The deterministic half, injectable because process-global environment mutation makes tests
/// race each other. `TWACO_*` wins over its legacy `TWX_*` alias when both are present.
fn load_from(
    solution_root: &Path,
    home: Option<&Path>,
    name: &str,
    environment: &BTreeMap<String, String>,
) -> Result<Profile, ProfileError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
    {
        return Err(ProfileError::InvalidName(name.to_string()));
    }

    let local = solution_root
        .join(".twaco")
        .join("profiles")
        .join(format!("{name}.toml"));
    let mut searched = vec![local.clone()];
    let global = home.map(|home| {
        home.join(".twaco")
            .join("profiles")
            .join(format!("{name}.toml"))
    });
    if let Some(path) = &global {
        searched.push(path.clone());
    }

    // First existing file wins; an invalid local profile is an error, not permission to fall
    // through to a different server in the user's global profile.
    let selected = std::iter::once(Some(local.as_path()))
        .chain(std::iter::once(global.as_deref()))
        .flatten()
        .find(|path| path.is_file());
    let mut values = BTreeMap::<&'static str, String>::new();
    let mut extra = BTreeMap::<String, toml::Value>::new();
    let mut source = "environment".to_string();
    if let Some(path) = selected {
        let text = std::fs::read_to_string(path).map_err(|e| ProfileError::Unreadable {
            path: path.to_path_buf(),
            why: e.to_string(),
        })?;
        let profile: Profile = toml::from_str(&text).map_err(|e| ProfileError::Invalid {
            path: path.to_path_buf(),
            why: e.to_string(),
        })?;
        values.insert("url", profile.url);
        values.insert("username", profile.username);
        values.insert("password", profile.password);
        if let Some(app_key) = profile.app_key {
            values.insert("app_key", app_key);
        }
        extra = profile.extra;
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
        extra.insert(
            suffix.to_ascii_lowercase(),
            toml::Value::String(value.clone()),
        );
    }

    let missing: Vec<&'static str> = ["url", "username", "password"]
        .into_iter()
        .filter(|field| {
            values
                .get(field)
                .is_none_or(|value| value.trim().is_empty())
        })
        .collect();
    if !missing.is_empty() {
        if selected.is_none() && values.is_empty() {
            return Err(ProfileError::Missing {
                name: name.to_string(),
                searched,
            });
        }
        return Err(ProfileError::Incomplete { source, missing });
    }

    Ok(Profile {
        url: values.remove("url").expect("checked above"),
        username: values.remove("username").expect("checked above"),
        password: values.remove("password").expect("checked above"),
        app_key: values.remove("app_key"),
        extra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

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
                password: password.to_string(),
                app_key: Some(format!("key-{password}")),
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

    fn temp(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "twaco-profile-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
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
        let root = temp("precedence");
        let home = root.join("home");
        let local = root.join("repo/.twaco/profiles/default.toml");
        let global = home.join(".twaco/profiles/default.toml");
        write(&global, "https://global/", "global-user", "global-pass");
        write(&local, "https://local/", "local-user", "local-pass");

        let mut env = BTreeMap::new();
        env.insert("TWX_USERNAME".to_string(), "legacy-user".to_string());
        env.insert("TWACO_USERNAME".to_string(), "override-user".to_string());
        let profile = load_from(&root.join("repo"), Some(&home), "default", &env).unwrap();
        assert_eq!(profile.url, "https://local/");
        assert_eq!(profile.username, "override-user");
        assert_eq!(profile.password, "local-pass");

        std::fs::remove_file(local).unwrap();
        let profile =
            load_from(&root.join("repo"), Some(&home), "default", &BTreeMap::new()).unwrap();
        assert_eq!(profile.url, "https://global/");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn free_form_values_are_flattened_and_twaco_environment_wins() {
        let root = temp("extra");
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
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn environment_alone_can_form_a_profile() {
        let root = temp("environment");
        let env = BTreeMap::from([
            (
                "TWX_URL".to_string(),
                "http://server/Thingworx/".to_string(),
            ),
            ("TWX_USERNAME".to_string(), "user".to_string()),
            ("TWX_PASSWORD".to_string(), "pass".to_string()),
        ]);
        assert_eq!(
            load_from(&root, None, "default", &env).unwrap().username,
            "user"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_profile_name_cannot_escape_its_directory() {
        assert!(matches!(
            load_from(Path::new("."), None, "../secret", &BTreeMap::new()),
            Err(ProfileError::InvalidName(_))
        ));
    }
}
