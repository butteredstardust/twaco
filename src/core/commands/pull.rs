//! Command policy for taking server entity exports into the workspace.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::baseline::Baseline;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity::key::EntityKey;
use crate::core::progress::Progress;
use crate::core::{
    adopt, lock, profile, push, status, transaction::Transaction, workflow, workspace,
};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Names(Vec<String>),
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub target: Target,
    pub mode: Mode,
    pub profile: String,
    pub lock_label: &'static str,
}

#[derive(Debug)]
pub struct PullOutcome {
    pub entities: Vec<String>,
    pub applied: bool,
    pub recorded: usize,
    pub effects: Effects,
}

#[derive(Debug)]
pub enum PullError {
    Lock(lock::LockError),
    Target,
    Resolve(workspace::WorkspaceError),
    Unreadable(Vec<String>),
    Profile(profile::ProfileError),
    Remote(String),
    Refused(String),
    Layout(String),
    Write(crate::core::transaction::TransactionError),
    Baseline(crate::core::baseline::BaselineError),
}
impl fmt::Display for PullError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(e) => e.fmt(f),
            Self::Target => write!(f, "name an entity, or pass --all"),
            Self::Resolve(e) => e.fmt(f),
            Self::Unreadable(x) => write!(f, "{}", x.join("\n")),
            Self::Profile(e) => e.fmt(f),
            Self::Remote(e) | Self::Refused(e) | Self::Layout(e) => f.write_str(e),
            Self::Write(e) => e.fmt(f),
            Self::Baseline(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for PullError {}
impl Coded for PullError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(e) => e.code(),
            Self::Target => ErrorCode::InvalidArguments,
            Self::Resolve(e) => e.code(),
            Self::Unreadable(_) => ErrorCode::InvalidData,
            Self::Profile(e) => e.code(),
            Self::Remote(_) => ErrorCode::Unclassified,
            Self::Refused(_) => ErrorCode::ServerConflict,
            Self::Layout(_) => ErrorCode::InvalidData,
            Self::Write(e) => e.code(),
            Self::Baseline(e) => e.code(),
        }
    }
}

/// Compare first, then take only server-changed exports.  The server remains the source for
/// server-owned parts; those selectors affect comparison, not the bytes accepted here.
pub fn execute<R, F>(
    solution: &Solution,
    request: &PullRequest,
    open: F,
    notices: &mut Notices,
    progress: &dyn Progress,
) -> Result<PullOutcome, PullError>
where
    R: push::Remote + Sync,
    F: FnOnce(profile::Profile) -> R,
{
    let held = match request.mode {
        Mode::Plan => None,
        Mode::Apply => {
            Some(lock_workspace(solution, request.lock_label, notices).map_err(PullError::Lock)?)
        }
    };
    let found = workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(PullError::Unreadable(found.unreadable));
    }
    let named = matches!(request.target, Target::Names(_));
    let chosen = match &request.target {
        Target::All => found.entities,
        Target::Names(names) => {
            if names.is_empty() {
                return Err(PullError::Target);
            }
            names
                .iter()
                .map(|n| workspace::resolve(&found.entities, n).cloned())
                .collect::<Result<Vec<_>, _>>()
                .map_err(PullError::Resolve)?
        }
    };
    let profile = profile::load(&solution.root, &request.profile).map_err(PullError::Profile)?;
    let remote = open(profile);
    let baseline = Baseline::load(&solution.root).map_err(PullError::Baseline)?;
    let (statuses, failures) = status::compute_with_progress(
        &remote,
        &baseline,
        &chosen,
        &solution.server_owned,
        progress,
    );
    if !failures.is_empty() {
        return Err(PullError::Remote(failures.join("; ")));
    }
    let mut work = Vec::new();
    for entry in statuses {
        let label = format!("{}/{}", entry.collection, entry.name);
        match entry.verdict {
            status::Verdict::ServerChanged => work.push((label, entry)),
            status::Verdict::NoBaselineDiffers if named => work.push((label, entry)),
            status::Verdict::LocalChanged | status::Verdict::BothChanged => {
                return Err(PullError::Refused(format!(
                    "{label}: working file changed; use twaco adopt for a merge"
                )))
            }
            status::Verdict::NotOnServer => {
                return Err(PullError::Refused(format!(
                    "{label}: not on the server, so it cannot be pulled"
                )))
            }
            _ => {}
        }
    }
    let labels = work
        .iter()
        .map(|(label, _)| label.clone())
        .collect::<Vec<_>>();
    if matches!(request.mode, Mode::Plan) {
        return Ok(PullOutcome {
            entities: labels,
            applied: false,
            recorded: 0,
            effects: Effects::new(Access::Read, Access::Read),
        });
    }
    let mut tx = Transaction::new(&solution.root, "entity pull");
    for (_, entry) in &work {
        let file = chosen
            .iter()
            .find(|file| file.info.collection == entry.collection && file.info.name == entry.name)
            .expect("status was selected");
        let key = EntityKey::address(&entry.collection, &entry.name)
            .map_err(|e| PullError::Remote(e.to_string()))?;
        // The Exporter's copy is what an import takes back: the REST read status compares also
        // holds what the export leaves out (a group's user members).
        let exported = remote
            .export_owned(&key)
            .map_err(|e| PullError::Remote(format!("{label}: {e}", label = key)))?
            .ok_or_else(|| {
                PullError::Refused(format!("{key}: not on the server, so it cannot be pulled"))
            })?;
        let wanted = adopt::EntityRef {
            collection: entry.collection.clone(),
            name: entry.name.clone(),
        };
        let raw = adopt::raw_entity_bytes(&exported, &wanted)
            .map_err(|e| PullError::Layout(format!("{key}: {e:?}")))?;
        let old = std::fs::read(&file.path)
            .map_err(|e| PullError::Remote(format!("{}: {e}", file.path.display())))?;
        // The entity goes into the file's own envelope, in the file's line endings.
        let raw = line_endings_of(&old, raw);
        let laid =
            adopt::replace_entity_bytes(&old, &wanted, &raw, solution.format.indent_cdata_payload)
                .map_err(|e| PullError::Layout(format!("{key}: {e}")))?;
        tx.replace_file(&file.path, &old, laid)
            .map_err(PullError::Write)?;
    }
    tx.apply(held.as_ref().expect("apply holds lock"))
        .map_err(PullError::Write)?;
    let pulled = chosen
        .into_iter()
        .filter(|file| {
            work.iter().any(|(_, status)| {
                status.collection == file.info.collection && status.name == file.info.name
            })
        })
        .collect::<Vec<_>>();
    let extracted = workflow::extract(
        solution,
        &pulled,
        &[],
        true,
        held.as_ref().expect("apply holds lock"),
    );
    // The entity files are the server's now; say how to finish what stopped after them.
    let finish = |why: String| {
        PullError::Remote(format!(
            "the entity files were written, but {why}; finish with `twaco extract` for {} and `twaco entity status --all --record`",
            labels.join(", ")
        ))
    };
    if extracted.failed != 0 {
        return Err(finish(format!(
            "their sidecars could not be extracted: {}",
            extracted.log.errors().collect::<Vec<_>>().join("; ")
        )));
    }
    let mut after = Baseline::load(&solution.root).map_err(PullError::Baseline)?;
    let (mut fresh, failures) =
        status::compute_with_progress(&remote, &after, &pulled, &solution.server_owned, progress);
    if !failures.is_empty() {
        return Err(finish(format!(
            "the baseline was not recorded: {}",
            failures.join("; ")
        )));
    }
    let recorded = status::record_matching(&mut after, &mut fresh);
    after
        .write(&solution.root)
        .map_err(|error| finish(format!("the baseline was not recorded: {error}")))?;
    Ok(PullOutcome {
        entities: labels,
        applied: true,
        recorded,
        effects: Effects::new(Access::Write, Access::Read),
    })
}

/// `bytes` with the line endings `file` uses: an export is CRLF throughout.
fn line_endings_of(file: &[u8], bytes: Vec<u8>) -> Vec<u8> {
    let crlf = file.windows(2).any(|pair| pair == b"\r\n");
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    if crlf {
        text.replace('\n', "\r\n").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::normalise;
    use crate::core::server::ServerError;

    /// The server's copy of the one entity, as both reads return it.
    struct Fake(Vec<u8>);

    impl push::Remote for Fake {
        fn fetch(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(Some(self.0.clone()))
        }
        fn export_owned(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(Some(self.0.clone()))
        }
        fn import(&self, _: &str, _: &[u8]) -> Result<(), ServerError> {
            unreachable!("pull never imports")
        }
    }

    fn thing(description: &str, newline: &str) -> Vec<u8> {
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{newline}<Entities>{newline}    <Things>{newline}        <Thing description=\"{description}\" name=\"P.T\" projectName=\"P\"></Thing>{newline}    </Things>{newline}</Entities>{newline}").into_bytes()
    }

    /// A solution with `Things/P.T.xml` holding `working`, its baseline recorded from `base`.
    fn setup(working: &[u8], base: &[u8]) -> (tempfile::TempDir, Solution) {
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
        let mut baseline = Baseline::load(root).unwrap();
        let hash = normalise::hash(base).unwrap();
        baseline.set("Things", "P.T", hash.clone(), hash);
        baseline.write(root).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn pull(solution: &Solution, server: &[u8], mode: Mode) -> Result<PullOutcome, PullError> {
        execute(
            solution,
            &PullRequest {
                target: Target::All,
                mode,
                profile: "default".into(),
                lock_label: "entity pull",
            },
            |_| Fake(server.to_vec()),
            &mut Notices::default(),
            &crate::core::progress::NONE,
        )
    }

    #[test]
    fn a_server_change_is_taken_in_the_file_line_endings_and_recorded() {
        let base = thing("", "\r\n");
        let (dir, solution) = setup(&base, &base);
        // The server's copy comes in an export's shape: one line, its own envelope.
        let server = b"<Entities build=\"b1\"><Things><Thing description=\"server\" name=\"P.T\" projectName=\"P\"></Thing></Things></Entities>";
        let plan = pull(&solution, server, Mode::Plan).unwrap();
        assert_eq!(plan.entities, ["Things/P.T"]);
        assert_eq!(
            std::fs::read(dir.path().join("Things/P.T.xml")).unwrap(),
            base
        );
        let applied = pull(&solution, server, Mode::Apply).unwrap();
        assert_eq!(applied.recorded, 1);
        let after = std::fs::read_to_string(dir.path().join("Things/P.T.xml")).unwrap();
        assert!(after.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<Entities>\r\n"));
        assert!(after.contains("description=\"server\""));
        assert_eq!(after.matches('\n').count(), after.matches("\r\n").count());
        assert_eq!(
            normalise::hash(after.as_bytes()).unwrap(),
            normalise::hash(server).unwrap()
        );
    }

    #[test]
    fn a_locally_changed_entity_is_refused_and_left_alone() {
        let base = thing("", "\n");
        let working = thing("mine", "\n");
        let (dir, solution) = setup(&working, &base);
        let error = pull(&solution, &thing("theirs", "\n"), Mode::Apply).unwrap_err();
        assert!(error.to_string().contains("twaco adopt"), "{error}");
        assert_eq!(
            std::fs::read(dir.path().join("Things/P.T.xml")).unwrap(),
            working
        );
    }
}
