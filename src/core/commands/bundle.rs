//! The command policy around assembling one importable bundle.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::adopt;
use crate::core::bundle;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{lock, workspace};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleRequest {
    pub backend_only: bool,
    pub handoff: Option<String>,
    pub changed_since: Option<String>,
    pub exclude: Vec<String>,
    pub out: Option<PathBuf>,
    /// Whose mashup patches to apply; none without one.
    pub profile: Option<String>,
    pub mode: Mode,
    pub lock_label: &'static str,
}

pub enum BundleOutcome {
    Current {
        target: PathBuf,
        bundle: bundle::Bundle,
        effects: Effects,
        notes: Vec<String>,
    },
    OutOfDate {
        target: PathBuf,
        effects: Effects,
        notes: Vec<String>,
    },
    Missing {
        target: PathBuf,
        effects: Effects,
        notes: Vec<String>,
    },
    Written {
        target: PathBuf,
        bundle: bundle::Bundle,
        effects: Effects,
        notes: Vec<String>,
    },
}

impl BundleOutcome {
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); no adapter reads it yet"
    )]
    pub(crate) const fn effects(&self) -> Effects {
        match self {
            Self::Current { effects, .. }
            | Self::OutOfDate { effects, .. }
            | Self::Missing { effects, .. }
            | Self::Written { effects, .. } => *effects,
        }
    }
}

impl BundleOutcome {
    pub fn notes(&self) -> &[String] {
        match self {
            Self::Current { notes, .. }
            | Self::OutOfDate { notes, .. }
            | Self::Missing { notes, .. }
            | Self::Written { notes, .. } => notes,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BundleCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("no entity XML found under {}", .root.display())]
    Empty { root: PathBuf },
    #[error("{0}")]
    Build(bundle::BundleError),
    #[error("{}: {why}", .path.display())]
    Create { path: PathBuf, why: std::io::Error },
    #[error("{0}")]
    Write(workspace::WorkspaceError),
    #[error("{0}")]
    Handoff(adopt::AdoptError),
    #[error("{0}")]
    Selection(String),
    #[error("--out {path} is a directory")]
    OutIsDirectory { path: PathBuf },
}

impl Coded for BundleCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(error) => error.code(),
            Self::Empty { .. } | Self::Build(_) => ErrorCode::InvalidData,
            Self::Create { .. } => ErrorCode::IoError,
            Self::Write(error) => error.code(),
            Self::Handoff(error) => error.code(),
            Self::Selection(_) | Self::OutIsDirectory { .. } => ErrorCode::InvalidArguments,
        }
    }
}

/// Build a bundle, taking the lock before inspecting sources when the generated bundle changes.
pub fn execute(
    solution: &Solution,
    request: &BundleRequest,
    notices: &mut Notices,
) -> Result<BundleOutcome, BundleCommandError> {
    if request.handoff.is_some()
        && (request.changed_since.is_some()
            || !request.exclude.is_empty()
            || request.profile.is_some())
    {
        return Err(BundleCommandError::Selection(
            "--handoff cannot be combined with --changed-since, --exclude or --profile".to_string(),
        ));
    }
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices)
                .map_err(BundleCommandError::Lock)?,
        ),
    };
    let selection = if request.backend_only {
        bundle::Selection::backend(solution)
    } else {
        bundle::Selection::everything()
    };
    let found = workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(BundleCommandError::Selection(found.unreadable.join("; ")));
    }
    let candidates = found
        .entities
        .into_iter()
        .filter(|entity| {
            !request.backend_only
                || bundle::Selection::backend(solution).wants(&entity.info.collection)
        })
        .collect();
    let (selected, mut notes) = bundle::select_entities(
        solution,
        candidates,
        request.changed_since.as_deref(),
        &request.exclude,
    )
    .map_err(BundleCommandError::Selection)?;
    let selected_paths: std::collections::BTreeSet<PathBuf> =
        selected.into_iter().map(|entity| entity.path).collect();
    let files: Vec<PathBuf> = bundle::source_files(solution)
        .into_iter()
        .filter(|path| selected_paths.contains(path))
        .collect();
    notes.extend(bundle::dangling_references(
        &files,
        &bundle::source_files(solution),
    ));
    if files.is_empty() {
        return Err(BundleCommandError::Empty {
            root: solution.root.clone(),
        });
    }
    let built = bundle::build(&files, &selection).map_err(BundleCommandError::Build)?;
    let built = match &request.profile {
        Some(profile) => bundle::apply_patches(solution, built, &files, profile)
            .map_err(BundleCommandError::Selection)?,
        None => built,
    };
    let name = if request.backend_only {
        &solution.bundle.backend_name
    } else {
        &solution.bundle.name
    };
    let target = request
        .out
        .clone()
        .unwrap_or_else(|| solution.root.join(&solution.solution.dist).join(name));
    if target.is_dir() {
        return Err(BundleCommandError::OutIsDirectory { path: target });
    }
    let directory = target.parent().unwrap_or(&solution.root).to_path_buf();
    match request.mode {
        Mode::Plan => match std::fs::read(&target) {
            Ok(existing) if existing == built.bytes => Ok(BundleOutcome::Current {
                target,
                bundle: built,
                effects: Effects::new(Access::Read, Access::None),
                notes,
            }),
            Ok(_) => Ok(BundleOutcome::OutOfDate {
                target,
                effects: Effects::new(Access::Read, Access::None),
                notes,
            }),
            Err(_) => Ok(BundleOutcome::Missing {
                target,
                effects: Effects::new(Access::Read, Access::None),
                notes,
            }),
        },
        Mode::Apply => {
            std::fs::create_dir_all(&directory).map_err(|why| BundleCommandError::Create {
                path: directory,
                why,
            })?;
            workspace::write_entity(&target, &built.bytes).map_err(BundleCommandError::Write)?;
            if let Some(name) = &request.handoff {
                adopt::record_handoff(solution, name, std::slice::from_ref(&target))
                    .map_err(BundleCommandError::Handoff)?;
            }
            Ok(BundleOutcome::Written {
                target,
                bundle: built,
                effects: Effects::new(Access::Write, Access::None),
                notes,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;

    #[test]
    fn a_bundle_check_needs_no_lock_but_a_write_takes_one_first() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-bundle-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let plan = BundleRequest {
            backend_only: false,
            handoff: None,
            changed_since: None,
            exclude: Vec::new(),
            out: None,
            profile: None,
            mode: Mode::Plan,
            lock_label: "bundle",
        };
        assert!(matches!(
            execute(&solution, &plan, &mut Notices::default()),
            Err(BundleCommandError::Empty { .. })
        ));
        let apply = BundleRequest {
            mode: Mode::Apply,
            ..plan
        };
        assert!(matches!(
            execute(&solution, &apply, &mut Notices::default()),
            Err(BundleCommandError::Lock(_))
        ));
        drop(held);
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

    fn thing(name: &str, template: &str) -> String {
        format!(
            "<Entities><Things><Thing name=\"{name}\" projectName=\"P\" thingTemplate=\"{template}\"/></Things></Entities>"
        )
    }

    fn request(out: &std::path::Path) -> BundleRequest {
        BundleRequest {
            backend_only: false,
            handoff: None,
            changed_since: None,
            exclude: Vec::new(),
            out: Some(out.to_path_buf()),
            profile: None,
            mode: Mode::Apply,
            lock_label: "bundle",
        }
    }

    /// A solution in a subfolder of its repository, committed once, then changed: P.B edited,
    /// P.C's sidecar edited, P.New added untracked, P.Gone deleted, P.A and P.Tpl untouched.
    fn changed_repository() -> (tempfile::TempDir, Solution) {
        let repository = tempfile::Builder::new()
            .prefix("twaco-changed-since-")
            .tempdir()
            .unwrap();
        let root = repository.path().join("app");
        for folder in ["Things", "ThingTemplates", "src/P.C"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        for name in ["P.A", "P.B", "P.C", "P.Gone"] {
            std::fs::write(
                root.join(format!("Things/{name}.xml")),
                thing(name, "GenericThing"),
            )
            .unwrap();
        }
        std::fs::write(
            root.join("ThingTemplates/P.Tpl.xml"),
            "<Entities><ThingTemplates><ThingTemplate name=\"P.Tpl\" projectName=\"P\"/></ThingTemplates></Entities>",
        )
        .unwrap();
        std::fs::write(root.join("src/P.C/notes.md"), "one").unwrap();
        git(repository.path(), &["init", "-q"]);
        git(repository.path(), &["add", "."]);
        git(
            repository.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        std::fs::write(root.join("Things/P.B.xml"), thing("P.B", "P.Tpl")).unwrap();
        std::fs::write(root.join("src/P.C/notes.md"), "two").unwrap();
        std::fs::write(
            root.join("Things/P.New.xml"),
            thing("P.New", "GenericThing"),
        )
        .unwrap();
        std::fs::remove_file(root.join("Things/P.Gone.xml")).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (repository, solution)
    }

    #[test]
    fn changed_since_selects_changed_files_sidecars_and_new_entities_and_names_the_rest() {
        let (repository, solution) = changed_repository();
        let out = repository.path().join("out.xml");
        let outcome = execute(
            &solution,
            &BundleRequest {
                changed_since: Some("HEAD".into()),
                ..request(&out)
            },
            &mut Notices::default(),
        )
        .unwrap();
        let written = std::fs::read_to_string(&out).unwrap();
        for name in ["P.B", "P.C", "P.New"] {
            assert!(
                written.contains(&format!("name=\"{name}\"")),
                "{name}: {written}"
            );
        }
        for name in ["P.A", "P.Tpl", "P.Gone"] {
            assert!(
                !written.contains(&format!("name=\"{name}\"")),
                "{name}: {written}"
            );
        }
        let kinds: std::collections::BTreeMap<_, _> =
            bundle::notes_by_kind(outcome.notes()).into_iter().collect();
        assert_eq!(
            kinds["deleted"],
            ["deleted since HEAD: Things/P.Gone; deploy never deletes"]
        );
        assert_eq!(
            kinds["left_out_references"],
            ["P.B names P.Tpl as its thingTemplate"]
        );
        assert!(kinds["unmatched_excludes"].is_empty());
    }

    #[test]
    fn exclude_removes_by_name_or_glob_and_says_when_it_matched_nothing() {
        let (repository, solution) = changed_repository();
        let out = repository.path().join("out.xml");
        let outcome = execute(
            &solution,
            &BundleRequest {
                changed_since: Some("HEAD".into()),
                exclude: vec![
                    "Things/P.C".into(),
                    "Things/P.N*".into(),
                    "Things/Nope".into(),
                ],
                ..request(&out)
            },
            &mut Notices::default(),
        )
        .unwrap();
        let written = std::fs::read_to_string(&out).unwrap();
        assert!(written.contains("name=\"P.B\""));
        assert!(!written.contains("name=\"P.C\"") && !written.contains("name=\"P.New\""));
        assert_eq!(
            bundle::notes_by_kind(outcome.notes())
                .into_iter()
                .find(|(kind, _)| *kind == "unmatched_excludes")
                .unwrap()
                .1,
            ["exclude Things/Nope matched no selected entities"]
        );
    }

    #[test]
    fn an_unknown_revision_and_a_handoff_of_a_selection_are_refused() {
        let (repository, solution) = changed_repository();
        let out = repository.path().join("out.xml");
        let error = execute(
            &solution,
            &BundleRequest {
                changed_since: Some("no-such-rev".into()),
                ..request(&out)
            },
            &mut Notices::default(),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("no such revision"), "{error}");
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        let error = execute(
            &solution,
            &BundleRequest {
                handoff: Some("designer".into()),
                exclude: vec!["Things/P.A".into()],
                ..request(&out)
            },
            &mut Notices::default(),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("--handoff"), "{error}");
        assert!(!out.exists());
    }

    #[test]
    fn a_bundle_for_a_profile_carries_its_patches_and_one_without_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Mashups")).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\n[profiles.qa]\n[[profiles.qa.patches]]\n\
             entity = \"Mashups/P.Main\"\npath = \"UI.Properties.Master\"\nvalue = \"P.Qa\"\n",
        )
        .unwrap();
        let working = "<Entities><Mashups><Mashup name=\"P.Main\" projectName=\"P\"><mashupContent><![CDATA[{\"UI\":{\"Properties\":{\"Master\":\"P.Dev\"}}}]]></mashupContent></Mashup></Mashups></Entities>";
        std::fs::write(root.join("Mashups/P.Main.xml"), working).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let out = root.join("qa.xml");
        execute(
            &solution,
            &BundleRequest {
                profile: Some("qa".into()),
                ..request(&out)
            },
            &mut Notices::default(),
        )
        .unwrap();
        let written = std::fs::read_to_string(&out).unwrap();
        assert!(
            written.contains("P.Qa") && !written.contains("P.Dev"),
            "{written}"
        );
        let plain = root.join("plain.xml");
        execute(&solution, &request(&plain), &mut Notices::default()).unwrap();
        assert!(std::fs::read_to_string(&plain).unwrap().contains("P.Dev"));
    }
}
