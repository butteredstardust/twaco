//! The command policy around extracting entity sidecars.

use super::{lock_workspace, Access, Effects, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{lock, workflow, workspace};
use std::fmt;

/// The entities an extract request selects before it changes any files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractTarget {
    pub project: Option<String>,
    pub entities: Vec<String>,
    pub all: bool,
    pub reject_entities_with_all: bool,
    pub missing_target: &'static str,
}

/// The arguments that affect extraction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractRequest {
    pub target: ExtractTarget,
    /// The lock-holder label the calling surface has historically shown.
    pub lock_label: &'static str,
}

/// One completed extraction, before either adapter projects it to its wire format.
#[derive(Debug)]
pub struct ExtractOutcome {
    pub report: workflow::ExtractOutcome,
}

impl ExtractOutcome {
    /// Extraction always changes or may change the workspace.
    pub const fn effects(&self) -> Effects {
        Effects::new(Access::Write, Access::None)
    }
}

/// A failure before an extraction outcome could be produced.
#[derive(Debug)]
pub enum ExtractCommandError {
    Lock(lock::LockError),
    Project(String),
    Target(String),
    Resolve(workspace::WorkspaceError),
}

impl fmt::Display for ExtractCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(why) => why.fmt(f),
            Self::Resolve(why) => why.fmt(f),
            Self::Project(name) => write!(f, "this solution has no project named {name}"),
            Self::Target(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ExtractCommandError {}

impl Coded for ExtractCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
            Self::Project(_) | Self::Target(_) => ErrorCode::InvalidArguments,
            Self::Resolve(why) => why.code(),
        }
    }
}

/// Extract sidecars. The lock is held before discovery, so the set of files read and written is
/// protected together.
pub fn execute(
    solution: &Solution,
    request: &ExtractRequest,
    notices: &mut Notices,
) -> Result<ExtractOutcome, ExtractCommandError> {
    let lock =
        lock_workspace(solution, request.lock_label, notices).map_err(ExtractCommandError::Lock)?;
    let (chosen, unreadable, named) = select(solution, &request.target)?;
    Ok(ExtractOutcome {
        report: workflow::extract(solution, &chosen, &unreadable, named, &lock),
    })
}

fn select(
    solution: &Solution,
    target: &ExtractTarget,
) -> Result<(Vec<workspace::EntityFile>, Vec<String>, bool), ExtractCommandError> {
    let found = workspace::discover(solution);
    let mut pool = found.entities;
    if let Some(project) = &target.project {
        if solution.project(project).is_none() {
            return Err(ExtractCommandError::Project(project.clone()));
        }
        pool.retain(|entity| &entity.found_under == project);
    }
    if target.all && !(target.reject_entities_with_all && !target.entities.is_empty()) {
        return Ok((pool, found.unreadable, false));
    }
    if target.all {
        return Err(ExtractCommandError::Target(
            "name an entity or pass all: true, not both".to_string(),
        ));
    }
    if target.entities.is_empty() {
        return Err(ExtractCommandError::Target(
            target.missing_target.to_string(),
        ));
    }
    let chosen = target
        .entities
        .iter()
        .map(|name| workspace::resolve(&pool, name).cloned())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ExtractCommandError::Resolve)?;
    Ok((chosen, found.unreadable, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;

    #[test]
    fn extraction_locks_before_it_discovers_the_target() {
        let root =
            std::env::temp_dir().join(format!("twaco-command-extract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = ExtractRequest {
            target: ExtractTarget {
                project: None,
                entities: Vec::new(),
                all: true,
                reject_entities_with_all: false,
                missing_target: "name an entity, or pass --all",
            },
            lock_label: "extract",
        };
        let error = execute(&solution, &request, &mut Notices::default()).unwrap_err();
        assert!(matches!(error, ExtractCommandError::Lock(_)));
        drop(held);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn target_refusals_keep_their_codes() {
        let error = ExtractCommandError::Target("missing target".to_string());
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        let error = ExtractCommandError::Resolve(workspace::WorkspaceError::UnknownEntity {
            name: "T".to_string(),
        });
        assert_eq!(error.code(), ErrorCode::UnknownEntity);
    }
}
