//! The command policy around deploy's gates, backup and workspace record.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::backup;
use crate::core::check;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::progress::Progress;
use crate::core::{deploy, localization, lock, network, profile, requirements};

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
        /// The localization tables a whole-solution deploy carries: planned, or imported when
        /// applied. `None` when the deploy does not carry them.
        localization: Option<localization::Pushed>,
        effects: Effects,
    },
}

impl DeployOutcome {
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); no adapter reads it yet"
    )]
    pub(crate) const fn effects(&self) -> Effects {
        match self {
            Self::GatesBlocked { effects, .. } | Self::Complete { effects, .. } => *effects,
        }
    }
}

/// A remote used by deploy, its pre-overwrite backup, its localization tables and the
/// requirements it checks before importing.
pub trait Remote:
    deploy::Remote + backup::Remote + localization::Remote + network::Remote + requirements::Remote
{
}

impl<
        T: deploy::Remote
            + backup::Remote
            + localization::Remote
            + network::Remote
            + requirements::Remote
            + ?Sized,
    > Remote for T
{
}

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
        /// What the refused or failed imports would have done to Network connections.
        network_changes: Vec<deploy::NetworkChange>,
    },
    /// The localization tables stopped the deploy: before any import, or after the entities
    /// were imported (`deployed`).
    #[error("{}", localization_failure(why, *deployed))]
    Localization {
        why: Box<localization::LocalizationError>,
        deployed: bool,
        backup: Option<String>,
        gates: Option<check::CheckReport>,
    },
}

fn localization_failure(why: &localization::LocalizationError, deployed: bool) -> String {
    if deployed {
        format!(
            "the entities were deployed, but the localization tables were not: {why}; \
             `twaco localization push --apply` retries them"
        )
    } else {
        format!("localization tables: {why}; nothing was imported")
    }
}

impl DeployCommandError {
    pub fn backup(&self) -> Option<&str> {
        match self {
            Self::Deploy { backup, .. } | Self::Localization { backup, .. } => backup.as_deref(),
            _ => None,
        }
    }

    pub fn gates(&self) -> Option<&check::CheckReport> {
        match self {
            Self::Plan { gates, .. }
            | Self::Backup { gates, .. }
            | Self::Deploy { gates, .. }
            | Self::Localization { gates, .. } => gates.as_ref(),
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
            Self::Localization { why, .. } => why.code(),
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

/// What importing each bundled Network does to the server's connections. Only reported: a
/// server whose connections changed since the last deploy is already refused by the baseline
/// guard, and removals the repository made on purpose are what the deploy is for.
fn network_changes<R: network::Remote>(
    projects: &[deploy::ProjectBundle],
    remote: &R,
) -> Result<Vec<deploy::NetworkChange>, String> {
    let mut changes = Vec::new();
    for entity in projects
        .iter()
        .flat_map(|project| &project.entities)
        .filter(|entity| entity.collection == "Networks")
    {
        let repository = network::parse(&entity.bytes)
            .map_err(|error| format!("Networks/{}: {error}", entity.name))?;
        // A Network the server does not have yet has no connections to lose.
        let server = match remote.connections(&entity.name, false) {
            Ok(rows) => rows,
            Err(error) if error.is_not_found() => Vec::new(),
            Err(error) => return Err(format!("Networks/{}: {error}", entity.name)),
        };
        let diff = super::network::diff(&repository, &server);
        let change = deploy::NetworkChange {
            name: entity.name.clone(),
            adds: diff.repository_only.len(),
            moves: diff.different.len(),
            removes: diff.server_only.len(),
        };
        if change.adds + change.moves + change.removes != 0 {
            changes.push(change);
        }
    }
    Ok(changes)
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
        let on_server = solution.root.join(requirements::FILE).exists();
        if solution.gates.live || on_server {
            match profile::load(&solution.root, &request.profile) {
                Ok(profile) => {
                    let remote = open(profile);
                    if solution.gates.live {
                        report.gates.push(check::live_parse_with_progress(
                            solution,
                            Ok(&DeployChecker(&remote)),
                            progress,
                        ));
                    }
                    if on_server {
                        check::requirements_on_server(solution, &mut report, &remote, progress);
                    }
                }
                // The profile error stops the deploy below; the offline requirements gate stands.
                Err(error) if solution.gates.live => report.gates.push(
                    check::live_parse_with_progress(solution, Err(error.to_string()), progress),
                ),
                Err(_) => {}
            }
        }
        if report.blocks() {
            let server = if solution.gates.live || on_server {
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
    let network_changes = match network_changes(&projects, &remote) {
        Ok(changes) => changes,
        Err(why) => return Err(DeployCommandError::Plan { why, gates }),
    };
    // Planned before anything is imported, so a table the server would refuse stops the deploy
    // while nothing has been sent.
    let tables = if carries_localization(solution, request) {
        match localization::push(solution, &remote, None, false, false, progress) {
            Ok(planned) => Some(planned),
            Err(why) => {
                return Err(DeployCommandError::Localization {
                    why: Box::new(why),
                    deployed: false,
                    backup: None,
                    gates,
                })
            }
        }
    } else {
        None
    };
    let backup = if matches!(request.mode, Mode::Apply) && request.force && request.backup {
        match backup::before_forced_deploy(&remote, solution, &projects, &backup::new_stamp()) {
            Ok(saved) => saved,
            Err(why) => return Err(DeployCommandError::Backup { why, gates }),
        }
    } else {
        None
    };
    let mut report = match deploy::run_with_progress(
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
                network_changes,
            })
        }
    };
    report.network_changes = network_changes;
    let localization = match tables {
        Some(planned) if matches!(request.mode, Mode::Apply) && !planned.tables.is_empty() => {
            match localization::push(solution, &remote, None, false, true, progress) {
                Ok(pushed) => Some(pushed),
                Err(why) => {
                    return Err(DeployCommandError::Localization {
                        why: Box::new(why),
                        deployed: true,
                        backup,
                        gates,
                    })
                }
            }
        }
        tables => tables,
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
        localization,
        effects: Effects::new(workspace, server),
    })
}

/// Whether this deploy carries the localization tables: a deploy of the whole solution does,
/// when it has table files (or files that look like tables and cannot be read, which then stop
/// it). A deploy of some projects or entities, or of the backend only, leaves them to
/// `twaco localization push`: the tables are shared by every project.
fn carries_localization(solution: &Solution, request: &DeployRequest) -> bool {
    request.only.is_empty()
        && request.only_projects.is_empty()
        && !request.backend_only
        && localization::discover(&localization::root(solution)).map_or(true, |found| {
            !found.files.is_empty() || !found.unreadable.is_empty()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;

    struct NetworkFake(Vec<network::ServerConnection>);
    impl network::Remote for NetworkFake {
        fn connections(
            &self,
            _: &str,
            _: bool,
        ) -> Result<Vec<network::ServerConnection>, crate::core::server::ServerError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn deploy_network_preflight_counts_replaced_connections_as_data_loss() {
        let project = deploy::ProjectBundle {
            project: "Acme".into(),
            file_name: "Acme.xml".into(),
            bytes: Vec::new(),
            entities: vec![deploy::Entity {
                server_owned: Vec::new(),
                collection: "Networks".into(),
                name: "Acme.Plant".into(),
                bytes: b"<Entities><Networks><Network name=\"Acme.Plant\" projectName=\"Acme\"><Connections><Connection connectionType=\"Contains\" from=\"\" to=\"Acme.Site\"></Connection></Connections></Network></Networks></Entities>".to_vec(),
            }],
            scripts: Vec::new(),
            deploy: None,
            post_import: Vec::new(),
        };
        let server = NetworkFake(vec![network::ServerConnection {
            connection_type: "Contains".into(),
            from: "".into(),
            to: "Acme.ServerSite".into(),
            template: None,
        }]);
        assert_eq!(
            network_changes(&[project], &server).unwrap(),
            vec![deploy::NetworkChange {
                name: "Acme.Plant".into(),
                adds: 1,
                moves: 0,
                removes: 1,
            }]
        );
    }

    struct NoNetwork;
    impl network::Remote for NoNetwork {
        fn connections(
            &self,
            network: &str,
            _: bool,
        ) -> Result<Vec<network::ServerConnection>, crate::core::server::ServerError> {
            Err(crate::core::server::ServerError::Http {
                method: crate::core::server::Method::Post,
                status: 404,
                url: format!("Networks/{network}"),
                body: String::new(),
            })
        }
    }

    #[test]
    fn a_network_new_to_the_server_only_adds_connections() {
        let project = deploy::ProjectBundle {
            project: "Acme".into(),
            file_name: "Acme.xml".into(),
            bytes: Vec::new(),
            entities: vec![deploy::Entity {
                server_owned: Vec::new(),
                collection: "Networks".into(),
                name: "Acme.Plant".into(),
                bytes: b"<Entities><Networks><Network name=\"Acme.Plant\" projectName=\"Acme\"><Connections><Connection connectionType=\"Contains\" from=\"\" to=\"Acme.Site\"></Connection></Connections></Network></Networks></Entities>".to_vec(),
            }],
            scripts: Vec::new(),
            deploy: None,
            post_import: Vec::new(),
        };
        assert_eq!(
            network_changes(&[project], &NoNetwork).unwrap(),
            vec![deploy::NetworkChange {
                name: "Acme.Plant".into(),
                adds: 1,
                moves: 0,
                removes: 0,
            }]
        );
    }

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

    impl network::Remote for Rejecting {
        fn connections(
            &self,
            _: &str,
            _: bool,
        ) -> Result<Vec<network::ServerConnection>, crate::core::server::ServerError> {
            Ok(Vec::new())
        }
    }

    impl crate::core::push::Remote for Rejecting {
        fn fetch(
            &self,
            _: &crate::core::entity::key::EntityKey,
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
            _: &crate::core::entity::key::EntityKey,
        ) -> Result<Option<Vec<u8>>, crate::core::server::ServerError> {
            Ok(None)
        }

        fn import(&self, _: &str, _: &[u8]) -> Result<(), crate::core::server::ServerError> {
            Ok(())
        }

        fn exists(
            &self,
            _: &crate::core::entity::key::EntityKey,
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
            _: &crate::core::entity::key::ServiceTarget,
            _: &str,
            _: &serde_json::Value,
            _: std::time::Duration,
        ) -> Result<Option<serde_json::Value>, crate::core::server::ServerError> {
            Ok(None)
        }
    }

    impl crate::core::imports::Remote for Rejecting {
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
            unreachable!("no test here imports a file")
        }

        fn source_control(
            &self,
            _: &str,
            _: &serde_json::Value,
        ) -> Result<Option<serde_json::Value>, crate::core::server::ServerError> {
            unreachable!("no test here exports")
        }
    }

    /// A server with an empty Default table: every local token is one to import.
    impl localization::Remote for Rejecting {
        fn tables(&self) -> Result<Vec<String>, crate::core::server::ServerError> {
            Ok(vec![localization::DEFAULT_TABLE.to_string()])
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
            unreachable!("a deploy never prunes")
        }
    }

    /// No test with this server keeps a requirements.toml.
    impl requirements::Remote for Rejecting {
        fn platform_version(&self) -> Result<String, String> {
            unreachable!("no requirements to check")
        }
        fn extensions(&self) -> Result<std::collections::BTreeMap<String, String>, String> {
            unreachable!("no requirements to check")
        }
        fn entity_exists(&self, _: &str, _: &str) -> Result<bool, String> {
            unreachable!("no requirements to check")
        }
    }

    /// A solution of one Thing, a profile, and a Default table file holding `table`.
    fn with_table(table: &[u8]) -> (tempfile::TempDir, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-deploy-localization-")
            .tempdir()
            .unwrap();
        let root = root_guard.path();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(root.join("localization")).unwrap();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://127.0.0.1:1\"
username = \"u\"
password = \"p\"
",
        )
        .unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]
name = \"Acme.App\"
",
        )
        .unwrap();
        std::fs::write(
            root.join("Things/Acme.App.T.xml"),
            "<Entities><Things><Thing name=\"Acme.App.T\" projectName=\"Acme.App\"></Thing></Things></Entities>",
        )
        .unwrap();
        std::fs::write(
            root.join("localization/Acme.App_LocalizationTable_Default.xml"),
            table,
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, solution)
    }

    fn whole(mode: Mode) -> DeployRequest {
        DeployRequest {
            mode,
            force: false,
            backup: false,
            skip_checks: true,
            only_projects: Vec::new(),
            only: Vec::new(),
            backend_only: false,
            profile: "default".to_string(),
            lock_label: "deploy",
        }
    }

    const DEFAULT: &[u8] = include_bytes!(
        "../../../tests/fixtures/localization/flat/Acme.App_LocalizationTable_Default.xml"
    );

    #[test]
    fn a_whole_solution_deploy_plans_its_localization_tables_and_a_partial_one_leaves_them() {
        let (_root, solution) = with_table(DEFAULT);
        let outcome = execute(
            &solution,
            &whole(Mode::Plan),
            |_| Rejecting,
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        let DeployOutcome::Complete { localization, .. } = outcome else {
            panic!("the plan completes");
        };
        let pushed = localization.expect("a whole-solution deploy carries the tables");
        assert!(!pushed.applied);
        assert_eq!(pushed.tables.len(), 1);
        assert_eq!(pushed.tables[0].table, localization::DEFAULT_TABLE);
        assert_eq!(pushed.tables[0].set.len(), 6);
        for partial in [
            DeployRequest {
                only_projects: vec!["Acme.App".to_string()],
                ..whole(Mode::Plan)
            },
            DeployRequest {
                only: vec!["Acme.App.T".to_string()],
                ..whole(Mode::Plan)
            },
            DeployRequest {
                backend_only: true,
                ..whole(Mode::Plan)
            },
        ] {
            assert!(!carries_localization(&solution, &partial), "{partial:?}");
        }
    }

    /// A server that keeps what it imports: the entity, read back as the file holds it, and the
    /// localization tables, unless `forgets` makes it drop every token it is sent.
    #[derive(Clone)]
    struct Keeping {
        entity: Vec<u8>,
        imported: std::sync::Arc<std::sync::Mutex<bool>>,
        tables: std::sync::Arc<std::sync::Mutex<Vec<localization::Token>>>,
        events: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        forgets: bool,
    }

    impl network::Remote for Keeping {
        fn connections(
            &self,
            _: &str,
            _: bool,
        ) -> Result<Vec<network::ServerConnection>, crate::core::server::ServerError> {
            Ok(Vec::new())
        }
    }

    impl Keeping {
        fn new(solution: &Solution, forgets: bool) -> Self {
            Keeping {
                entity: std::fs::read(solution.root.join("Things/Acme.App.T.xml")).unwrap(),
                imported: std::sync::Arc::default(),
                tables: std::sync::Arc::default(),
                events: std::sync::Arc::default(),
                forgets,
            }
        }
    }

    impl crate::core::push::Remote for Keeping {
        fn fetch(
            &self,
            _: &crate::core::entity::key::EntityKey,
        ) -> Result<Option<Vec<u8>>, crate::core::server::ServerError> {
            Ok((*self.imported.lock().unwrap()).then(|| self.entity.clone()))
        }

        fn import(&self, _: &str, _: &[u8]) -> Result<(), crate::core::server::ServerError> {
            *self.imported.lock().unwrap() = true;
            self.events.lock().unwrap().push("entities".to_string());
            Ok(())
        }
    }

    impl backup::Remote for Keeping {
        fn export(
            &self,
            _: &crate::core::entity::key::EntityKey,
        ) -> Result<Option<Vec<u8>>, crate::core::server::ServerError> {
            Ok(None)
        }

        fn import(&self, _: &str, _: &[u8]) -> Result<(), crate::core::server::ServerError> {
            Ok(())
        }

        fn exists(
            &self,
            _: &crate::core::entity::key::EntityKey,
        ) -> Result<bool, crate::core::server::ServerError> {
            Ok(false)
        }
    }

    impl deploy::Remote for Keeping {
        fn check_script(
            &self,
            _: &str,
        ) -> Result<crate::core::server::ScriptCheck, crate::core::server::ServerError> {
            unreachable!("the solution has no scripts")
        }

        fn call_service(
            &self,
            _: &crate::core::entity::key::ServiceTarget,
            _: &str,
            _: &serde_json::Value,
            _: std::time::Duration,
        ) -> Result<Option<serde_json::Value>, crate::core::server::ServerError> {
            Ok(None)
        }
    }

    impl crate::core::imports::Remote for Keeping {
        fn exists(
            &self,
            _: &crate::core::entity::key::EntityKey,
        ) -> Result<bool, crate::core::server::ServerError> {
            Ok(true)
        }

        fn import_file(
            &self,
            _: &str,
            bytes: &[u8],
            _: bool,
            _: bool,
        ) -> Result<(), crate::core::server::ServerError> {
            let file = localization::read(std::path::Path::new("sent.xml"), bytes)
                .unwrap()
                .unwrap();
            self.events
                .lock()
                .unwrap()
                .push(format!("table {}", file.table));
            if !self.forgets {
                *self.tables.lock().unwrap() = file.tokens;
            }
            Ok(())
        }

        fn source_control(
            &self,
            _: &str,
            _: &serde_json::Value,
        ) -> Result<Option<serde_json::Value>, crate::core::server::ServerError> {
            unreachable!()
        }
    }

    impl localization::Remote for Keeping {
        fn tables(&self) -> Result<Vec<String>, crate::core::server::ServerError> {
            Ok(vec![localization::DEFAULT_TABLE.to_string()])
        }

        fn tokens(
            &self,
            _: &str,
        ) -> Result<Vec<localization::Token>, crate::core::server::ServerError> {
            Ok(self.tables.lock().unwrap().clone())
        }

        fn header(
            &self,
            _: &str,
        ) -> Result<localization::Header, crate::core::server::ServerError> {
            Ok(localization::Header::default())
        }

        fn delete_token(&self, _: &str, _: &str) -> Result<(), crate::core::server::ServerError> {
            unreachable!("a deploy never prunes")
        }
    }

    /// A 10.0 server with no extensions installed.
    impl requirements::Remote for Keeping {
        fn platform_version(&self) -> Result<String, String> {
            Ok("10.0.0".into())
        }
        fn extensions(&self) -> Result<std::collections::BTreeMap<String, String>, String> {
            Ok(std::collections::BTreeMap::new())
        }
        fn entity_exists(&self, _: &str, _: &str) -> Result<bool, String> {
            Ok(true)
        }
    }

    #[test]
    fn a_deploy_stops_before_any_import_when_the_server_lacks_a_requirement() {
        let (_root, solution) = with_table(DEFAULT);
        std::fs::write(
            solution.root.join("requirements.toml"),
            "[[extension]]
name = \"Acme.Ext\"
minimum = \"1.0.0\"
",
        )
        .unwrap();
        let server = Keeping::new(&solution, false);
        let checked = DeployRequest {
            skip_checks: false,
            ..whole(Mode::Apply)
        };
        let outcome = execute(
            &solution,
            &checked,
            |_| server.clone(),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        let DeployOutcome::GatesBlocked { report, .. } = outcome else {
            panic!("the unmet requirement blocks the deploy");
        };
        let gate = report
            .gates
            .iter()
            .find(|gate| gate.name == "requirements")
            .unwrap();
        assert!(
            gate.findings
                .iter()
                .any(|f| f.rule == "unmet" && f.message.contains("extension/Acme.Ext")),
            "{gate:?}"
        );
        assert_eq!(
            report
                .gates
                .iter()
                .filter(|g| g.name == "requirements")
                .count(),
            1
        );
        assert!(
            server.events.lock().unwrap().is_empty(),
            "nothing was imported"
        );

        // Skipping the checks skips the server's requirements too.
        let outcome = execute(
            &solution,
            &whole(Mode::Plan),
            |_| server.clone(),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        assert!(matches!(outcome, DeployOutcome::Complete { .. }));
    }

    #[test]
    fn an_applied_deploy_imports_the_entities_then_the_tables_that_differ() {
        let (_root, solution) = with_table(DEFAULT);
        let server = Keeping::new(&solution, false);
        let outcome = execute(
            &solution,
            &whole(Mode::Apply),
            |_| server.clone(),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        let DeployOutcome::Complete { localization, .. } = outcome else {
            panic!("the deploy completes");
        };
        let pushed = localization.unwrap();
        assert!(pushed.applied);
        assert_eq!(pushed.tables[0].table, localization::DEFAULT_TABLE);
        assert_eq!(
            *server.events.lock().unwrap(),
            ["entities", "table Default"]
        );
        assert_eq!(server.tables.lock().unwrap().len(), 6);
    }

    #[test]
    fn tables_that_fail_after_the_import_say_the_entities_were_deployed() {
        let (_root, solution) = with_table(DEFAULT);
        let server = Keeping::new(&solution, true);
        let error = execute(
            &solution,
            &whole(Mode::Apply),
            |_| server.clone(),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                DeployCommandError::Localization { deployed: true, .. }
            ),
            "{error:?}"
        );
        assert!(
            error.to_string().contains("the entities were deployed"),
            "{error}"
        );
        assert_eq!(
            *server.events.lock().unwrap(),
            ["entities", "table Default"]
        );
    }

    #[test]
    fn a_table_the_server_would_refuse_stops_the_deploy_before_anything_is_imported() {
        let (_root, solution) = with_table(
            b"<Entities><LocalizationTables><LocalizationTable/></LocalizationTables></Entities>",
        );
        // Rejecting answers an import as success; reaching one would not fail this test, so the
        // apply is refused by its error, which says nothing was imported.
        let error = execute(
            &solution,
            &whole(Mode::Apply),
            |_| Rejecting,
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                DeployCommandError::Localization {
                    deployed: false,
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(
            error.to_string().contains("nothing was imported"),
            "{error}"
        );
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
