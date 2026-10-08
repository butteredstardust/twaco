//! The command policy around exports from a server.

use super::{Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{export, profile, workspace};
use std::path::PathBuf;

/// An export destination and its requested selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportAction {
    Xml {
        what: export::What,
        out: PathBuf,
        force: bool,
    },
    SourceControl {
        repository: String,
        path: String,
        filters: export::Filters,
        zip: Option<String>,
        mode: Mode,
    },
}

/// The profile and export operation to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRequest {
    pub action: ExportAction,
    pub profile: String,
}

/// A completed export before an adapter formats it.
#[derive(Debug)]
pub enum ExportOutcome {
    Xml {
        out: PathBuf,
        exported: export::Exported,
        effects: Effects,
    },
    SourceControl {
        plan: String,
        download: Option<String>,
        mode: Mode,
        effects: Effects,
    },
}

impl ExportOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Xml { effects, .. } | Self::SourceControl { effects, .. } => *effects,
        }
    }
}

/// A failure before a typed export outcome could be produced.
#[derive(Debug, thiserror::Error)]
pub enum ExportCommandError {
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{} exists; pass --force to replace it", .0.display())]
    Exists(PathBuf),
    #[error("{0}")]
    Export(export::ExportError),
    #[error("{}: {why}", .path.display())]
    Create { path: PathBuf, why: std::io::Error },
    #[error("{0}")]
    Write(workspace::WorkspaceError),
}

impl Coded for ExportCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(why) => why.code(),
            Self::Exists(_) => ErrorCode::AlreadyExists,
            Self::Export(why) => why.code(),
            Self::Create { .. } | Self::Write(_) => ErrorCode::IoError,
        }
    }
}

/// Execute an export. A source-control export alone is a server write, so its mode controls
/// whether it is sent. XML exports write only the explicitly named output file.
pub fn execute<R, F>(
    solution: &Solution,
    request: &ExportRequest,
    open: F,
    _: &mut Notices,
) -> Result<ExportOutcome, ExportCommandError>
where
    R: export::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile =
        profile::load(&solution.root, &request.profile).map_err(ExportCommandError::Profile)?;
    let remote = open(profile);
    match &request.action {
        ExportAction::Xml { what, out, force } => {
            if out.exists() && !force {
                return Err(ExportCommandError::Exists(out.clone()));
            }
            let exported = export::export(&remote, what).map_err(ExportCommandError::Export)?;
            if let Some(folder) = out.parent().filter(|path| !path.as_os_str().is_empty()) {
                std::fs::create_dir_all(folder).map_err(|why| ExportCommandError::Create {
                    path: folder.to_path_buf(),
                    why,
                })?;
            }
            workspace::write_entity(out, &exported.xml).map_err(ExportCommandError::Write)?;
            Ok(ExportOutcome::Xml {
                out: out.clone(),
                exported,
                effects: Effects::new(Access::Write, Access::Read),
            })
        }
        ExportAction::SourceControl {
            repository,
            path,
            filters,
            zip,
            mode,
        } => {
            let (plan, download) = export::source_control(
                &remote,
                repository,
                path,
                filters,
                zip.as_deref(),
                matches!(mode, Mode::Apply),
            )
            .map_err(ExportCommandError::Export)?;
            let server = if matches!(mode, Mode::Apply) {
                Access::Write
            } else {
                Access::Read
            };
            Ok(ExportOutcome::SourceControl {
                plan,
                download,
                mode: *mode,
                effects: Effects::new(Access::None, server),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;
    use crate::core::server::ServerError;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl export::Remote for Fake {
        fn export_xml(
            &self,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<Vec<u8>, ServerError> {
            self.calls.lock().unwrap().push("xml".to_string());
            Ok(b"<Entities><Things><Thing name=\"A\"/></Things></Entities>".to_vec())
        }

        fn source_control(&self, service: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            self.calls.lock().unwrap().push(service.to_string());
            Ok(None)
        }
    }

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-export-")
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
        (root_guard, root, solution)
    }

    #[test]
    fn source_control_plans_without_writing_and_applies_through_the_same_executor() {
        let (_dir, _, solution) = setup();
        let request = |mode| ExportRequest {
            action: ExportAction::SourceControl {
                repository: "R".to_string(),
                path: "/".to_string(),
                filters: export::Filters {
                    project: Some("P".to_string()),
                    ..Default::default()
                },
                zip: None,
                mode,
            },
            profile: "default".to_string(),
        };
        let fake = Fake {
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let held = lock::acquire_for(&solution, "holder").unwrap();
        let plan = execute(
            &solution,
            &request(Mode::Plan),
            {
                let remote = fake.clone();
                move |_| remote
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(plan.effects(), Effects::new(Access::None, Access::Read));
        assert!(fake.calls.lock().unwrap().is_empty());
        drop(held);

        let fake = Fake {
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let apply = execute(
            &solution,
            &request(Mode::Apply),
            {
                let remote = fake.clone();
                move |_| remote
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(apply.effects(), Effects::new(Access::None, Access::Write));
        assert_eq!(
            *fake.calls.lock().unwrap(),
            ["ExportSourceControlledEntities"]
        );
    }

    #[test]
    fn xml_force_and_refusal_keep_their_effects_and_code() {
        let (_dir, root, solution) = setup();
        let out = root.join("out.xml");
        std::fs::write(&out, b"old").unwrap();
        let request = |force| ExportRequest {
            action: ExportAction::Xml {
                what: export::What::Project {
                    project: "P".to_string(),
                },
                out: out.clone(),
                force,
            },
            profile: "default".to_string(),
        };
        let refusal = execute(
            &solution,
            &request(false),
            |_| Fake {
                calls: Arc::new(Mutex::new(Vec::new())),
            },
            &mut Notices::default(),
        )
        .unwrap_err();
        assert_eq!(refusal.code(), ErrorCode::AlreadyExists);
        let outcome = execute(
            &solution,
            &request(true),
            |_| Fake {
                calls: Arc::new(Mutex::new(Vec::new())),
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.effects(), Effects::new(Access::Write, Access::Read));
    }
}
