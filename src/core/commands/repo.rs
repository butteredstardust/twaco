//! The command policy around file repositories.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::progress::{self, Progress};
use crate::core::{lock, profile, repo, workspace};
use std::path::PathBuf;

/// A repository operation requested by either adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoAction {
    List,
    Ls {
        repository: String,
        path: String,
        recursive: bool,
    },
    Get {
        repository: String,
        path: String,
        out: Option<PathBuf>,
        force: bool,
    },
    Status {
        repository: String,
    },
    Change {
        repository: String,
        change: repo::Change,
        mode: Mode,
    },
    Sync {
        repository: String,
        direction: repo::Direction,
        overwrite: bool,
        mode: Mode,
    },
}

/// The profile and operation used to access a file repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRequest {
    pub action: RepoAction,
    pub profile: String,
}

/// A completed repository operation before an adapter formats it.
#[derive(Debug)]
pub enum RepoOutcome {
    Listed {
        repositories: Vec<String>,
        effects: Effects,
    },
    Ls {
        listing: repo::Listing,
        effects: Effects,
    },
    Got {
        bytes: Vec<u8>,
        out: Option<PathBuf>,
        effects: Effects,
    },
    Status {
        local: PathBuf,
        compared: Vec<repo::Compared>,
        effects: Effects,
    },
    Changed {
        planned: repo::Planned,
        effects: Effects,
    },
    Synced {
        local: PathBuf,
        direction: repo::Direction,
        synced: repo::Synced,
        mode: Mode,
        effects: Effects,
    },
}

impl RepoOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Listed { effects, .. }
            | Self::Ls { effects, .. }
            | Self::Got { effects, .. }
            | Self::Status { effects, .. }
            | Self::Changed { effects, .. }
            | Self::Synced { effects, .. } => *effects,
        }
    }
}

/// A failure before a typed repository outcome could be produced.
#[derive(Debug, thiserror::Error)]
pub enum RepoCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Repo(repo::RepoError),
    #[error("{} exists; pass --force to replace it", .0.display())]
    Exists(PathBuf),
    #[error("{}: {why}", .path.display())]
    Create { path: PathBuf, why: std::io::Error },
    #[error("{0}")]
    Write(workspace::WorkspaceError),
}
impl Coded for RepoCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
            Self::Profile(why) => why.code(),
            Self::Repo(why) => why.code(),
            Self::Exists(_) => ErrorCode::AlreadyExists,
            Self::Create { .. } | Self::Write(_) => ErrorCode::IoError,
        }
    }
}

/// Execute a repository operation. An applied pull locks before it computes the local root or
/// asks the server what it will copy; every other operation leaves the solution tree untouched.
pub fn execute<R, F>(
    solution: &Solution,
    request: &RepoRequest,
    open: F,
    notices: &mut Notices,
) -> Result<RepoOutcome, RepoCommandError>
where
    R: repo::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    execute_with_progress(solution, request, open, notices, &progress::NONE)
}

/// Like [`execute`], and report one step per folder listed, file compared, file copied or file
/// moved. Messages hold repository paths only.
pub fn execute_with_progress<R, F>(
    solution: &Solution,
    request: &RepoRequest,
    open: F,
    notices: &mut Notices,
    progress: &dyn Progress,
) -> Result<RepoOutcome, RepoCommandError>
where
    R: repo::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let pull = matches!(
        &request.action,
        RepoAction::Sync {
            direction: repo::Direction::Pull,
            mode: Mode::Apply,
            ..
        }
    );
    let _lock = if pull {
        Some(lock_workspace(solution, "repo pull", notices).map_err(RepoCommandError::Lock)?)
    } else {
        None
    };
    let profile =
        profile::load(&solution.root, &request.profile).map_err(RepoCommandError::Profile)?;
    let remote = open(profile);
    match &request.action {
        RepoAction::List => Ok(RepoOutcome::Listed {
            repositories: repo::Remote::repositories(&remote)
                .map_err(|why| RepoCommandError::Repo(repo::RepoError::Remote(why)))?,
            effects: Effects::new(Access::None, Access::Read),
        }),
        RepoAction::Ls {
            repository,
            path,
            recursive,
        } => Ok(RepoOutcome::Ls {
            listing: repo::list_with_progress(&remote, repository, path, *recursive, progress)
                .map_err(RepoCommandError::Repo)?,
            effects: Effects::new(Access::None, Access::Read),
        }),
        RepoAction::Get {
            repository,
            path,
            out,
            force,
        } => {
            if let Some(out) = out {
                if out.exists() && !force {
                    return Err(RepoCommandError::Exists(out.clone()));
                }
            }
            let bytes = {
                let _phase = progress::phase(progress, "downloading file", Some(1));
                let bytes = repo::get(&remote, repository, path).map_err(RepoCommandError::Repo)?;
                progress.advance(1);
                bytes
            };
            if let Some(out) = out {
                if let Some(folder) = out.parent().filter(|path| !path.as_os_str().is_empty()) {
                    std::fs::create_dir_all(folder).map_err(|why| RepoCommandError::Create {
                        path: folder.to_path_buf(),
                        why,
                    })?;
                }
                workspace::write_entity(out, &bytes).map_err(RepoCommandError::Write)?;
            }
            Ok(RepoOutcome::Got {
                bytes,
                out: out.clone(),
                effects: Effects::new(
                    if out.is_some() {
                        Access::Write
                    } else {
                        Access::None
                    },
                    Access::Read,
                ),
            })
        }
        RepoAction::Status { repository } => {
            let local = repo::local_root(
                &solution.root,
                solution.repositories.root.as_deref(),
                repository,
            );
            let compared = repo::status_with_progress(&remote, repository, &local, progress)
                .map_err(RepoCommandError::Repo)?;
            Ok(RepoOutcome::Status {
                local,
                compared,
                effects: Effects::new(Access::Read, Access::Read),
            })
        }
        RepoAction::Change {
            repository,
            change,
            mode,
        } => {
            let planned = {
                // A plan changes nothing, so its phase must not say that it changes a file.
                let phase = match mode {
                    Mode::Plan => "planning file change",
                    Mode::Apply => "changing file",
                };
                let _phase = progress::phase(progress, phase, Some(1));
                let planned =
                    repo::change(&remote, repository, change, matches!(mode, Mode::Apply))
                        .map_err(RepoCommandError::Repo)?;
                progress.advance(1);
                planned
            };
            let server = if planned.applied {
                Access::Write
            } else {
                Access::Read
            };
            Ok(RepoOutcome::Changed {
                planned,
                effects: Effects::new(Access::None, server),
            })
        }
        RepoAction::Sync {
            repository,
            direction,
            overwrite,
            mode,
        } => {
            let local = repo::local_root(
                &solution.root,
                solution.repositories.root.as_deref(),
                repository,
            );
            let synced = repo::sync_with_progress(
                &remote,
                repository,
                &local,
                *direction,
                *overwrite,
                matches!(mode, Mode::Apply),
                progress,
            )
            .map_err(RepoCommandError::Repo)?;
            let workspace = if *direction == repo::Direction::Pull && matches!(mode, Mode::Apply) {
                Access::Write
            } else {
                Access::Read
            };
            let server = if *direction == repo::Direction::Push && matches!(mode, Mode::Apply) {
                Access::Write
            } else {
                Access::Read
            };
            Ok(RepoOutcome::Synced {
                local,
                direction: *direction,
                synced,
                mode: *mode,
                effects: Effects::new(workspace, server),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::server::ServerError;
    use serde_json::Value;

    struct Never;

    impl repo::Remote for Never {
        fn service(&self, _: &str, _: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            unreachable!()
        }

        fn download(&self, _: &str, _: &str) -> Result<Vec<u8>, ServerError> {
            unreachable!()
        }

        fn repositories(&self) -> Result<Vec<String>, ServerError> {
            unreachable!()
        }
    }

    struct Bytes;

    impl repo::Remote for Bytes {
        fn service(&self, _: &str, _: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            unreachable!()
        }

        fn download(&self, _: &str, _: &str) -> Result<Vec<u8>, ServerError> {
            Ok(b"new".to_vec())
        }

        fn repositories(&self) -> Result<Vec<String>, ServerError> {
            unreachable!()
        }
    }

    #[test]
    fn a_pull_plan_does_not_lock_and_an_apply_locks_before_loading_its_profile() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-repo-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\nroot = \".\"\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire_for(&solution, "holder").unwrap();
        let request = |mode| RepoRequest {
            action: RepoAction::Sync {
                repository: "R".to_string(),
                direction: repo::Direction::Pull,
                overwrite: false,
                mode,
            },
            profile: "missing".to_string(),
        };
        let plan = execute(
            &solution,
            &request(Mode::Plan),
            |_| Never,
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(matches!(plan, RepoCommandError::Profile(_)));
        let apply = execute(
            &solution,
            &request(Mode::Apply),
            |_| Never,
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(matches!(apply, RepoCommandError::Lock(_)));
        drop(held);
    }

    #[test]
    fn get_output_refuses_an_existing_file_unless_forced() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-repo-get-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let out = root.join("download.bin");
        std::fs::write(&out, b"old").unwrap();
        let request = |force| RepoRequest {
            action: RepoAction::Get {
                repository: "R".to_string(),
                path: "/x".to_string(),
                out: Some(out.clone()),
                force,
            },
            profile: "default".to_string(),
        };
        let error = execute(
            &solution,
            &request(false),
            |_| Never,
            &mut Notices::default(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::AlreadyExists);
        let outcome = execute(
            &solution,
            &request(true),
            |_| Bytes,
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.effects(), Effects::new(Access::Write, Access::Read));
        assert_eq!(std::fs::read(&out).unwrap(), b"new");
    }

    /// Has no files, so every path is absent and every change succeeds.
    struct Empty;

    impl repo::Remote for Empty {
        fn service(&self, _: &str, service: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            if service == "GetFileInfo" {
                return Err(ServerError::Http {
                    method: crate::core::server::Method::Post,
                    status: 404,
                    url: String::new(),
                    body: String::new(),
                });
            }
            Ok(None)
        }

        fn download(&self, _: &str, _: &str) -> Result<Vec<u8>, ServerError> {
            Ok(b"new".to_vec())
        }

        fn repositories(&self) -> Result<Vec<String>, ServerError> {
            unreachable!()
        }
    }

    #[test]
    fn repository_progress_names_no_path_and_a_plan_does_not_report_a_change() {
        use crate::core::progress::{Event, Recorder};
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-repo-progress-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let run = |action| {
            let recorder = Recorder::default();
            // The fake does not keep a created folder, so an applied change fails its own
            // check afterwards. The phase is already recorded by then.
            let _ = execute_with_progress(
                &solution,
                &RepoRequest {
                    action,
                    profile: "default".to_string(),
                },
                |_| Empty,
                &mut Notices::default(),
                &recorder,
            );
            recorder
        };
        let got = run(RepoAction::Get {
            repository: "R".to_string(),
            path: "/secret-path".to_string(),
            out: None,
            force: false,
        });
        assert_eq!(got.phases(), [("downloading file".to_string(), Some(1))]);
        assert!(!got
            .events()
            .iter()
            .any(|event| matches!(event, Event::Message(_))));
        let change = |mode| RepoAction::Change {
            repository: "R".to_string(),
            change: repo::Change::Mkdir {
                path: "/new".to_string(),
            },
            mode,
        };
        assert_eq!(
            run(change(Mode::Plan)).phases(),
            [("planning file change".to_string(), Some(1))]
        );
        assert_eq!(
            run(change(Mode::Apply)).phases(),
            [("changing file".to_string(), Some(1))]
        );
    }
}
