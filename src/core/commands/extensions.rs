//! The command policy around extension package changes.

use super::{Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{extensions, profile};

/// An extension package operation requested by either adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionAction {
    List,
    Show {
        name: String,
    },
    Import {
        file_name: String,
        zip: Vec<u8>,
        mode: Mode,
    },
    Remove {
        name: String,
        mode: Mode,
    },
}

/// The profile and extension operation to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionRequest {
    pub action: ExtensionAction,
    pub profile: String,
}

/// A completed extension operation before an adapter formats it.
#[derive(Debug)]
pub enum ExtensionOutcome {
    Listed {
        packages: Vec<extensions::Package>,
        effects: Effects,
    },
    Shown {
        shown: extensions::Shown,
        effects: Effects,
    },
    Imported {
        imported: extensions::Imported,
        mode: Mode,
        effects: Effects,
    },
    Removed {
        plan: String,
        mode: Mode,
        effects: Effects,
    },
}

impl ExtensionOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Listed { effects, .. }
            | Self::Shown { effects, .. }
            | Self::Imported { effects, .. }
            | Self::Removed { effects, .. } => *effects,
        }
    }
}

/// A failure before a typed extension outcome could be produced.
#[derive(Debug, thiserror::Error)]
pub enum ExtensionCommandError {
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Extension(extensions::ExtensionError),
}
impl Coded for ExtensionCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(why) => why.code(),
            Self::Extension(why) => why.code(),
        }
    }
}

/// Execute an extension operation. Extension changes are server-only and therefore never lock
/// the workspace; plans validate without installing or removing a package.
pub fn execute<R, F>(
    solution: &Solution,
    request: &ExtensionRequest,
    open: F,
    _: &mut Notices,
) -> Result<ExtensionOutcome, ExtensionCommandError>
where
    R: extensions::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile =
        profile::load(&solution.root, &request.profile).map_err(ExtensionCommandError::Profile)?;
    let remote = open(profile);
    match &request.action {
        ExtensionAction::List => Ok(ExtensionOutcome::Listed {
            packages: extensions::list(&remote).map_err(ExtensionCommandError::Extension)?,
            effects: Effects::new(Access::None, Access::Read),
        }),
        ExtensionAction::Show { name } => Ok(ExtensionOutcome::Shown {
            shown: extensions::show(&remote, name).map_err(ExtensionCommandError::Extension)?,
            effects: Effects::new(Access::None, Access::Read),
        }),
        ExtensionAction::Import {
            file_name,
            zip,
            mode,
        } => Ok(ExtensionOutcome::Imported {
            imported: extensions::import(&remote, file_name, zip, matches!(mode, Mode::Apply))
                .map_err(ExtensionCommandError::Extension)?,
            mode: *mode,
            effects: Effects::new(
                Access::None,
                if matches!(mode, Mode::Apply) {
                    Access::Write
                } else {
                    Access::Read
                },
            ),
        }),
        ExtensionAction::Remove { name, mode } => Ok(ExtensionOutcome::Removed {
            plan: extensions::remove(&remote, name, matches!(mode, Mode::Apply))
                .map_err(ExtensionCommandError::Extension)?,
            mode: *mode,
            effects: Effects::new(
                Access::None,
                if matches!(mode, Mode::Apply) {
                    Access::Write
                } else {
                    Access::Read
                },
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::server::ServerError;
    use serde_json::{json, Value};
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        installed: Arc<Mutex<bool>>,
    }

    impl extensions::Remote for Fake {
        fn service(&self, service: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            self.calls.lock().unwrap().push(service.to_string());
            let rows = match service {
                "GetExtensionPackageList" if *self.installed.lock().unwrap() => {
                    vec![json!({ "name": "P", "packageVersion": "1.0.0" })]
                }
                "GetExtensionPackageList" => Vec::new(),
                "GetExtensionPackageDetails" => vec![json!({ "name": "E" })],
                "GetExtensionsInUse" => Vec::new(),
                "DeleteExtensionPackage" => {
                    *self.installed.lock().unwrap() = false;
                    return Ok(None);
                }
                other => panic!("unexpected {other}"),
            };
            Ok(Some(json!({ "rows": rows })))
        }

        fn upload(&self, _: &str, _: &[u8], validate: bool) -> Result<Value, ServerError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("upload {validate}"));
            if !validate {
                *self.installed.lock().unwrap() = true;
            }
            Ok(json!({ "rows": [{ "validate": { "rows": [{ "extensionReportStatus": 0 }] } }] }))
        }
    }

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-extensions-")
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

    fn zip() -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(&mut buffer);
        archive
            .start_file("metadata.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"<Entities><ExtensionPackages><ExtensionPackage name=\"P\" packageVersion=\"1.0.0\"/></ExtensionPackages></Entities>").unwrap();
        archive.finish().unwrap();
        buffer.into_inner()
    }

    #[test]
    fn extension_import_plan_and_apply_keep_their_effects() {
        let (_dir, _, solution) = setup();
        let request = |mode| ExtensionRequest {
            action: ExtensionAction::Import {
                file_name: "P.zip".to_string(),
                zip: zip(),
                mode,
            },
            profile: "default".to_string(),
        };
        let fake = Fake {
            calls: Arc::new(Mutex::new(Vec::new())),
            installed: Arc::new(Mutex::new(false)),
        };
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
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .contains(&"upload false".to_string()));
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
        assert!(*fake.installed.lock().unwrap());
    }

    #[test]
    fn profile_refusal_keeps_its_code() {
        let (_dir, _, solution) = setup();
        let error = execute(
            &solution,
            &ExtensionRequest {
                action: ExtensionAction::List,
                profile: "missing".to_string(),
            },
            |_| Fake {
                calls: Arc::new(Mutex::new(Vec::new())),
                installed: Arc::new(Mutex::new(false)),
            },
            &mut Notices::default(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidData);
    }
}
