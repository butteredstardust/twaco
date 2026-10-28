//! The command policy around comparing entities with the server.

use super::{lock_workspace, Access, Effects, Notices};
use crate::core::baseline::{self, Baseline};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::progress::Progress;
use crate::core::{lock, profile, push, status, workspace};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusTarget {
    Names(Vec<String>),
    All,
}

/// The arguments that affect an entity status read and optional baseline record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusRequest {
    pub target: StatusTarget,
    pub project: Option<String>,
    pub record: bool,
    pub profile: String,
    pub lock_label: &'static str,
    /// The CLI refuses unreadable workspace files for every status request. MCP preserves its
    /// read-only partial result, but refuses it when it would record a baseline.
    pub refuse_unreadable: bool,
    /// MCP turns failed reads during a record into one tool error; the CLI keeps its per-read
    /// diagnostics and exit status.
    pub refuse_record_failures: bool,
    /// Use the entities of this export document as the ancestor instead of the recorded baseline.
    /// Read-only: it cannot be combined with `record`.
    pub ancestor: Option<std::path::PathBuf>,
}

#[derive(Debug)]
pub struct StatusOutcome {
    pub statuses: Vec<status::EntityStatus>,
    pub failures: Vec<String>,
    pub(crate) unreadable: Vec<String>,
    pub recorded: Option<usize>,
    /// How many entities the `ancestor` export held, when one was given.
    pub ancestor: Option<usize>,
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); no adapter reads it yet"
    )]
    effects: Effects,
}

impl StatusOutcome {
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); no adapter reads it yet"
    )]
    pub(crate) const fn effects(&self) -> Effects {
        self.effects
    }
}

#[derive(Debug)]
pub enum StatusCommandError {
    Lock(lock::LockError),
    Target,
    Project(String),
    Resolve(workspace::WorkspaceError),
    Unreadable(Vec<String>),
    Profile(profile::ProfileError),
    Baseline(baseline::BaselineError),
    RecordFailures(Vec<String>),
    AncestorWithRecord,
    Ancestor {
        path: std::path::PathBuf,
        why: String,
    },
}

impl fmt::Display for StatusCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(f),
            Self::Target => write!(f, "name an entity, or pass --all"),
            Self::Project(name) => write!(f, "this solution has no project named {name}"),
            Self::Resolve(error) => error.fmt(f),
            Self::Unreadable(items) => write!(f, "{}", items.join("\n")),
            Self::Profile(error) => error.fmt(f),
            Self::Baseline(error) => error.fmt(f),
            Self::RecordFailures(failures) => write!(
                f,
                "nothing was recorded: {} entity read(s) failed: {}",
                failures.len(),
                failures.join("; ")
            ),
            Self::AncestorWithRecord => write!(
                f,
                "--ancestor compares with an export and records nothing; drop --record"
            ),
            Self::Ancestor { path, why } => write!(f, "{}: {why}", path.display()),
        }
    }
}

impl std::error::Error for StatusCommandError {}

impl Coded for StatusCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Target => ErrorCode::InvalidArguments,
            Self::Project(_) => ErrorCode::InvalidArguments,
            Self::Resolve(error) => error.code(),
            Self::Unreadable(_) => ErrorCode::InvalidData,
            Self::Profile(error) => error.code(),
            Self::Baseline(error) => error.code(),
            Self::RecordFailures(_) => ErrorCode::Unclassified,
            Self::AncestorWithRecord => ErrorCode::InvalidArguments,
            Self::Ancestor { .. } => ErrorCode::InvalidData,
        }
    }
}

/// Compare the requested entities. Recording locks before discovery so the baseline describes
/// precisely the workspace this invocation observed.
pub fn execute<R, F>(
    solution: &Solution,
    request: &StatusRequest,
    open: F,
    notices: &mut Notices,
    progress: &dyn Progress,
) -> Result<StatusOutcome, StatusCommandError>
where
    R: push::Remote + Sync,
    F: FnOnce(profile::Profile) -> R,
{
    if request.record && request.ancestor.is_some() {
        return Err(StatusCommandError::AncestorWithRecord);
    }
    let _lock = if request.record {
        Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(StatusCommandError::Lock)?,
        )
    } else {
        None
    };
    let outcome = compare(solution, request, open, progress)?;
    behind_sidecars(solution, request, notices);
    Ok(outcome)
}

/// Status compares entity files; a sidecar edited since the last sync is not in them yet, so
/// say so rather than let "in sync" stand for work the comparison never saw.
fn behind_sidecars(solution: &Solution, request: &StatusRequest, notices: &mut Notices) {
    let mut entities = workspace::discover(solution).entities;
    if let Some(project) = &request.project {
        entities.retain(|entity| &entity.found_under == project);
    }
    // Only the entities this status compared.
    if let StatusTarget::Names(names) = &request.target {
        let chosen: Vec<_> = names
            .iter()
            .filter_map(|name| workspace::resolve(&entities, name).ok().cloned())
            .collect();
        entities = chosen;
    }
    let report = crate::core::workflow::sync(
        solution,
        &entities,
        &[],
        crate::core::workflow::SyncOptions {
            check: true,
            allow_structural: false,
            relayout: false,
            named: false,
        },
    );
    if report.changed > 0 {
        notices.push(format!(
            "{} entity file(s) are behind their sidecars, which this status does not compare; `twaco sync --all` brings them in",
            report.changed
        ));
    }
    // A service or field added or removed in the sidecars stops a plain sync.
    if report.failed > 0 {
        notices.push(format!(
            "{} entity file(s) have sidecars a plain sync cannot bring in (a service or field added or removed, or one it cannot read); `twaco sync --all` says which, and --allow-add-remove takes an addition or removal",
            report.failed
        ));
    }
}

/// [`execute`] for a caller that already holds the workspace lock, so what it wrote and the
/// baseline recorded for it are one unit of work.
pub(crate) fn execute_holding<R, F>(
    solution: &Solution,
    request: &StatusRequest,
    open: F,
    _held: &crate::core::lock::WorkspaceLock,
    progress: &dyn Progress,
) -> Result<StatusOutcome, StatusCommandError>
where
    R: push::Remote + Sync,
    F: FnOnce(profile::Profile) -> R,
{
    compare(solution, request, open, progress)
}

fn compare<R, F>(
    solution: &Solution,
    request: &StatusRequest,
    open: F,
    progress: &dyn Progress,
) -> Result<StatusOutcome, StatusCommandError>
where
    R: push::Remote + Sync,
    F: FnOnce(profile::Profile) -> R,
{
    if request.record && request.ancestor.is_some() {
        return Err(StatusCommandError::AncestorWithRecord);
    }
    let found = workspace::discover(solution);
    if request.refuse_unreadable && !found.unreadable.is_empty() {
        return Err(StatusCommandError::Unreadable(found.unreadable));
    }
    let mut pool = found.entities;
    if let Some(project) = &request.project {
        if solution.project(project).is_none() {
            return Err(StatusCommandError::Project(project.clone()));
        }
        pool.retain(|entity| &entity.found_under == project);
    }
    let chosen = match &request.target {
        StatusTarget::All => pool,
        StatusTarget::Names(names) => {
            if names.is_empty() {
                return Err(StatusCommandError::Target);
            }
            names
                .iter()
                .map(|name| workspace::resolve(&pool, name).cloned())
                .collect::<Result<Vec<_>, _>>()
                .map_err(StatusCommandError::Resolve)?
        }
    };
    let profile =
        profile::load(&solution.root, &request.profile).map_err(StatusCommandError::Profile)?;
    let rules = solution.server_owned_for(&request.profile);
    let patches = solution.patches_for(&request.profile);
    let (mut baseline, ancestor) = match &request.ancestor {
        Some(path) => {
            let ancestor = |why: String| StatusCommandError::Ancestor {
                path: path.clone(),
                why,
            };
            let bytes = std::fs::read(path).map_err(|error| ancestor(error.to_string()))?;
            let (ancestor, count) =
                status::ancestor_from_export(&bytes, &rules).map_err(ancestor)?;
            (ancestor, Some(count))
        }
        None => (
            Baseline::load(&solution.root, &request.profile)
                .map_err(StatusCommandError::Baseline)?,
            None,
        ),
    };
    let (mut statuses, failures) = status::compute_with_progress(
        &open(profile),
        &baseline,
        &chosen,
        &rules,
        patches,
        progress,
    );
    if request.record && !failures.is_empty() {
        if request.refuse_record_failures {
            return Err(StatusCommandError::RecordFailures(failures));
        }
        return Ok(StatusOutcome {
            statuses,
            failures,
            unreadable: found.unreadable,
            recorded: None,
            ancestor,
            effects: Effects::new(Access::Read, Access::Read),
        });
    }
    let recorded = if request.record {
        let recorded = status::record_matching(&mut baseline, &mut statuses);
        baseline
            .write(&solution.root, &request.profile)
            .map_err(StatusCommandError::Baseline)?;
        Some(recorded)
    } else {
        None
    };
    let workspace = if request.record {
        Access::Write
    } else {
        Access::Read
    };
    Ok(StatusOutcome {
        statuses,
        failures,
        unreadable: found.unreadable,
        recorded,
        ancestor,
        effects: Effects::new(workspace, Access::Read),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity::key::EntityKey;
    use crate::core::lock;
    use crate::core::server::ServerError;

    /// The server's copy of every entity.
    struct Fake(Vec<u8>);

    impl push::Remote for Fake {
        fn fetch(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(Some(self.0.clone()))
        }
        fn export_owned(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(Some(self.0.clone()))
        }
        fn import(&self, _: &str, _: &[u8]) -> Result<(), ServerError> {
            unreachable!("status never imports")
        }
    }

    fn thing(description: &str) -> String {
        format!(
            "<Entities><Things><Thing description=\"{description}\" name=\"P.T\" projectName=\"P\"></Thing></Things></Entities>"
        )
    }

    /// A solution whose one Thing reads `working` and which has no baseline at all.
    fn without_baseline(working: &str) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        std::fs::write(root.join("Things/P.T.xml"), working).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn ancestor(
        solution: &Solution,
        export: &str,
        server: &str,
        record: bool,
    ) -> Result<StatusOutcome, StatusCommandError> {
        let path = solution.root.join("export.xml");
        std::fs::write(&path, export).unwrap();
        execute(
            solution,
            &StatusRequest {
                target: StatusTarget::All,
                project: None,
                record,
                profile: "default".to_string(),
                lock_label: "status",
                refuse_unreadable: true,
                refuse_record_failures: false,
                ancestor: Some(path),
            },
            |_| Fake(server.as_bytes().to_vec()),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
    }

    #[test]
    fn an_export_as_the_ancestor_says_which_side_moved_without_a_baseline() {
        let (_dir, solution) = without_baseline(&thing("local"));
        let verdict = |export: &str| {
            let outcome = ancestor(&solution, export, &thing("server"), false).unwrap();
            assert_eq!(outcome.ancestor, Some(1));
            outcome.statuses[0].verdict
        };
        assert_eq!(verdict(&thing("local")), status::Verdict::ServerChanged);
        assert_eq!(verdict(&thing("server")), status::Verdict::LocalChanged);
        assert_eq!(verdict(&thing("other")), status::Verdict::BothChanged);
        let elsewhere = "<Entities><Things><Thing name=\"P.Other\"></Thing></Things></Entities>";
        assert_eq!(verdict(elsewhere), status::Verdict::NoBaselineDiffers);
        assert!(!Baseline::path(&solution.root, "default").exists());
    }

    #[test]
    fn an_export_as_the_ancestor_records_nothing_and_must_hold_entities() {
        let (_dir, solution) = without_baseline(&thing("local"));
        assert!(matches!(
            ancestor(&solution, &thing("local"), &thing("local"), true),
            Err(StatusCommandError::AncestorWithRecord)
        ));
        let error = ancestor(&solution, "<Entities/>", &thing("local"), false).unwrap_err();
        assert!(
            matches!(error, StatusCommandError::Ancestor { .. }),
            "{error}"
        );
        assert_eq!(error.code(), ErrorCode::InvalidData);
        let prefixed = "<tw:Entities xmlns:tw=\"urn:x\"><tw:Things><tw:Thing name=\"P.T\"/></tw:Things></tw:Entities>";
        let error = ancestor(&solution, prefixed, &thing("local"), false).unwrap_err();
        assert!(error.to_string().contains("namespace prefix"), "{error}");
    }

    #[test]
    fn recording_one_profile_does_not_change_another_profiles_status() {
        let (_dir, solution) = without_baseline(&thing("current"));
        std::fs::write(
            solution.root.join(".twaco/profiles/development.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();

        let request = |profile: &str, record| StatusRequest {
            target: StatusTarget::All,
            project: None,
            record,
            profile: profile.to_string(),
            lock_label: "status",
            refuse_unreadable: true,
            refuse_record_failures: false,
            ancestor: None,
        };
        let server = thing("current");
        let recorded = execute(
            &solution,
            &request("development", true),
            |_| Fake(server.as_bytes().to_vec()),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        assert_eq!(recorded.recorded, Some(1));
        let default = execute(
            &solution,
            &request("default", false),
            |_| Fake(server.as_bytes().to_vec()),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        assert_eq!(default.statuses[0].verdict, status::Verdict::NoBaselineSame);
        assert!(!Baseline::path(&solution.root, "default").exists());
    }

    #[test]
    fn a_profile_patch_is_hashed_before_status_compares_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Mashups")).unwrap();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            r#"[[project]]
name = "P"
[profiles.qa]
[[profiles.qa.patches]]
entity = "Mashups/P.Main"
path = "UI.Properties.Master"
value = "P.Qa"
"#,
        )
        .unwrap();
        let working = "<Entities><Mashups><Mashup name=\"P.Main\" projectName=\"P\"><mashupContent><![CDATA[{\"UI\":{\"Properties\":{\"Master\":\"P.Dev\"}}}]]></mashupContent></Mashup></Mashups></Entities>";
        let server = working.replace("P.Dev", "P.Qa");
        std::fs::write(root.join("Mashups/P.Main.xml"), working).unwrap();
        for profile in ["qa", "default"] {
            std::fs::write(
                root.join(format!(".twaco/profiles/{profile}.toml")),
                "url='http://example.invalid/'\nusername='u'\npassword='p'\n",
            )
            .unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let request = |profile: &str| StatusRequest {
            target: StatusTarget::All,
            project: None,
            record: false,
            profile: profile.into(),
            lock_label: "status",
            refuse_unreadable: true,
            refuse_record_failures: false,
            ancestor: None,
        };
        let qa = execute(
            &solution,
            &request("qa"),
            |_| Fake(server.as_bytes().to_vec()),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        let default = execute(
            &solution,
            &request("default"),
            |_| Fake(server.as_bytes().to_vec()),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
        .unwrap();
        assert_eq!(qa.statuses[0].verdict, status::Verdict::NoBaselineSame);
        assert_eq!(
            default.statuses[0].verdict,
            status::Verdict::NoBaselineDiffers
        );
    }

    #[test]
    fn a_read_only_status_needs_no_lock_but_recording_takes_one_before_discovery() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-status-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = StatusRequest {
            target: StatusTarget::All,
            project: None,
            record: false,
            profile: "default".to_string(),
            lock_label: "status",
            refuse_unreadable: false,
            refuse_record_failures: false,
            ancestor: None,
        };
        assert!(matches!(
            execute::<crate::core::server::Client, _>(
                &solution,
                &request,
                crate::core::server::Client::new,
                &mut Notices::default(),
                &crate::core::progress::NONE
            ),
            Err(StatusCommandError::Profile(_))
        ));
        let mut recording = request;
        recording.record = true;
        assert!(matches!(
            execute::<crate::core::server::Client, _>(
                &solution,
                &recording,
                crate::core::server::Client::new,
                &mut Notices::default(),
                &crate::core::progress::NONE
            ),
            Err(StatusCommandError::Lock(_))
        ));
        drop(held);
    }
}
