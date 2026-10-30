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
}
#[derive(Debug)]
pub struct Outcome {
    pub plan: entity::new::Plan,
    pub applied: bool,
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
    match request.mode {
        Mode::Plan => Ok(Outcome {
            plan: entity::new::plan(solution, &request.request).map_err(CommandError::New)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        }),
        Mode::Apply => {
            let lock = lock_workspace(solution, request.lock_label, notices)
                .map_err(CommandError::Lock)?;
            let path = solution.root.join(config::CONFIG_FILE);
            // A bare solution stays bare until this apply writes its twaco.toml.
            let reloaded = if solution.projects.is_empty() && !path.exists() {
                Solution::bare(&solution.root)
            } else {
                Solution::load(&path).map_err(CommandError::Config)?
            };
            let plan = entity::new::plan(&reloaded, &request.request).map_err(CommandError::New)?;
            entity::new::apply(&reloaded, &plan, &lock).map_err(CommandError::New)?;
            Ok(Outcome {
                plan,
                applied: true,
                effects: Effects::new(Access::Write, Access::None),
            })
        }
    }
}

/// Fetch into the platform cache each ThingTemplate or ThingShape `request` names that neither the
/// solution nor the cache has, so a platform one (FileRepository, RemoteThing) can be named before
/// anything in the repository uses it. Best effort: what cannot be fetched is left to the request's
/// own refusal, and the notes say why.
pub fn fetch_platform_names(
    solution: &Solution,
    request: &entity::new::NewRequest,
    profile: &str,
) -> Vec<String> {
    use entity::new::Kind;
    let mut wanted = Vec::new();
    match request.kind {
        Kind::Thing => wanted.extend(request.template.iter().map(|t| ("ThingTemplates", t))),
        Kind::ThingTemplate => wanted.extend(request.base.iter().map(|t| ("ThingTemplates", t))),
        _ => {}
    }
    if matches!(request.kind, Kind::Thing | Kind::ThingTemplate) {
        wanted.extend(request.shapes.iter().map(|s| ("ThingShapes", s)));
    }
    let defined = crate::core::workspace::entities(solution);
    let cache = crate::core::types::platform::cached(solution).unwrap_or_default();
    let missing: Vec<_> = wanted
        .into_iter()
        .filter(|(collection, name)| {
            !defined
                .iter()
                .any(|e| e.info.collection == *collection && &e.info.name == *name)
                && !match *collection {
                    "ThingTemplates" => cache.templates.contains_key(*name),
                    _ => cache.shapes.contains_key(*name),
                }
        })
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
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
        match crate::core::types::platform::cache_one(&remote, solution, collection, name) {
            Ok(true) => notes.push(format!(
                "fetched {collection}/{name} into the platform cache"
            )),
            Ok(false) => {}
            Err(error) => notes.push(format!("{collection}/{name} not fetched: {error}")),
        }
    }
    notes
}
