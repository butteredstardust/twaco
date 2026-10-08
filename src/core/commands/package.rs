//! The command policy around writing one package file.

use super::{Access, Effects};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{package, workspace};
use std::fmt;
use std::path::PathBuf;

/// The kind of offline package to build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageAction {
    Bundle { part: package::Part },
    SourceControl,
    Extension { editable: bool },
}

/// The selected package and explicitly named output file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageRequest {
    pub action: PackageAction,
    pub project: Option<String>,
    pub out: PathBuf,
    pub force: bool,
}

/// A completed package write before an adapter formats it.
#[derive(Debug)]
pub struct PackageOutcome {
    pub out: PathBuf,
    pub bytes: usize,
    pub summary: String,
    /// What was built, from the build that was written, for an adapter that reports more than the
    /// one-line summary.
    pub detail: PackageDetail,
    pub effects: Effects,
}

/// What a package holds.
#[derive(Debug)]
pub enum PackageDetail {
    Bundle {
        entities: usize,
        files: usize,
    },
    SourceControl {
        entities: usize,
    },
    ProjectExtension {
        editable: bool,
        version: String,
        entities: usize,
    },
    SolutionExtension {
        editable: bool,
        version: String,
        projects: Vec<(String, usize)>,
    },
}

/// A failure before a package could be written.
#[derive(Debug)]
pub enum PackageCommandError {
    Exists(PathBuf),
    Package(package::PackageError),
    Create { path: PathBuf, why: std::io::Error },
    Write(workspace::WorkspaceError),
}

impl fmt::Display for PackageCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exists(path) => {
                write!(f, "{} exists; pass --force to replace it", path.display())
            }
            Self::Package(why) => why.fmt(f),
            Self::Create { path, why } => write!(f, "{}: {why}", path.display()),
            Self::Write(why) => why.fmt(f),
        }
    }
}

impl std::error::Error for PackageCommandError {}

impl Coded for PackageCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Exists(_) => ErrorCode::AlreadyExists,
            Self::Package(why) => why.code(),
            Self::Create { .. } | Self::Write(_) => ErrorCode::IoError,
        }
    }
}

/// Build a package and atomically write the explicitly named output file. This does not lock the
/// solution: callers chose a single output file rather than a solution-tree mutation.
pub fn execute(
    solution: &Solution,
    request: &PackageRequest,
) -> Result<PackageOutcome, PackageCommandError> {
    if request.out.exists() && !request.force {
        return Err(PackageCommandError::Exists(request.out.clone()));
    }
    let project = request.project.as_deref();
    let (bytes, summary, detail) = match request.action {
        PackageAction::Bundle { part } => {
            let built =
                package::bundle(solution, project, part).map_err(PackageCommandError::Package)?;
            let count: usize = built.entities.values().sum();
            (
                built.bytes,
                format!("{count} entities from {} files", built.files),
                PackageDetail::Bundle {
                    entities: count,
                    files: built.files,
                },
            )
        }
        PackageAction::SourceControl => {
            let (bytes, count) =
                package::source_control(solution, project).map_err(PackageCommandError::Package)?;
            (
                bytes,
                format!("{count} entities"),
                PackageDetail::SourceControl { entities: count },
            )
        }
        PackageAction::Extension { editable } => {
            let meta = package::Metadata::from_solution(solution);
            let kind = if editable { "editable" } else { "non-editable" };
            match project {
                Some(project) => {
                    let (bytes, count) = package::extension(solution, project, editable, &meta)
                        .map_err(PackageCommandError::Package)?;
                    (
                        bytes,
                        format!("{kind} {project} {}, {count} entities", meta.version),
                        PackageDetail::ProjectExtension {
                            editable,
                            version: meta.version.clone(),
                            entities: count,
                        },
                    )
                }
                None => {
                    let (bytes, counts) = package::solution_extensions(solution, editable, &meta)
                        .map_err(PackageCommandError::Package)?;
                    let each = counts
                        .iter()
                        .map(|(project, count)| format!("{project} ({count})"))
                        .collect::<Vec<_>>();
                    (
                        bytes,
                        format!(
                            "{kind} {} {}: {}",
                            solution.solution.name,
                            meta.version,
                            each.join(", ")
                        ),
                        PackageDetail::SolutionExtension {
                            editable,
                            version: meta.version.clone(),
                            projects: counts
                                .iter()
                                .map(|(project, count)| (project.to_string(), *count))
                                .collect(),
                        },
                    )
                }
            }
        }
    };
    if let Some(folder) = request
        .out
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        std::fs::create_dir_all(folder).map_err(|why| PackageCommandError::Create {
            path: folder.to_path_buf(),
            why,
        })?;
    }
    workspace::write_entity(&request.out, &bytes).map_err(PackageCommandError::Write)?;
    Ok(PackageOutcome {
        out: request.out.clone(),
        bytes: bytes.len(),
        summary,
        detail,
        effects: Effects::new(Access::Write, Access::None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-package-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\nroot = \".\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("Things/A.xml"),
            "<Entities><Things><Thing name=\"A\" projectName=\"P\"/></Things></Entities>",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution)
    }

    #[test]
    fn named_package_output_does_not_take_the_workspace_lock_and_force_replaces_it() {
        let (_dir, root, solution) = setup();
        let out = root.join("release.zip");
        std::fs::write(&out, b"old").unwrap();
        let request = |force| PackageRequest {
            action: PackageAction::Bundle {
                part: package::Part::All,
            },
            project: None,
            out: out.clone(),
            force,
        };
        let held = lock::acquire_for(&solution, "holder").unwrap();
        let error = execute(&solution, &request(false)).unwrap_err();
        assert_eq!(error.code(), ErrorCode::AlreadyExists);
        let outcome = execute(&solution, &request(true)).unwrap();
        assert_eq!(outcome.effects, Effects::new(Access::Write, Access::None));
        assert_ne!(std::fs::read(&out).unwrap(), b"old");
        drop(held);
    }

    #[test]
    fn what_is_reported_comes_from_the_one_build_that_was_written() {
        let (_dir, root, solution) = setup();
        let run = |action: PackageAction, name: &str| {
            execute(
                &solution,
                &PackageRequest {
                    action,
                    project: None,
                    out: root.join(name),
                    force: true,
                },
            )
            .unwrap()
        };
        let bundle = run(
            PackageAction::Bundle {
                part: package::Part::All,
            },
            "bundle.xml",
        );
        let PackageDetail::Bundle { entities, files } = bundle.detail else {
            panic!("a bundle reports a bundle");
        };
        assert_eq!((entities, files), (1, 1));
        assert_eq!(
            bundle.summary,
            format!("{entities} entities from {files} files")
        );
        assert_eq!(
            bundle.bytes,
            std::fs::read(root.join("bundle.xml")).unwrap().len()
        );

        let source_control = run(PackageAction::SourceControl, "source.zip");
        let PackageDetail::SourceControl { entities } = source_control.detail else {
            panic!("a source-control zip reports its entities");
        };
        assert_eq!(entities, 1);

        let extension = run(PackageAction::Extension { editable: true }, "extension.zip");
        let PackageDetail::SolutionExtension {
            editable,
            version,
            projects,
        } = extension.detail
        else {
            panic!("a solution without a named project reports every project");
        };
        assert!(editable);
        assert_eq!(projects, vec![("P".to_string(), 1)]);
        assert!(
            extension.summary.contains(&version),
            "{}",
            extension.summary
        );
    }
}
