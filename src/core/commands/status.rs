//! The command policy around comparing entities with the server.

use super::{lock_workspace, Access, Effects, Notices};
use crate::core::baseline::{self, Baseline};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::progress::Progress;
use crate::core::{lock, profile, push, status, workspace};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusTarget {
    Names(Vec<String>),
    All,
}

/// The arguments that affect an entity status read and optional baseline record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusRequest {
    pub target: StatusTarget,
    pub project: Option<String>,
    pub record: bool,
    pub profile: String,
    pub lock_label: &'static str,
    /// The CLI refuses unreadable workspace files for every status request. MCP preserves its
    /// read-only partial result, but refuses it when it would record a baseline.
    pub refuse_unreadable: bool,
    /// MCP turns failed reads during a record into one tool error; the CLI keeps its per-read
    /// diagnostics and exit status.
    pub refuse_record_failures: bool,
}

#[derive(Debug)]
pub struct StatusOutcome {
    pub statuses: Vec<status::EntityStatus>,
    pub failures: Vec<String>,
    pub unreadable: Vec<String>,
    pub recorded: Option<usize>,
    effects: Effects,
}

impl StatusOutcome {
    pub const fn effects(&self) -> Effects {
        self.effects
    }
}

#[derive(Debug)]
pub enum StatusCommandError {
    Lock(lock::LockError),
    Target,
    Project(String),
    Resolve(workspace::WorkspaceError),
    Unreadable(Vec<String>),
    Profile(profile::ProfileError),
    Baseline(baseline::BaselineError),
    RecordFailures(Vec<String>),
}

impl fmt::Display for StatusCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(f),
            Self::Target => write!(f, "name an entity, or pass --all"),
            Self::Project(name) => write!(f, "this solution has no project named {name}"),
            Self::Resolve(error) => error.fmt(f),
            Self::Unreadable(items) => write!(f, "{}", items.join("\n")),
            Self::Profile(error) => error.fmt(f),
            Self::Baseline(error) => error.fmt(f),
            Self::RecordFailures(failures) => write!(
                f,
                "nothing was recorded: {} entity read(s) failed: {}",
                failures.len(),
                failures.join("; ")
            ),
        }
    }
}

impl std::error::Error for StatusCommandError {}

impl Coded for StatusCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Target => ErrorCode::InvalidArguments,
            Self::Project(_) => ErrorCode::InvalidArguments,
            Self::Resolve(error) => error.code(),
            Self::Unreadable(_) => ErrorCode::InvalidData,
            Self::Profile(error) => error.code(),
            Self::Baseline(error) => error.code(),
            Self::RecordFailures(_) => ErrorCode::Unclassified,
        }
    }
}

/// Compare the requested entities. Recording locks before discovery so the baseline describes
/// precisely the workspace this invocation observed.
pub fn execute<R, F>(
    solution: &Solution,
    request: &StatusRequest,
    open: F,
    notices: &mut Notices,
    progress: &dyn Progress,
) -> Result<StatusOutcome, StatusCommandError>
where
    R: push::Remote + Sync,
    F: FnOnce(profile::Profile) -> R,
{
    let _lock = if request.record {
        Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(StatusCommandError::Lock)?,
        )
    } else {
        None
    };
    let found = workspace::discover(solution);
    if request.refuse_unreadable && !found.unreadable.is_empty() {
        return Err(StatusCommandError::Unreadable(found.unreadable));
    }
    let mut pool = found.entities;
    if let Some(project) = &request.project {
        if solution.project(project).is_none() {
            return Err(StatusCommandError::Project(project.clone()));
        }
        pool.retain(|entity| &entity.found_under == project);
    }
    let chosen = match &request.target {
        StatusTarget::All => pool,
        StatusTarget::Names(names) => {
            if names.is_empty() {
                return Err(StatusCommandError::Target);
            }
            names
                .iter()
                .map(|name| workspace::resolve(&pool, name).cloned())
                .collect::<Result<Vec<_>, _>>()
                .map_err(StatusCommandError::Resolve)?
        }
    };
    let profile =
        profile::load(&solution.root, &request.profile).map_err(StatusCommandError::Profile)?;
    let mut baseline = Baseline::load(&solution.root).map_err(StatusCommandError::Baseline)?;
    let (statuses, failures) =
        status::compute_with_progress(&open(profile), &baseline, &chosen, progress);
    if request.record && !failures.is_empty() {
        if request.refuse_record_failures {
            return Err(StatusCommandError::RecordFailures(failures));
        }
        return Ok(StatusOutcome {
            statuses,
            failures,
            unreadable: found.unreadable,
            recorded: None,
            effects: Effects::new(Access::Read, Access::Read),
        });
    }
    let recorded = if request.record {
        let recorded = status::record_matching(&mut baseline, &statuses);
        baseline
            .write(&solution.root)
            .map_err(StatusCommandError::Baseline)?;
        Some(recorded)
    } else {
        None
    };
    let workspace = if request.record {
        Access::Write
    } else {
        Access::Read
    };
    Ok(StatusOutcome {
        statuses,
        failures,
        unreadable: found.unreadable,
        recorded,
        effects: Effects::new(workspace, Access::Read),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;

    #[test]
    fn a_read_only_status_needs_no_lock_but_recording_takes_one_before_discovery() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-status-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = StatusRequest {
            target: StatusTarget::All,
            project: None,
            record: false,
            profile: "default".to_string(),
            lock_label: "status",
            refuse_unreadable: false,
            refuse_record_failures: false,
        };
        assert!(matches!(
            execute::<crate::core::server::Client, _>(
                &solution,
                &request,
                crate::core::server::Client::new,
                &mut Notices::default(),
                &crate::core::progress::NONE
            ),
            Err(StatusCommandError::Profile(_))
        ));
        let mut recording = request;
        recording.record = true;
        assert!(matches!(
            execute::<crate::core::server::Client, _>(
                &solution,
                &recording,
                crate::core::server::Client::new,
                &mut Notices::default(),
                &crate::core::progress::NONE
            ),
            Err(StatusCommandError::Lock(_))
        ));
        drop(held);
    }
}
