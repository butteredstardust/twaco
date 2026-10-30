//! Command policy for `new <kind> <name>`.
use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::{entity, lock};

#[derive(Clone, Debug)]
pub struct Request {
    pub request: entity::new::NewRequest,
    pub mode: Mode,
    pub lock_label: &'static str,
    /// Fetch a platform ThingTemplate or ThingShape the request names into the platform cache from
    /// this profile's server, when neither the solution nor the cache has it. `None` fetches nothing.
    pub fetch_profile: Option<String>,
}
#[derive(Debug)]
pub struct Outcome {
    pub plan: entity::new::Plan,
    pub applied: bool,
    /// What fetching into the platform cache did, one note each.
    pub fetched: Vec<String>,
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); no adapter reads it yet"
    )]
    pub(crate) effects: Effects,
}
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Config(config::ConfigError),
    #[error("{0}")]
    New(entity::new::NewError),
}
impl Coded for CommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::New(error) => error.code(),
        }
    }
}
pub fn execute(
    solution: &Solution,
    request: &Request,
    notices: &mut Notices,
) -> Result<Outcome, CommandError> {
    let missing = missing_platform_names(solution, &request.request);
    let fetch = request
        .fetch_profile
        .as_deref()
        .filter(|_| !missing.is_empty());
    match request.mode {
        Mode::Plan => {
            // The cache is the workspace's: filling it takes the lock, even for a plan, and the
            // effects say so. A plan with nothing to fetch takes no lock and writes nothing.
            let fetched = match fetch {
                Some(profile) => {
                    let lock = lock_workspace(solution, request.lock_label, notices)
                        .map_err(CommandError::Lock)?;
                    fetch_missing(solution, &missing, profile, &lock)
                }
                None => Vec::new(),
            };
            let effects = if fetch.is_some() {
                Effects::new(Access::Write, Access::Read)
            } else {
                Effects::new(Access::Read, Access::None)
            };
            let plan = entity::new::plan(solution, &request.request)
                .map_err(|error| refused(error, &fetched, notices))?;
            Ok(Outcome {
                plan,
                applied: false,
                fetched,
                effects,
            })
        }
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(CommandError::Lock)?;
            let fetched = match fetch {
                Some(profile) => fetch_missing(solution, &missing, profile, &lock),
                None => Vec::new(),
            };
            let path = solution.root.join(config::CONFIG_FILE);
            // A bare solution stays bare until this apply writes its twaco.toml.
            let reloaded = if solution.projects.is_empty() && !path.exists() {
                Solution::bare(&solution.root)
            } else {
                Solution::load(&path).map_err(CommandError::Config)?
            };
            let plan = entity::new::plan(&reloaded, &request.request)
                .map_err(|error| refused(error, &fetched, notices))?;
            entity::new::apply(&reloaded, &plan, &lock).map_err(CommandError::New)?;
            Ok(Outcome {
                plan,
                applied: true,
                fetched,
                effects: Effects::new(
                    Access::Write,
                    if fetch.is_some() {
                        Access::Read
                    } else {
                        Access::None
                    },
                ),
            })
        }
    }
}

/// A refused request, with what fetching into the platform cache said: often why a template it
/// names is unknown.
fn refused(
    error: entity::new::NewError,
    fetched: &[String],
    notices: &mut Notices,
) -> CommandError {
    notices.0.extend(fetched.iter().cloned());
    CommandError::New(error)
}

/// The ThingTemplates and ThingShapes `request` names that neither the solution nor the
/// platform cache has, so a platform one (FileRepository, RemoteThing) can be fetched before
/// anything in the repository uses it.
fn missing_platform_names(
    solution: &Solution,
    request: &entity::new::NewRequest,
) -> Vec<(&'static str, String)> {
    use entity::new::Kind;
    let mut wanted: Vec<(&'static str, String)> = Vec::new();
    match request.kind {
        Kind::Thing => wanted.extend(
            request
                .template
                .iter()
                .map(|t| ("ThingTemplates", t.clone())),
        ),
        Kind::ThingTemplate => {
            wanted.extend(request.base.iter().map(|t| ("ThingTemplates", t.clone())))
        }
        _ => {}
    }
    if matches!(request.kind, Kind::Thing | Kind::ThingTemplate) {
        wanted.extend(request.shapes.iter().map(|s| ("ThingShapes", s.clone())));
    }
    if wanted.is_empty() {
        return wanted;
    }
    let defined = crate::core::workspace::entities(solution);
    let cache = crate::core::types::platform::cached(solution).unwrap_or_default();
    wanted
        .into_iter()
        .filter(|(collection, name)| {
            !defined
                .iter()
                .any(|e| e.info.collection == *collection && &e.info.name == name)
                && !match *collection {
                    "ThingTemplates" => cache.templates.contains_key(name),
                    _ => cache.shapes.contains_key(name),
                }
        })
        .collect()
}

/// Fetch `missing` into the platform cache, holding the workspace lock. Best effort: what cannot
/// be fetched is left to the request's own refusal, and the notes say why.
fn fetch_missing(
    solution: &Solution,
    missing: &[(&'static str, String)],
    profile: &str,
    held: &lock::WorkspaceLock,
) -> Vec<String> {
    let remote = match crate::core::profile::load(&solution.root, profile) {
        Ok(profile) => crate::core::server::Client::new(profile),
        Err(error) => {
            return vec![format!(
                "{} not fetched from a server: {error}",
                missing
                    .iter()
                    .map(|(c, n)| format!("{c}/{n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )]
        }
    };
    let mut notes = Vec::new();
    for (collection, name) in missing {
        match crate::core::types::platform::cache_one(&remote, solution, collection, name, held) {
            Ok(true) => notes.push(format!(
                "fetched {collection}/{name} into the platform cache"
            )),
            Ok(false) => {}
            Err(error) => notes.push(format!("{collection}/{name} not fetched: {error}")),
        }
    }
    notes
}
