//! The command policy around synchronising sidecars into entity files.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{lock, workflow, workspace};

/// The entities a sync request selects before it changes any files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncTarget {
    pub project: Option<String>,
    pub entities: Vec<String>,
    pub all: bool,
    pub reject_entities_with_all: bool,
    pub missing_target: &'static str,
}

/// The arguments that affect a sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncRequest {
    pub target: SyncTarget,
    pub mode: Mode,
    pub allow_structural: bool,
    pub relayout: bool,
    /// The lock-holder label the calling surface has historically shown.
    pub lock_label: &'static str,
}

/// One completed sync, before either adapter projects it to its wire format.
#[derive(Debug)]
pub struct SyncOutcome {
    pub report: workflow::SyncOutcome,
    effects: Effects,
}

impl SyncOutcome {
    /// The access this sync used or would use.
    pub const fn effects(&self) -> Effects {
        self.effects
    }
}

/// A failure before a sync outcome could be produced.
#[derive(Debug, thiserror::Error)]
pub enum SyncCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("this solution has no project named {0}")]
    Project(String),
    #[error("{0}")]
    Target(String),
    #[error("{0}")]
    Resolve(workspace::WorkspaceError),
}

impl Coded for SyncCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
            Self::Project(_) | Self::Target(_) => ErrorCode::InvalidArguments,
            Self::Resolve(why) => why.code(),
        }
    }
}

/// Synchronise the selected entities. An apply takes the lock before discovery, while a plan
/// reads without taking it.
pub fn execute(
    solution: &Solution,
    request: &SyncRequest,
    notices: &mut Notices,
) -> Result<SyncOutcome, SyncCommandError> {
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(SyncCommandError::Lock)?,
        ),
    };
    let (chosen, unreadable, named) = select(solution, &request.target)?;
    let report = workflow::sync(
        solution,
        &chosen,
        &unreadable,
        workflow::SyncOptions {
            check: matches!(request.mode, Mode::Plan),
            allow_structural: request.allow_structural,
            relayout: request.relayout,
            named,
        },
    );
    let workspace = match request.mode {
        Mode::Plan => Access::Read,
        Mode::Apply => Access::Write,
    };
    Ok(SyncOutcome {
        report,
        effects: Effects::new(workspace, Access::None),
    })
}

fn select(
    solution: &Solution,
    target: &SyncTarget,
) -> Result<(Vec<workspace::EntityFile>, Vec<String>, bool), SyncCommandError> {
    let found = workspace::discover(solution);
    let mut pool = found.entities;
    if let Some(project) = &target.project {
        if solution.project(project).is_none() {
            return Err(SyncCommandError::Project(project.clone()));
        }
        pool.retain(|entity| &entity.found_under == project);
    }
    if target.all && !(target.reject_entities_with_all && !target.entities.is_empty()) {
        return Ok((pool, found.unreadable, false));
    }
    if target.all {
        return Err(SyncCommandError::Target(
            "name an entity or pass all: true, not both".to_string(),
        ));
    }
    if target.entities.is_empty() {
        return Err(SyncCommandError::Target(target.missing_target.to_string()));
    }
    let chosen = target
        .entities
        .iter()
        .map(|name| workspace::resolve(&pool, name).cloned())
        .collect::<Result<Vec<_>, _>>()
        .map_err(SyncCommandError::Resolve)?;
    Ok((chosen, found.unreadable, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;
    use std::path::PathBuf;

    fn setup() -> (tempfile::TempDir, PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-sync-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("Things/T.xml"),
            "<Entities><Things><Thing name=\"T\" projectName=\"P\"/></Things></Entities>",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution)
    }

    fn request(mode: Mode) -> SyncRequest {
        SyncRequest {
            target: SyncTarget {
                project: None,
                entities: Vec::new(),
                all: true,
                reject_entities_with_all: false,
                missing_target: "name an entity, or pass --all",
            },
            mode,
            allow_structural: false,
            relayout: false,
            lock_label: "sync",
        }
    }

    #[test]
    fn a_plan_runs_without_the_workspace_lock_but_an_apply_takes_it_first() {
        let (_dir, root, solution) = setup();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let planned = execute(&solution, &request(Mode::Plan), &mut Notices::default()).unwrap();
        assert_eq!(planned.effects(), Effects::new(Access::Read, Access::None));
        let error = execute(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap_err();
        assert!(matches!(error, SyncCommandError::Lock(_)));
        drop(held);
    }

    #[test]
    fn an_apply_reports_what_taking_its_lock_recovered() {
        let (_dir, root, solution) = setup();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join(".twaco/.baseline.json.1.twaco-tmp"), b"half").unwrap();
        let mut notices = Notices::default();
        execute(&solution, &request(Mode::Apply), &mut notices).unwrap();
        assert_eq!(notices.lines().len(), 1, "{:?}", notices.lines());
        assert!(notices.lines()[0].contains("left by an interrupted write"));
    }
}
