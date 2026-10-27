//! The command policy around server imports.

use super::{Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::progress::{self, Progress};
use crate::core::{imports, profile};

/// An import source supplied by either adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportAction {
    File { file_name: String, bytes: Vec<u8> },
    SourceControl { repository: String, path: String },
}

/// The profile, overwrite choices, and mode for one import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRequest {
    pub action: ImportAction,
    pub mode: Mode,
    pub overwrite_properties: bool,
    pub overwrite_tables: bool,
    pub profile: String,
}

/// A completed import before an adapter formats it.
#[derive(Debug)]
pub enum ImportOutcome {
    File {
        plan: imports::FilePlan,
        effects: Effects,
    },
    SourceControl {
        report: imports::TreeImport,
        effects: Effects,
    },
}

impl ImportOutcome {
    #[cfg(test)]
    pub(crate) const fn effects(&self) -> Effects {
        match self {
            Self::File { effects, .. } | Self::SourceControl { effects, .. } => *effects,
        }
    }
}

/// A failure before a typed import outcome could be produced.
#[derive(Debug, thiserror::Error)]
pub enum ImportCommandError {
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Import(imports::ImportError),
}
impl Coded for ImportCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(why) => why.code(),
            Self::Import(why) => why.code(),
        }
    }
}

/// Execute an import. Plans do the domain's validation and server reads; applies send only after
/// that preparation succeeds.
pub fn execute<R, F>(
    solution: &Solution,
    request: &ImportRequest,
    open: F,
    notices: &mut Notices,
    progress: &dyn Progress,
) -> Result<ImportOutcome, ImportCommandError>
where
    R: imports::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile =
        profile::load(&solution.root, &request.profile).map_err(ImportCommandError::Profile)?;
    let remote = open(profile);
    let apply = matches!(request.mode, Mode::Apply);
    let server = if apply { Access::Write } else { Access::Read };
    match &request.action {
        ImportAction::File { file_name, bytes } => Ok(ImportOutcome::File {
            plan: imports::import_file_with_server_owned_progress(
                &remote,
                file_name,
                bytes,
                imports::FileOptions {
                    overwrite_properties: request.overwrite_properties,
                    overwrite_tables: request.overwrite_tables,
                    apply,
                    server_owned: &solution.server_owned_for(&request.profile),
                },
                progress,
            )
            .map_err(ImportCommandError::Import)?,
            effects: Effects::new(Access::None, server),
        }),
        ImportAction::SourceControl { repository, path } => {
            if solution
                .server_owned_for(&request.profile)
                .iter()
                .any(|rule| !rule.built_in)
            {
                notices
                    .push("source-control import does not preserve server-owned parts".to_string());
            }
            Ok(ImportOutcome::SourceControl {
                report: {
                    let _phase = progress::phase(progress, "importing source control", Some(1));
                    let report = imports::import_source_control(
                        &remote,
                        repository,
                        path,
                        request.overwrite_properties,
                        request.overwrite_tables,
                        apply,
                    )
                    .map_err(ImportCommandError::Import)?;
                    progress.advance(1);
                    report
                },
                effects: Effects::new(Access::None, server),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::server::ServerError;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        present: bool,
        export: Option<Vec<u8>>,
    }

    impl imports::Remote for Fake {
        fn exists(&self, _: &crate::core::entity::key::EntityKey) -> Result<bool, ServerError> {
            Ok(self.present)
        }

        fn import_file(&self, _: &str, _: &[u8], _: bool, _: bool) -> Result<(), ServerError> {
            self.calls.lock().unwrap().push("file".to_string());
            Ok(())
        }

        fn source_control(&self, service: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            self.calls.lock().unwrap().push(service.to_string());
            Ok(Some(
                json!({ "rows": [{ "entityType": "Things", "name": "A", "difference": { "rows": [{ "diffType": "different", "name": "A" }] } }] }),
            ))
        }

        fn export_owned(
            &self,
            _: &crate::core::entity::key::EntityKey,
        ) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(self.export.clone())
        }
    }

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-imports-")
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
    fn source_control_plan_and_apply_use_the_same_policy() {
        let (_dir, _, solution) = setup();
        let request = |mode| ImportRequest {
            action: ImportAction::SourceControl {
                repository: "R".to_string(),
                path: "/".to_string(),
            },
            mode,
            overwrite_properties: false,
            overwrite_tables: false,
            profile: "default".to_string(),
        };
        let fake = Fake {
            calls: Arc::new(Mutex::new(Vec::new())),
            ..Fake::default()
        };
        let plan = execute(
            &solution,
            &request(Mode::Plan),
            {
                let remote = fake.clone();
                move |_| remote
            },
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        assert_eq!(plan.effects(), Effects::new(Access::None, Access::Read));
        assert_eq!(
            *fake.calls.lock().unwrap(),
            ["DiffSourceControlledEntities"]
        );
        fake.calls.lock().unwrap().clear();
        let apply = execute(
            &solution,
            &request(Mode::Apply),
            {
                let remote = fake.clone();
                move |_| remote
            },
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        assert_eq!(apply.effects(), Effects::new(Access::None, Access::Write));
        assert_eq!(
            *fake.calls.lock().unwrap(),
            [
                "DiffSourceControlledEntities",
                "ImportSourceControlledEntities",
                "DiffSourceControlledEntities"
            ]
        );
    }

    #[test]
    fn file_refusal_keeps_the_import_error_code() {
        let (_dir, _, solution) = setup();
        let request = ImportRequest {
            action: ImportAction::File {
                file_name: "bad.xml".to_string(),
                bytes: b"not xml".to_vec(),
            },
            mode: Mode::Plan,
            overwrite_properties: false,
            overwrite_tables: false,
            profile: "default".to_string(),
        };
        let error = execute(
            &solution,
            &request,
            |_| Fake {
                calls: Arc::new(Mutex::new(Vec::new())),
                ..Fake::default()
            },
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidData);
    }

    #[test]
    fn profile_specific_owned_rules_apply_only_to_that_import_profile() {
        let (_dir, root, _) = setup();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\n[profiles.qa]\n[[profiles.qa.server_owned]]\nentities = [\"Things/T\"]\nparts = [\"property-values:connection\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".twaco/profiles/qa.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let bytes = b"<Entities><Things><Thing name=\"T\"><ThingProperties><connection>local</connection></ThingProperties></Thing></Things></Entities>".to_vec();
        let donor = b"<Entities><Things><Thing name=\"T\"><ThingProperties><connection>server</connection></ThingProperties></Thing></Things></Entities>".to_vec();
        let request = |profile: &str| ImportRequest {
            action: ImportAction::File {
                file_name: "x.xml".to_string(),
                bytes: bytes.clone(),
            },
            mode: Mode::Plan,
            overwrite_properties: false,
            overwrite_tables: false,
            profile: profile.to_string(),
        };
        let fake = Fake {
            present: true,
            export: Some(donor),
            ..Fake::default()
        };
        let default = execute(
            &solution,
            &request("default"),
            {
                let remote = fake.clone();
                move |_| remote
            },
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        let ImportOutcome::File { plan, .. } = default else {
            unreachable!()
        };
        assert!(plan.server_owned.is_empty());
        let qa = execute(
            &solution,
            &request("qa"),
            move |_| fake,
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        let ImportOutcome::File { plan, .. } = qa else {
            unreachable!()
        };
        assert_eq!(plan.server_owned[0].parts, ["property-values:connection"]);
    }

    #[test]
    fn source_control_import_notices_that_owned_parts_are_not_preserved() {
        let (_dir, root, _) = setup();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\n[[server_owned]]\nentities = [\"Things/T\"]\nparts = [\"property-values:connection\"]\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let request = ImportRequest {
            action: ImportAction::SourceControl {
                repository: "R".to_string(),
                path: "/".to_string(),
            },
            mode: Mode::Plan,
            overwrite_properties: false,
            overwrite_tables: false,
            profile: "default".to_string(),
        };
        let mut notices = Notices::default();
        execute(
            &solution,
            &request,
            |_| Fake::default(),
            &mut notices,
            &crate::core::progress::NONE,
        )
        .unwrap();
        assert!(notices.lines()[0].contains("does not preserve server-owned parts"));
    }
}
