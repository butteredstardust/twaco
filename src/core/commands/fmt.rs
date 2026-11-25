//! The command policy around formatting service sidecars.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{bundle, lock, workflow, workspace};

/// The arguments that affect formatting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FmtRequest {
    pub mode: Mode,
    /// Entity names or Collection/Name globs whose scripts to format.
    pub entities: Vec<String>,
    /// Select entity files or sidecars changed since this Git revision.
    pub changed_since: Option<String>,
    /// With no entity and no revision given, format only what changed against `HEAD` (plus
    /// untracked files) and count the rest, falling back to everything outside a git
    /// repository. Never for a check: a gate judges the whole solution.
    pub changed_default: bool,
    /// The lock-holder label the calling surface has historically shown.
    pub lock_label: &'static str,
}

/// One completed format run, before either adapter projects it to its wire format.
#[derive(Debug)]
pub struct FmtOutcome {
    pub report: workflow::FmtOutcome,
    /// Set when the run was narrowed to the changed files by default: how many other files
    /// are still unformatted (a plan over the rest).
    pub others_unformatted: Option<usize>,
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); read only by tests"
    )]
    effects: Effects,
}

impl FmtOutcome {
    /// The access this format run used or would use.
    #[cfg(test)]
    pub(crate) const fn effects(&self) -> Effects {
        self.effects
    }
}

/// A failure before a format outcome could be produced.
#[derive(Debug, thiserror::Error)]
pub enum FmtCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Selection(String),
}

impl Coded for FmtCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
            Self::Selection(_) => ErrorCode::InvalidArguments,
        }
    }
}

/// Format scripts, subscription Events and RBAC sets. A check is a plan and does not take the workspace lock.
pub fn execute(
    solution: &Solution,
    request: &FmtRequest,
    notices: &mut Notices,
) -> Result<FmtOutcome, FmtCommandError> {
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices).map_err(FmtCommandError::Lock)?,
        ),
    };
    let mut changed_since = request.changed_since.clone();
    let mut narrowed = false;
    if request.changed_default
        && request.mode == Mode::Apply
        && request.entities.is_empty()
        && changed_since.is_none()
    {
        if bundle::validate_changed_since(solution, "HEAD").is_ok() {
            changed_since = Some("HEAD".into());
            narrowed = true;
        } else {
            notices.push(
                "not a git repository with a commit: formatting the whole solution".to_string(),
            );
        }
    }
    // Chosen under the lock, so what is formatted is what was selected.
    let selected = if request.entities.is_empty() && changed_since.is_none() {
        None
    } else {
        let found = workspace::discover(solution);
        let selected = bundle::select_included(found.entities, &request.entities)
            .map_err(FmtCommandError::Selection)?;
        let (selected, notes) =
            bundle::select_entities(solution, selected, changed_since.as_deref(), &[])
                .map_err(FmtCommandError::Selection)?;
        for note in notes {
            notices.push(note);
        }
        Some(selected)
    };
    let report = workflow::fmt(
        solution,
        matches!(request.mode, Mode::Plan),
        selected.as_deref(),
        _lock.as_ref(),
    );
    let others_unformatted =
        narrowed.then(|| workflow::fmt(solution, true, None, None).changed.len());
    let workspace = match request.mode {
        Mode::Plan => Access::Read,
        Mode::Apply => Access::Write,
    };
    Ok(FmtOutcome {
        report,
        others_unformatted,
        effects: Effects::new(workspace, Access::None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;

    fn request() -> FmtRequest {
        FmtRequest {
            mode: Mode::Apply,
            entities: Vec::new(),
            changed_since: None,
            changed_default: false,
            lock_label: "fmt",
        }
    }

    fn entity(name: &str) -> String {
        format!(
            concat!(
                "<Entities><Things><Thing name=\"{}\" projectName=\"Acme.App\"><ThingShape>",
                "<ServiceDefinitions><ServiceDefinition name=\"Run\"/></ServiceDefinitions>",
                "<ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\">",
                "<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>",
                "<code><![CDATA[if(true){{\nresult=1;\n}}]]></code>",
                "</Row></Rows></ConfigurationTable></ConfigurationTables>",
                "</ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>"
            ), name
        )
    }

    fn fixture() -> (tempfile::TempDir, Solution) {
        let root_guard = tempfile::tempdir().unwrap();
        let root = root_guard.path();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"Acme.App\"\n",
        )
        .unwrap();
        for name in ["Acme.One", "Acme.Two"] {
            std::fs::write(root.join(format!("Things/{name}.xml")), entity(name)).unwrap();
            let service = root.join(format!("src/{name}/services/Run"));
            std::fs::create_dir_all(&service).unwrap();
            std::fs::write(service.join("script.js"), "if(true){\nresult=1;\n}").unwrap();
            std::fs::write(
                service.join("definition.xml"),
                "<ServiceDefinition name=\"Run\"/>\n",
            )
            .unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, solution)
    }

    fn git(root: &std::path::Path, arguments: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn a_format_plan_needs_no_lock_but_an_apply_does() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-fmt-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let planned = execute(
            &solution,
            &FmtRequest {
                mode: Mode::Plan,
                entities: Vec::new(),
                changed_since: None,
                changed_default: false,
                lock_label: "fmt",
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(planned.effects(), Effects::new(Access::Read, Access::None));
        let error = execute(
            &solution,
            &FmtRequest {
                mode: Mode::Apply,
                entities: Vec::new(),
                changed_since: None,
                changed_default: false,
                lock_label: "fmt",
            },
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(matches!(error, FmtCommandError::Lock(_)));
        drop(held);
    }

    #[test]
    fn entity_selector_formats_only_its_scripts_and_check_respects_the_scope() {
        let (root_guard, solution) = fixture();
        let root = root_guard.path();
        let one = root.join("src/Acme.One/services/Run/script.js");
        let two = root.join("src/Acme.Two/services/Run/script.js");
        let two_before = std::fs::read(&two).unwrap();
        let outcome = execute(
            &solution,
            &FmtRequest {
                entities: vec!["Acme.One".into()],
                ..request()
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.report.files, 1);
        assert_eq!(outcome.report.failed, 0, "{:?}", outcome.report.log);
        assert_eq!(outcome.report.synced, 1);
        assert_eq!(
            outcome.report.changed,
            vec![root.join("Things/Acme.One.xml"), one.clone()]
        );
        assert_ne!(std::fs::read(&one).unwrap(), std::fs::read(&two).unwrap());
        assert_eq!(std::fs::read(&two).unwrap(), two_before);

        let outcome = execute(
            &solution,
            &FmtRequest {
                mode: Mode::Plan,
                entities: vec!["Things/Acme.Two".into()],
                ..request()
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.report.files, 1);
        assert_eq!(outcome.report.changed, vec![two.clone()]);
        assert_eq!(std::fs::read(&two).unwrap(), two_before);
    }

    #[test]
    fn changed_since_formats_only_changed_entity_sidecars() {
        let (root_guard, solution) = fixture();
        let root = root_guard.path();
        git(root, &["init", "-q"]);
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.name=Acme Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        let one = root.join("src/Acme.One/services/Run/script.js");
        let two = root.join("src/Acme.Two/services/Run/script.js");
        let one_before = std::fs::read(&one).unwrap();
        std::fs::write(&two, "if(true){\nresult=2;\n}").unwrap();
        let outcome = execute(
            &solution,
            &FmtRequest {
                changed_since: Some("HEAD".into()),
                ..request()
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.report.files, 1);
        assert_eq!(
            outcome.report.changed,
            vec![root.join("Things/Acme.Two.xml"), two]
        );
        assert_eq!(std::fs::read(&one).unwrap(), one_before);

        let outcome = execute(
            &solution,
            &FmtRequest {
                entities: vec!["Acme.One".into()],
                changed_since: Some("HEAD".into()),
                ..request()
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.report.files, 0);
        assert!(outcome.report.changed.is_empty());
    }

    fn committed_fixture() -> (tempfile::TempDir, Solution) {
        let (root_guard, solution) = fixture();
        let root = root_guard.path();
        git(root, &["init", "-q"]);
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.name=Acme Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        (root_guard, solution)
    }

    #[test]
    fn plain_fmt_formats_only_changed_and_untracked_files_and_counts_the_rest() {
        let (root_guard, solution) = committed_fixture();
        let root = root_guard.path();
        let one = root.join("src/Acme.One/services/Run/script.js");
        let two = root.join("src/Acme.Two/services/Run/script.js");
        let one_before = std::fs::read(&one).unwrap();
        // Changed against HEAD (still unformatted), and a new untracked entity.
        std::fs::write(
            &two,
            "if(true){
result=2;
}",
        )
        .unwrap();
        std::fs::write(root.join("Things/Acme.Three.xml"), entity("Acme.Three")).unwrap();
        let three = root.join("src/Acme.Three/services/Run");
        std::fs::create_dir_all(&three).unwrap();
        std::fs::write(
            three.join("script.js"),
            "if(true){
result=3;
}",
        )
        .unwrap();
        std::fs::write(
            three.join("definition.xml"),
            "<ServiceDefinition name=\"Run\"/>
",
        )
        .unwrap();
        let outcome = execute(
            &solution,
            &FmtRequest {
                changed_default: true,
                ..request()
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.report.failed, 0, "{:?}", outcome.report.log);
        assert!(outcome.report.changed.contains(&two));
        assert!(outcome.report.changed.contains(&three.join("script.js")));
        assert_eq!(std::fs::read(&one).unwrap(), one_before);
        // The committed, unformatted entity is reported, not touched.
        assert!(outcome.others_unformatted.unwrap() >= 1);

        // --check judges everything, even with the default scope asked for.
        let check = execute(
            &solution,
            &FmtRequest {
                mode: Mode::Plan,
                changed_default: true,
                ..request()
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert!(check.others_unformatted.is_none());
        assert!(check.report.changed.contains(&one));

        // --all formats everything.
        let all = execute(&solution, &request(), &mut Notices::default()).unwrap();
        assert!(all.others_unformatted.is_none());
        assert_ne!(std::fs::read(&one).unwrap(), one_before);
    }

    #[test]
    fn plain_fmt_outside_git_formats_everything_with_a_note() {
        let (_root_guard, solution) = fixture();
        let mut notices = Notices::default();
        let outcome = execute(
            &solution,
            &FmtRequest {
                changed_default: true,
                ..request()
            },
            &mut notices,
        )
        .unwrap();
        assert!(outcome.others_unformatted.is_none());
        assert_eq!(outcome.report.files, 2);
        assert!(notices
            .lines()
            .iter()
            .any(|note| note.contains("whole solution")));
    }

    #[test]
    fn unmatched_entity_selector_writes_nothing() {
        let (root_guard, solution) = fixture();
        let root = root_guard.path();
        let one = root.join("src/Acme.One/services/Run/script.js");
        let two = root.join("src/Acme.Two/services/Run/script.js");
        let before = [std::fs::read(&one).unwrap(), std::fs::read(&two).unwrap()];
        let error = execute(
            &solution,
            &FmtRequest {
                entities: vec!["Things/Acme.Missing".into()],
                ..request()
            },
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Things/Acme.Missing"));
        assert_eq!(std::fs::read(&one).unwrap(), before[0]);
        assert_eq!(std::fs::read(&two).unwrap(), before[1]);
    }

    #[test]
    fn unscoped_fmt_keeps_formatting_sidecars_without_an_entity_file() {
        let root_guard = tempfile::tempdir().unwrap();
        let root = root_guard.path();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"Acme.App\"\n",
        )
        .unwrap();
        let service = root.join("src/Acme.Orphan/services/Run");
        std::fs::create_dir_all(&service).unwrap();
        let script = service.join("script.js");
        std::fs::write(&script, "if(true){\nresult=1;\n}").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();

        let outcome = execute(&solution, &request(), &mut Notices::default()).unwrap();
        assert_eq!(outcome.report.files, 1);
        assert_eq!(outcome.report.changed, vec![script]);
    }
}
