//! The command policy around deploy's gates, backup and workspace record.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::backup;
use crate::core::check;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::progress::Progress;
use crate::core::{deploy, lock, profile};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployRequest {
    pub mode: Mode,
    pub force: bool,
    pub backup: bool,
    pub skip_checks: bool,
    pub only_projects: Vec<String>,
    pub only: Vec<String>,
    pub backend_only: bool,
    pub profile: String,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub enum DeployOutcome {
    GatesBlocked {
        report: check::CheckReport,
        effects: Effects,
    },
    Complete {
        report: Box<deploy::Report>,
        gates: Option<check::CheckReport>,
        notes: Vec<String>,
        backup: Option<String>,
        effects: Effects,
    },
}

impl DeployOutcome {
    pub const fn effects(&self) -> Effects {
        match self {
            Self::GatesBlocked { effects, .. } | Self::Complete { effects, .. } => *effects,
        }
    }
}

/// A remote used by deploy and its pre-overwrite backup.
pub trait Remote: deploy::Remote + backup::Remote {}

impl<T: deploy::Remote + backup::Remote + ?Sized> Remote for T {}

#[derive(Debug, thiserror::Error)]
pub enum DeployCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{why}")]
    Profile {
        why: profile::ProfileError,
        gates: Option<check::CheckReport>,
    },
    #[error("{why}")]
    Plan {
        why: String,
        gates: Option<check::CheckReport>,
    },
    #[error("{why}")]
    Backup {
        why: backup::BackupError,
        gates: Option<check::CheckReport>,
    },
    #[error("{why}")]
    Deploy {
        why: Box<deploy::DeployError>,
        backup: Option<String>,
        gates: Option<check::CheckReport>,
    },
}

impl DeployCommandError {
    pub fn backup(&self) -> Option<&str> {
        match self {
            Self::Deploy { backup, .. } => backup.as_deref(),
            _ => None,
        }
    }

    pub fn gates(&self) -> Option<&check::CheckReport> {
        match self {
            Self::Plan { gates, .. } | Self::Backup { gates, .. } | Self::Deploy { gates, .. } => {
                gates.as_ref()
            }
            Self::Lock(_) => None,
            Self::Profile { gates, .. } => gates.as_ref(),
        }
    }
}

impl Coded for DeployCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Profile { why, .. } => why.code(),
            Self::Plan { .. } => ErrorCode::Unclassified,
            Self::Backup { why, .. } => why.code(),
            Self::Deploy { why, .. } => why.code(),
        }
    }
}

struct DeployChecker<'a, R>(&'a R);

impl<R: deploy::Remote> check::ScriptChecker for DeployChecker<'_, R> {
    fn check_script(
        &self,
        script: &str,
    ) -> Result<crate::core::server::ScriptCheck, crate::core::server::ServerError> {
        self.0.check_script(script)
    }
}

/// Run a deploy. Applying takes the workspace lock before gates or bundle planning, because any
/// successful import records the baseline for exactly those bytes.
pub fn execute<R, F>(
    solution: &Solution,
    request: &DeployRequest,
    open: F,
    notices: &mut Notices,
    progress: &dyn Progress,
) -> Result<DeployOutcome, DeployCommandError>
where
    R: Remote,
    F: Fn(profile::Profile) -> R,
{
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(DeployCommandError::Lock)?,
        ),
    };
    let gates = if !request.skip_checks {
        let mut report = check::run(solution);
        if solution.gates.live {
            match profile::load(&solution.root, &request.profile) {
                Ok(profile) => {
                    let remote = open(profile);
                    report.gates.push(check::live_parse_with_progress(
                        solution,
                        Ok(&DeployChecker(&remote)),
                        progress,
                    ));
                }
                Err(error) => report.gates.push(check::live_parse_with_progress(
                    solution,
                    Err(error.to_string()),
                    progress,
                )),
            }
        }
        if report.blocks() {
            let server = if solution.gates.live {
                Access::Read
            } else {
                Access::None
            };
            return Ok(DeployOutcome::GatesBlocked {
                report,
                effects: Effects::new(Access::Read, server),
            });
        }
        Some(report)
    } else {
        None
    };
    let (projects, notes) = match deploy::plan_bundles(
        solution,
        deploy::PlanOptions {
            only_projects: &request.only_projects,
            only: &request.only,
            backend_only: request.backend_only,
        },
    ) {
        Ok(planned) => planned,
        Err(why) => return Err(DeployCommandError::Plan { why, gates }),
    };
    let profile = match profile::load(&solution.root, &request.profile) {
        Ok(profile) => profile,
        Err(why) => return Err(DeployCommandError::Profile { why, gates }),
    };
    let remote = open(profile.clone());
    let backup = if matches!(request.mode, Mode::Apply) && request.force && request.backup {
        match backup::before_forced_deploy(&remote, solution, &projects, &backup::new_stamp()) {
            Ok(saved) => saved,
            Err(why) => return Err(DeployCommandError::Backup { why, gates }),
        }
    } else {
        None
    };
    let report = match deploy::run_with_progress(
        &remote,
        &deploy::DiskBaseline::new(&solution.root),
        &profile,
        &projects,
        deploy::RunOptions {
            apply: matches!(request.mode, Mode::Apply),
            force: request.force,
            only: !request.only.is_empty(),
        },
        progress,
    ) {
        Ok(report) => report,
        Err(why) => {
            return Err(DeployCommandError::Deploy {
                why: Box::new(why),
                backup,
                gates,
            })
        }
    };
    let workspace = if matches!(request.mode, Mode::Apply) {
        Access::Write
    } else {
        Access::Read
    };
    let server = if matches!(request.mode, Mode::Apply) {
        Access::Write
    } else {
        Access::Read
    };
    Ok(DeployOutcome::Complete {
        report: Box::new(report),
        gates,
        notes,
        backup,
        effects: Effects::new(workspace, server),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;

    #[test]
    fn a_deploy_plan_needs_no_lock_but_an_apply_locks_before_gates() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-deploy-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = DeployRequest {
            mode: Mode::Plan,
            force: false,
            backup: true,
            skip_checks: true,
            only_projects: Vec::new(),
            only: Vec::new(),
            backend_only: false,
            profile: "default".to_string(),
            lock_label: "deploy",
        };
        assert!(matches!(
            execute::<crate::core::server::Client, _>(
                &solution,
                &request,
                crate::core::server::Client::new,
                &mut Notices::default(),
                &crate::core::progress::NONE
            ),
            Err(DeployCommandError::Plan { .. })
        ));
        let apply = DeployRequest {
            mode: Mode::Apply,
            ..request
        };
        assert!(matches!(
            execute::<crate::core::server::Client, _>(
                &solution,
                &apply,
                crate::core::server::Client::new,
                &mut Notices::default(),
                &crate::core::progress::NONE
            ),
            Err(DeployCommandError::Lock(_))
        ));
        drop(held);
    }

    /// A remote that rejects every script, so the live gate blocks the deploy.
    struct Rejecting;

    impl crate::core::push::Remote for Rejecting {
        fn fetch(
            &self,
            _: &crate::core::entity_key::EntityKey,
        ) -> Result<Option<Vec<u8>>, crate::core::server::ServerError> {
            Ok(None)
        }

        fn import(&self, _: &str, _: &[u8]) -> Result<(), crate::core::server::ServerError> {
            Ok(())
        }
    }

    impl backup::Remote for Rejecting {
        fn export(
            &self,
            _: &crate::core::entity_key::EntityKey,
        ) -> Result<Option<Vec<u8>>, crate::core::server::ServerError> {
            Ok(None)
        }

        fn import(&self, _: &str, _: &[u8]) -> Result<(), crate::core::server::ServerError> {
            Ok(())
        }

        fn exists(
            &self,
            _: &crate::core::entity_key::EntityKey,
        ) -> Result<bool, crate::core::server::ServerError> {
            Ok(false)
        }
    }

    impl deploy::Remote for Rejecting {
        fn check_script(
            &self,
            _: &str,
        ) -> Result<crate::core::server::ScriptCheck, crate::core::server::ServerError> {
            Ok(crate::core::server::ScriptCheck {
                status: false,
                line_number: 1,
                column_number: 1,
                message: "syntax error".to_string(),
            })
        }

        fn call_service(
            &self,
            _: &crate::core::entity_key::ServiceTarget,
            _: &str,
            _: &serde_json::Value,
            _: std::time::Duration,
        ) -> Result<Option<serde_json::Value>, crate::core::server::ServerError> {
            Ok(None)
        }
    }

    #[test]
    fn the_live_gate_of_a_deploy_reports_one_step_per_script() {
        use crate::core::progress::{Event, Recorder};
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-deploy-gate-")
            .tempdir()
            .unwrap();
        let root = root_guard.path();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://127.0.0.1:1\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[gates]\nlive = true\n\n[[project]]\nname = \"P\"\n",
        )
        .unwrap();
        let service = |name: &str| {
            format!(
                "<ServiceImplementation name=\"{name}\" handlerName=\"Script\"><ConfigurationTables>\
                 <ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[result = 1;]]></code></Row>\
                 </Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation>"
            )
        };
        let xml = format!(
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><ThingShape><ServiceDefinitions>\
             <ServiceDefinition name=\"A\"/><ServiceDefinition name=\"B\"/></ServiceDefinitions>\
             <ServiceImplementations>{}{}</ServiceImplementations></ThingShape></Thing></Things></Entities>",
            service("A"),
            service("B")
        );
        std::fs::write(root.join("Things/P.T.xml"), xml).unwrap();
        for name in ["A", "B"] {
            let dir = root.join(format!("src/P.T/services/{name}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("script.js"), "result = 1;").unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let request = DeployRequest {
            mode: Mode::Plan,
            force: false,
            backup: false,
            skip_checks: false,
            only_projects: Vec::new(),
            only: Vec::new(),
            backend_only: false,
            profile: "default".to_string(),
            lock_label: "deploy",
        };
        let recorder = Recorder::default();
        let outcome = execute(
            &solution,
            &request,
            |_| Rejecting,
            &mut Notices::default(),
            &recorder,
        )
        .unwrap();
        assert!(matches!(outcome, DeployOutcome::GatesBlocked { .. }));
        assert_eq!(
            recorder.phases(),
            [("checking scripts".to_string(), Some(2))]
        );
        assert_eq!(recorder.advanced(), 2);
        assert_eq!(recorder.events().last(), Some(&Event::Finish));
    }
}
