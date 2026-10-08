//! Diagnostic logs: off unless a flag or an environment variable asks for them.
//!
//! WARNING: Logs go to stderr or to a file. Never write them to stdout, because MCP speaks
//! JSON-RPC there. Never log headers, bodies, profiles, environment variables or MCP tool
//! `arguments`. Scrub a server URL with the client's secrets first.
//!
//! The purpose is to show what twaco did when a command misbehaves, without changing normal
//! output. A bad option never stops a command: [`init`] returns a warning and logs stay off.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

/// Filter directive: `--log` beats this variable.
pub const LOG_VARIABLE: &str = "TWACO_LOG";
/// Log file: `--log-file` beats this variable.
pub const LOG_FILE_VARIABLE: &str = "TWACO_LOG_FILE";

const LEVELS: [&str; 6] = ["error", "warn", "info", "debug", "trace", "off"];

/// What the flags and variables ask for.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// A filter directive, as given.
    pub filter: String,
    /// Append to this file; stderr when absent.
    pub file: Option<PathBuf>,
}

/// Decide what to log. A flag beats its variable. An empty value counts as unset.
/// `--log-file` alone means `debug`. Returns `None` when logs stay off.
pub fn resolve(
    log: Option<&str>,
    log_file: Option<&str>,
    variable: &dyn Fn(&str) -> Option<String>,
) -> Option<Plan> {
    let given = |flag: Option<&str>, name: &str| {
        flag.map(str::to_string)
            .or_else(|| variable(name))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let filter = given(log, LOG_VARIABLE);
    let file = given(log_file, LOG_FILE_VARIABLE).map(PathBuf::from);
    match (filter, file) {
        (None, None) => None,
        (Some(filter), file) => Some(Plan { filter, file }),
        (None, file) => Some(Plan {
            filter: "debug".to_string(),
            file,
        }),
    }
}

/// Build the filter. A bare level applies to the `twaco` target only, so dependencies stay quiet.
pub fn filter_of(spec: &str) -> Result<EnvFilter, String> {
    let spec = spec.trim();
    let directive = if LEVELS.contains(&spec.to_ascii_lowercase().as_str()) {
        format!("twaco={}", spec.to_ascii_lowercase())
    } else {
        spec.to_string()
    };
    EnvFilter::try_new(&directive).map_err(|error| error.to_string())
}

/// Install the global subscriber, if the flags or variables ask for one.
///
/// Returns a warning for an invalid filter or an unopenable file. The caller prints it and
/// continues. Call once, before the command runs.
pub fn init(log: Option<&str>, log_file: Option<&str>) -> Option<String> {
    let plan = resolve(log, log_file, &|name| std::env::var(name).ok())?;
    install(&plan).err()
}

fn install(plan: &Plan) -> Result<(), String> {
    let filter = filter_of(&plan.filter)
        .map_err(|why| format!("ignoring the log filter {:?}: {why}", plan.filter))?;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true);
    let set = match &plan.file {
        Some(path) => {
            let file = open(path)
                .map_err(|why| format!("ignoring the log file {}: {why}", path.display()))?;
            if is_stdout(&file) {
                return Err(format!(
                    "ignoring the log file {}: it is standard output",
                    path.display()
                ));
            }
            tracing::subscriber::set_global_default(builder.with_writer(Mutex::new(file)).finish())
        }
        None => {
            tracing::subscriber::set_global_default(builder.with_writer(std::io::stderr).finish())
        }
    };
    set.map_err(|why| format!("logs are already set up: {why}"))
}

/// Tell whether `file` is the same file as standard output. Logs on stdout break MCP messages.
#[cfg(unix)]
fn is_stdout(file: &std::fs::File) -> bool {
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;
    let Ok(stdout) = std::io::stdout().as_fd().try_clone_to_owned() else {
        return false;
    };
    let (Ok(mine), Ok(theirs)) = (file.metadata(), std::fs::File::from(stdout).metadata()) else {
        return false;
    };
    mine.dev() == theirs.dev() && mine.ino() == theirs.ino()
}

/// Windows has no check: the stdout comparison needs Unix device and inode numbers.
#[cfg(not(unix))]
fn is_stdout(_file: &std::fs::File) -> bool {
    false
}

fn open(path: &Path) -> std::io::Result<std::fs::File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// Test helper: run `f` and return the log text recorded meanwhile, at `trace`.
///
/// One global subscriber serves every test, because a subscriber set per thread races with
/// tracing's callsite cache when tests run in parallel. While a capture runs, events from other
/// test threads land in the text too. Assert on the presence of a line, not on its absence.
#[cfg(test)]
pub(crate) fn captured<R>(f: impl FnOnce() -> R) -> (R, String) {
    use std::sync::Once;
    static SERIAL: Mutex<()> = Mutex::new(());
    static RECORD: Mutex<Option<Vec<u8>>> = Mutex::new(None);
    static INSTALL: Once = Once::new();

    struct Sink;
    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if let Some(record) = RECORD.lock().unwrap().as_mut() {
                record.extend_from_slice(bytes);
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    INSTALL.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(|| Sink)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("no other global subscriber");
    });
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *RECORD.lock().unwrap() = Some(Vec::new());
    let result = f();
    let bytes = RECORD.lock().unwrap().take().unwrap_or_default();
    (result, String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn nothing_asked_means_logs_stay_off() {
        assert_eq!(resolve(None, None, &none), None);
        let blank = |_: &str| Some("  ".to_string());
        assert_eq!(resolve(None, None, &blank), None);
    }

    #[test]
    fn a_log_file_alone_means_debug() {
        let plan = resolve(None, Some("x.log"), &none).unwrap();
        assert_eq!(plan.filter, "debug");
        assert_eq!(plan.file, Some(PathBuf::from("x.log")));
    }

    #[test]
    fn a_flag_beats_its_variable() {
        let variable = |name: &str| match name {
            LOG_VARIABLE => Some("trace".to_string()),
            LOG_FILE_VARIABLE => Some("env.log".to_string()),
            _ => None,
        };
        let plan = resolve(Some("warn"), Some("flag.log"), &variable).unwrap();
        assert_eq!(plan.filter, "warn");
        assert_eq!(plan.file, Some(PathBuf::from("flag.log")));
        let plan = resolve(None, None, &variable).unwrap();
        assert_eq!(plan.filter, "trace");
        assert_eq!(plan.file, Some(PathBuf::from("env.log")));
    }

    #[test]
    fn a_bare_level_applies_to_the_twaco_target_only() {
        assert_eq!(filter_of("debug").unwrap().to_string(), "twaco=debug");
        assert_eq!(filter_of("DEBUG").unwrap().to_string(), "twaco=debug");
        assert_eq!(
            filter_of("twaco::core::server=trace").unwrap().to_string(),
            "twaco::core::server=trace"
        );
    }

    #[test]
    fn an_invalid_filter_is_an_error() {
        assert!(filter_of("twaco=loud").is_err());
    }
}
