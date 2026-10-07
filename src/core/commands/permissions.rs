//! The command policy around comparing and pushing entity permissions.

use super::status::StatusTarget;
use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::baseline::{self, Baseline};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{entity_carry, lock, permissions, profile, push, status, transaction, workspace};
use std::fmt;

/// The arguments that affect a permissions diff or push.
#[derive(Clone, Debug)]
pub struct PermissionsRequest {
    pub target: StatusTarget,
    pub project: Option<String>,
    pub mode: Mode,
    pub profile: String,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct PermissionsOutcome {
    pub report: permissions::Report,
    /// After a push, how many pushed entities now match the server and were recorded.
    pub recorded: Option<usize>,
    effects: Effects,
}

impl PermissionsOutcome {
    pub const fn effects(&self) -> Effects {
        self.effects
    }
}

/// A remote that can read and write permissions and export entities.
pub trait Remote: entity_carry::Remote + push::Remote + Sync {}

impl<T: entity_carry::Remote + push::Remote + Sync + ?Sized> Remote for T {}

#[derive(Debug)]
pub enum PermissionsCommandError {
    Lock(lock::LockError),
    Target,
    Project(String),
    Resolve(workspace::WorkspaceError),
    Unreadable(Vec<String>),
    Profile(profile::ProfileError),
    Baseline(baseline::BaselineError),
}

impl fmt::Display for PermissionsCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(f),
            Self::Target => write!(f, "name an entity, or pass --all"),
            Self::Project(name) => write!(f, "this solution has no project named {name}"),
            Self::Resolve(error) => error.fmt(f),
            Self::Unreadable(items) => write!(f, "{}", items.join("\n")),
            Self::Profile(error) => error.fmt(f),
            Self::Baseline(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for PermissionsCommandError {}

impl Coded for PermissionsCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Target | Self::Project(_) => ErrorCode::InvalidArguments,
            Self::Resolve(error) => error.code(),
            Self::Unreadable(_) => ErrorCode::InvalidData,
            Self::Profile(error) => error.code(),
            Self::Baseline(error) => error.code(),
        }
    }
}

/// Compare the chosen entities' permissions with the server's, and with `Apply` make them the
/// repository's. A push takes the workspace lock before discovery, because it records the
/// baseline of every pushed entity that then matches the server.
pub fn execute<R, F>(
    solution: &Solution,
    request: &PermissionsRequest,
    open: F,
    notices: &mut Notices,
) -> Result<PermissionsOutcome, PermissionsCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let apply = matches!(request.mode, Mode::Apply);
    let _lock = if apply {
        Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(PermissionsCommandError::Lock)?,
        )
    } else {
        None
    };
    let found = workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(PermissionsCommandError::Unreadable(found.unreadable));
    }
    let mut pool = found.entities;
    if let Some(project) = &request.project {
        if solution.project(project).is_none() {
            return Err(PermissionsCommandError::Project(project.clone()));
        }
        pool.retain(|entity| &entity.found_under == project);
    }
    let chosen = match &request.target {
        StatusTarget::All => pool,
        StatusTarget::Names(names) => {
            if names.is_empty() {
                return Err(PermissionsCommandError::Target);
            }
            names
                .iter()
                .map(|name| workspace::resolve(&pool, name).cloned())
                .collect::<Result<Vec<_>, _>>()
                .map_err(PermissionsCommandError::Resolve)?
        }
    };
    let profile = profile::load(&solution.root, &request.profile)
        .map_err(PermissionsCommandError::Profile)?;
    let remote = open(profile);
    let report = permissions::run(&remote, &chosen, apply);
    let pushed: Vec<_> = chosen
        .iter()
        .zip(&report.entities)
        .filter(|(_, entity)| entity.status == permissions::Status::Pushed)
        .map(|(file, _)| file.clone())
        .collect();
    let recorded = if apply && !pushed.is_empty() {
        let mut baseline =
            Baseline::load(&solution.root).map_err(PermissionsCommandError::Baseline)?;
        let (statuses, _) = status::compute(&remote, &baseline, &pushed);
        let recorded = status::record_matching(&mut baseline, &statuses);
        if recorded > 0 {
            baseline
                .write(&solution.root)
                .map_err(PermissionsCommandError::Baseline)?;
        }
        Some(recorded)
    } else {
        None
    };
    let wrote = report
        .entities
        .iter()
        .any(|entity| !entity.sets.is_empty() && entity.status != permissions::Status::Differs);
    Ok(PermissionsOutcome {
        report,
        effects: Effects::new(
            if recorded.unwrap_or(0) > 0 {
                Access::Write
            } else {
                Access::Read
            },
            if apply && wrote {
                Access::Write
            } else {
                Access::Read
            },
        ),
        recorded,
    })
}

/// The arguments of `permissions audit`.
#[derive(Clone, Debug)]
pub struct AuditRequest {
    pub project: Option<String>,
    /// The profile of the server to compare too; offline when absent.
    pub server: Option<String>,
}

#[derive(Debug)]
pub enum AuditCommandError {
    Audit(permissions::audit::AuditError),
    Profile(profile::ProfileError),
}

impl fmt::Display for AuditCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audit(error) => error.fmt(f),
            Self::Profile(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AuditCommandError {}

impl Coded for AuditCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Audit(error) => error.code(),
            Self::Profile(error) => error.code(),
        }
    }
}

/// Audit the policies offline, and with a profile against that server too. Read-only.
pub fn execute_audit<R, F>(
    solution: &Solution,
    request: &AuditRequest,
    open: F,
) -> Result<permissions::audit::AuditReport, AuditCommandError>
where
    R: permissions::server_audit::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    match &request.server {
        None => permissions::audit::audit(solution, request.project.as_deref())
            .map_err(AuditCommandError::Audit),
        Some(name) => {
            let profile =
                profile::load(&solution.root, name).map_err(AuditCommandError::Profile)?;
            let remote = open(profile);
            permissions::audit::audit_with(solution, request.project.as_deref(), Some(&remote))
                .map_err(AuditCommandError::Audit)
        }
    }
}

/// The arguments of `permissions apply`.
#[derive(Clone, Debug)]
pub struct ApplyRequest {
    pub project: Option<String>,
    pub mode: Mode,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct ApplyOutcome {
    pub plan: permissions::apply::ApplyPlan,
    /// Whether the files were written.
    pub applied: bool,
    effects: Effects,
}

impl ApplyOutcome {
    pub const fn effects(&self) -> Effects {
        self.effects
    }
}

#[derive(Debug)]
pub enum ApplyCommandError {
    Lock(lock::LockError),
    Plan(permissions::apply::ApplyError),
    Write(transaction::TransactionError),
}

impl fmt::Display for ApplyCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(f),
            Self::Plan(error) => error.fmt(f),
            Self::Write(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ApplyCommandError {}

impl Coded for ApplyCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Plan(error) => error.code(),
            Self::Write(error) => error.code(),
        }
    }
}

/// Plan writing each project's permission policy into its entity XML, and with `Apply` write it:
/// every changed file in one transaction, under the workspace lock taken before planning.
pub fn execute_apply(
    solution: &Solution,
    request: &ApplyRequest,
    notices: &mut Notices,
) -> Result<ApplyOutcome, ApplyCommandError> {
    let lock = match request.mode {
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(ApplyCommandError::Lock)?,
        ),
        Mode::Plan => None,
    };
    let plan = permissions::apply::plan(solution, request.project.as_deref())
        .map_err(ApplyCommandError::Plan)?;
    let Some(lock) = lock else {
        return Ok(ApplyOutcome {
            plan,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        });
    };
    let mut operation = transaction::Transaction::new(&solution.root, request.lock_label);
    for change in plan.changes() {
        operation
            .replace_file(&change.path, &change.before, change.after.clone())
            .map_err(ApplyCommandError::Write)?;
    }
    let wrote = !operation.is_empty();
    operation.apply(&lock).map_err(ApplyCommandError::Write)?;
    Ok(ApplyOutcome {
        plan,
        applied: true,
        effects: Effects::new(
            if wrote { Access::Write } else { Access::Read },
            Access::None,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_needs_no_lock_but_a_push_takes_one_before_anything_else() {
        let root =
            std::env::temp_dir().join(format!("twaco-command-permissions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = PermissionsRequest {
            target: StatusTarget::All,
            project: None,
            mode: Mode::Plan,
            profile: "default".to_string(),
            lock_label: "permissions",
        };
        assert!(matches!(
            execute::<crate::core::server::Client, _>(
                &solution,
                &request,
                crate::core::server::Client::new,
                &mut Notices::default()
            ),
            Err(PermissionsCommandError::Profile(_))
        ));
        let mut push = request;
        push.mode = Mode::Apply;
        assert!(matches!(
            execute::<crate::core::server::Client, _>(
                &solution,
                &push,
                crate::core::server::Client::new,
                &mut Notices::default()
            ),
            Err(PermissionsCommandError::Lock(_))
        ));
        drop(held);
        std::fs::remove_dir_all(root).unwrap();
    }
}
