//! The command policy around starting a repository from server projects.

use super::{Access, Effects};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::{export, init_server, localization, profile, push, server};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct InitFromServerRequest {
    pub root: PathBuf,
    pub projects: Vec<String>,
    pub profile: String,
    pub apply: bool,
}

pub struct InitFromServerOutcome {
    pub result: init_server::Outcome,
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); no adapter reads it yet"
    )]
    effects: Effects,
}

impl InitFromServerOutcome {
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); no adapter reads it yet"
    )]
    pub(crate) const fn effects(&self) -> Effects {
        self.effects
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InitFromServerCommandError {
    #[error("{0}")]
    Init(init_server::Error),
    #[error("{0}")]
    Profile(profile::ProfileError),
}

impl Coded for InitFromServerCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Init(error) => error.code(),
            Self::Profile(error) => error.code(),
        }
    }
}

/// Check the target, load its profile once, then plan or write the repository.
///
/// `init_server::execute` already holds the workspace lock for every applied run, including
/// adding to an existing solution, so this executor deliberately does not take a second lock.
pub fn init_from_server(
    request: InitFromServerRequest,
) -> Result<InitFromServerOutcome, InitFromServerCommandError> {
    init_from_server_with(request, server::Client::new)
}

fn init_from_server_with<R, F>(
    request: InitFromServerRequest,
    open: F,
) -> Result<InitFromServerOutcome, InitFromServerCommandError>
where
    R: export::Remote + push::Remote + localization::Remote + Sync,
    F: FnOnce(profile::Profile) -> R,
{
    init_server::preflight(&request.root, &request.projects)
        .map_err(InitFromServerCommandError::Init)?;
    let profile = profile::load(&request.root, &request.profile)
        .map_err(InitFromServerCommandError::Profile)?;
    let result = init_server::execute(
        &open(profile),
        init_server::Request {
            root: request.root,
            projects: request.projects,
            apply: request.apply,
            profile: request.profile,
        },
    )
    .map_err(InitFromServerCommandError::Init)?;
    let effects = Effects::new(
        if result.applied {
            Access::Write
        } else {
            Access::Read
        },
        Access::Read,
    );
    Ok(InitFromServerOutcome { result, effects })
}

/// The stable object returned by both `init --from-server --json` and `init_from_server`.
pub fn outcome_json(outcome: &InitFromServerOutcome) -> Value {
    let outcome = &outcome.result;
    json!({
        "projects": outcome.projects,
        "files": outcome.files,
        "entities": outcome.entities,
        "toml": outcome.toml,
        "notes": outcome.notes,
        "applied": outcome.applied,
        "added": outcome.added,
        "recorded": outcome.recorded,
        "not_in_sync": outcome.not_in_sync,
        "localization_files": outcome.localization_files,
        "localization_tokens": outcome.localization_tokens,
        "localization_failure": outcome.localization_failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::localization;
    use serde_json::Value;
    use std::collections::BTreeMap;

    struct Fake(BTreeMap<String, Vec<u8>>);

    impl export::Remote for Fake {
        fn export_xml(
            &self,
            _: Option<&str>,
            _: Option<&str>,
            project: Option<&str>,
        ) -> Result<Vec<u8>, crate::core::server::ServerError> {
            Ok(self.0.get(project.unwrap()).cloned().unwrap())
        }

        fn source_control(
            &self,
            _: &str,
            _: &Value,
        ) -> Result<Option<Value>, crate::core::server::ServerError> {
            unreachable!("init --from-server only reads Exporter")
        }
    }

    impl push::Remote for Fake {
        fn fetch(
            &self,
            _: &crate::core::entity::key::EntityKey,
        ) -> Result<Option<Vec<u8>>, crate::core::server::ServerError> {
            Ok(None)
        }

        fn import(&self, _: &str, _: &[u8]) -> Result<(), crate::core::server::ServerError> {
            unreachable!("init --from-server never imports")
        }
    }

    impl crate::core::imports::Remote for Fake {
        fn exists(
            &self,
            _: &crate::core::entity::key::EntityKey,
        ) -> Result<bool, crate::core::server::ServerError> {
            Ok(false)
        }

        fn import_file(
            &self,
            _: &str,
            _: &[u8],
            _: bool,
            _: bool,
        ) -> Result<(), crate::core::server::ServerError> {
            unreachable!("init --from-server never imports")
        }

        fn source_control(
            &self,
            _: &str,
            _: &Value,
        ) -> Result<Option<Value>, crate::core::server::ServerError> {
            unreachable!("init --from-server does not use source control")
        }
    }

    impl localization::Remote for Fake {
        fn tables(&self) -> Result<Vec<String>, crate::core::server::ServerError> {
            Ok(Vec::new())
        }

        fn tokens(
            &self,
            _: &str,
        ) -> Result<Vec<localization::Token>, crate::core::server::ServerError> {
            Ok(Vec::new())
        }

        fn header(
            &self,
            _: &str,
        ) -> Result<localization::Header, crate::core::server::ServerError> {
            Ok(localization::Header::default())
        }

        fn delete_token(&self, _: &str, _: &str) -> Result<(), crate::core::server::ServerError> {
            unreachable!("init --from-server never deletes tokens")
        }
    }

    fn export(project: &str, name: &str) -> Vec<u8> {
        format!("<Entities><Things><Thing name=\"{name}\" projectName=\"{project}\"><ThingShape><ServiceDefinitions><ServiceDefinition name=\"S\"/></ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[me();]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>").into_bytes()
    }

    fn profile(root: &std::path::Path) {
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://127.0.0.1:1\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
    }

    #[test]
    fn executor_plans_then_applies_with_a_fake_remote() {
        let temp = tempfile::tempdir().unwrap();
        profile(temp.path());
        let request = |apply| InitFromServerRequest {
            root: temp.path().to_path_buf(),
            projects: vec!["P".into()],
            profile: "default".into(),
            apply,
        };
        let remote = || Fake(BTreeMap::from([("P".to_string(), export("P", "P.T"))]));

        let plan = init_from_server_with(request(false), |_| remote()).unwrap();
        assert!(!plan.result.applied);
        assert!(!temp.path().join("twaco.toml").exists());

        let applied = init_from_server_with(request(true), |_| remote()).unwrap();
        assert!(applied.result.applied);
        assert!(temp.path().join("Things/P.T.xml").exists());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("src/P.T/services/S/script.js")).unwrap(),
            "me();"
        );
        assert!(
            std::fs::read_to_string(temp.path().join("src/P.T/services/S/definition.xml"))
                .unwrap()
                .contains("name=\"S\"")
        );
    }

    #[test]
    fn outcome_json_is_the_single_adapter_projection() {
        let outcome = InitFromServerOutcome {
            result: init_server::Outcome {
                projects: vec!["P".into()],
                files: 1,
                entities: BTreeMap::from([("Things".into(), 1)]),
                toml: "[[project]]\nname = \"P\"\n".into(),
                notes: vec!["P: 1 entities".into()],
                applied: false,
                added: false,
                recorded: 0,
                not_in_sync: Vec::new(),
                localization_files: 0,
                localization_tokens: 0,
                localization_failure: None,
            },
            effects: Effects::new(Access::None, Access::Read),
        };
        assert_eq!(
            outcome_json(&outcome),
            json!({
                "projects": ["P"], "files": 1, "entities": {"Things": 1},
                "toml": "[[project]]\nname = \"P\"\n", "notes": ["P: 1 entities"],
                "applied": false, "added": false, "recorded": 0, "not_in_sync": [],
                "localization_files": 0, "localization_tokens": 0,
                "localization_failure": null,
            })
        );
    }
}
