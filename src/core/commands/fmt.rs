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
    /// The lock-holder label the calling surface has historically shown.
    pub lock_label: &'static str,
}

/// One completed format run, before either adapter projects it to its wire format.
#[derive(Debug)]
pub struct FmtOutcome {
    pub report: workflow::FmtOutcome,
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

/// Format scripts. A check is a plan and does not take the workspace lock.
pub fn execute(
    solution: &Solution,
    request: &FmtRequest,
    notices: &mut Notices,
) -> Result<FmtOutcome, FmtCommandError> {
    let selected = if request.entities.is_empty() && request.changed_since.is_none() {
        None
    } else {
        let found = workspace::discover(solution);
        let selected = bundle::select_included(found.entities, &request.entities)
            .map_err(FmtCommandError::Selection)?;
        Some(
            bundle::select_entities(solution, selected, request.changed_since.as_deref(), &[])
                .map_err(FmtCommandError::Selection)?
                .0,
        )
    };
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices).map_err(FmtCommandError::Lock)?,
        ),
    };
    let report = workflow::fmt(
        solution,
        matches!(request.mode, Mode::Plan),
        selected.as_deref(),
    );
    let workspace = match request.mode {
        Mode::Plan => Access::Read,
        Mode::Apply => Access::Write,
    };
    Ok(FmtOutcome {
        report,
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
            lock_label: "fmt",
        }
    }

    fn entity(name: &str) -> String {
        format!(
            "<Entities><Things><Thing name=\"{name}\" projectName=\"Acme.App\"/></Things></Entities>"
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
        assert_eq!(outcome.report.changed, vec![one.clone()]);
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
        assert_eq!(outcome.report.changed, vec![two]);
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
