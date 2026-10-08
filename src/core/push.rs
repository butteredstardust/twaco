//! Pushing one entity to a server, refusing when that would overwrite someone else's work.
//!
//! The question a push has to answer is not "does the server differ from my file" (it always
//! will, or there would be nothing to push) but "has the server changed since I last saw it".
//! That needs a two-sided baseline: the normalised hashes last seen for the working copy and
//! server. W and S are compared to their respective recorded sides.
//!
//! **This is detection, not prevention.** Someone can save in Composer between the check and
//! the import, and nothing here can stop that. What the push *can* do is look afterwards: it
//! reads the entity back and compares, so it never records a baseline for a state it did not
//! observe on the server.

use super::baseline::{Baseline, BaselineError};
use super::entity_key::EntityKey;
use super::normalise::{self, NormaliseError};
use super::server::{Client, ServerError};
use std::fmt;
use std::path::Path;

/// What a push would do, decided before anything is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The server already holds the working copy, or both sides match their recorded states.
    AlreadyThere,
    /// The entity is not on the server and never was, as far as the baseline knows.
    Create,
    /// The server is unchanged since the last sync, so the push overwrites nothing unseen.
    Update,
    /// A refusal, which `--force` overrides.
    Refuse(Refusal),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The server baseline says the entity was present; now it is not.
    DeletedOnServer,
    /// The server has the entity but there is no baseline, so who changed what is unknowable.
    UnknownAncestor { server: String },
    /// The server changed since its recorded baseline.
    Conflict { server: String, baseline: String },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::DeletedOnServer => write!(
                f,
                "the entity was deleted on the server since the last sync; pushing would recreate it"
            ),
            Refusal::UnknownAncestor { server } => write!(
                f,
                "the server has a different version ({server}) and there is no baseline to say \
                 which side changed; compare with `twaco entity status` first"
            ),
            Refusal::Conflict { server, baseline } => write!(
                f,
                "the server changed since the last sync (server baseline {baseline}, server now {server}); \
                 pushing would overwrite that change"
            ),
        }
    }
}

/// The decision table. Pure, so every row is testable.
pub fn decide(working: &str, server: Option<&str>, baseline: Option<(&str, &str)>) -> Decision {
    match (server, baseline) {
        (Some(server), _) if server == working => Decision::AlreadyThere,
        (None, None) => Decision::Create,
        (None, Some(_)) => Decision::Refuse(Refusal::DeletedOnServer),
        (Some(server), None) => Decision::Refuse(Refusal::UnknownAncestor {
            server: server.to_string(),
        }),
        (Some(server), Some((local, server_baseline)))
            if working == local && server == server_baseline =>
        {
            Decision::AlreadyThere
        }
        (Some(server), Some((_, server_baseline))) if server == server_baseline => Decision::Update,
        (Some(server), Some((_, server_baseline))) => Decision::Refuse(Refusal::Conflict {
            server: server.to_string(),
            baseline: server_baseline.to_string(),
        }),
    }
}

/// The two server operations a push needs. A trait so the orchestration can be tested without
/// a server; [`Client`] is the real one.
pub trait Remote {
    fn fetch(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError>;
    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), ServerError>;
}

impl Remote for Client {
    fn fetch(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
        match self.fetch_entity(key) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), ServerError> {
        self.import_entity(file_name, xml)
    }
}

/// The entity being pushed.
pub struct Target<'a> {
    pub key: EntityKey,
    pub document: EntityDocument<'a>,
}

/// The bytes and importer file name for an entity being pushed.
pub struct EntityDocument<'a> {
    pub file_name: &'a str,
    pub bytes: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A dry run: this is what `--apply` would do.
    WouldDo(Decision),
    /// Nothing needed importing; an equal pair is recorded and a differing pair is preserved.
    AlreadyThere,
    /// Imported, read back, and matching.
    Pushed { created: bool },
    /// Refused, and nothing was sent.
    Refused(Refusal),
}

#[derive(Debug)]
pub enum PushError {
    /// The working copy cannot be hashed, including a file that holds more than one entity.
    Working(NormaliseError),
    /// The server's copy cannot be hashed.
    Server(NormaliseError),
    Remote(ServerError),
    /// The import succeeded but the entity could not be read back. The server has changed to
    /// something not yet observed, and the baseline was left alone.
    Unverified(ServerError),
    Baseline(BaselineError),
    /// The import said success, but the entity read back is not what was sent. The baseline
    /// was left alone, so `entity status` still shows the difference.
    NotKept {
        sent: String,
        read_back: Option<String>,
    },
}

impl fmt::Display for PushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PushError::Working(error) => write!(f, "the working copy cannot be pushed: {error}"),
            PushError::Server(error) => write!(f, "the server's copy cannot be read: {error}"),
            PushError::Remote(error) => write!(f, "{error}"),
            PushError::Unverified(error) => write!(
                f,
                "the import succeeded, but reading the entity back failed, so what the server now \
                 holds is unknown; the baseline was not updated. Run `twaco entity status` once \
                 the server answers ({error})"
            ),
            PushError::Baseline(error) => write!(f, "{error}"),
            PushError::NotKept { sent, read_back } => write!(
                f,
                "the import reported success, but the server did not keep what was sent \
                 (sent {sent}, read back {}); the baseline was not updated. Known causes: a \
                 configuration table the template does not define is dropped, and a file \
                 missing sections the server always writes (a hand-written one, typically) \
                 comes back with them filled in. `twaco entity get` shows what the server kept",
                read_back
                    .as_deref()
                    .unwrap_or("nothing: the entity is missing")
            ),
        }
    }
}

impl std::error::Error for PushError {}

/// Decide, and with `apply`, act. `force` overrides a refusal and nothing else.
///
/// The baseline is loaded and written as late as possible, and only through
/// [`Baseline::write`], which replaces the file atomically. Only this entity's key changes.
pub fn push(
    remote: &dyn Remote,
    root: &Path,
    target: &Target,
    apply: bool,
    force: bool,
) -> Result<Outcome, PushError> {
    let working = normalise::hash(target.document.bytes).map_err(PushError::Working)?;
    let server = remote
        .fetch(&target.key)
        .map_err(PushError::Remote)?
        .map(|bytes| normalise::hash(&bytes))
        .transpose()
        .map_err(PushError::Server)?;
    let baseline = Baseline::load(root).map_err(PushError::Baseline)?;
    let entry = baseline.get(target.key.collection(), target.key.name());
    let decision = decide(
        &working,
        server.as_deref(),
        entry.map(|entry| (entry.local.as_str(), entry.server.as_str())),
    );

    if !apply {
        return Ok(Outcome::WouldDo(decision));
    }
    let created = match &decision {
        Decision::AlreadyThere => {
            if server.as_deref() == Some(working.as_str()) {
                record(root, target, working.clone(), working)?;
            }
            return Ok(Outcome::AlreadyThere);
        }
        Decision::Refuse(refusal) if !force => return Ok(Outcome::Refused(refusal.clone())),
        Decision::Refuse(refusal) => matches!(refusal, Refusal::DeletedOnServer),
        Decision::Create => true,
        Decision::Update => false,
    };

    remote
        .import(target.document.file_name, target.document.bytes)
        .map_err(PushError::Remote)?;
    // From here on the server has changed, so a failure must say so rather than read like the
    // fetch before the import did.
    let read_back = remote
        .fetch(&target.key)
        .map_err(PushError::Unverified)?
        .map(|bytes| normalise::hash(&bytes))
        .transpose()
        .map_err(PushError::Server)?;
    if read_back.as_deref() != Some(working.as_str()) {
        return Err(PushError::NotKept {
            sent: working,
            read_back,
        });
    }
    record(root, target, working.clone(), working)?;
    Ok(Outcome::Pushed { created })
}

fn record(root: &Path, target: &Target, local: String, server: String) -> Result<(), PushError> {
    let mut baseline = Baseline::load(root).map_err(PushError::Baseline)?;
    baseline.set(target.key.collection(), target.key.name(), local, server);
    baseline.write(root).map_err(PushError::Baseline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn entity(script: &str) -> Vec<u8> {
        format!(
            "<Entities><Things><Thing name=\"T\" projectName=\"P\"><code><![CDATA[{script}]]></code>\
             </Thing></Things></Entities>"
        )
        .into_bytes()
    }

    /// A server that holds at most one entity, keeps what it is sent unless told to drop it,
    /// and records every import.
    struct Fake {
        held: RefCell<Option<Vec<u8>>>,
        keep: bool,
        imports: RefCell<usize>,
    }

    impl Fake {
        fn holding(held: Option<Vec<u8>>) -> Self {
            Fake {
                held: RefCell::new(held),
                keep: true,
                imports: RefCell::new(0),
            }
        }
    }

    impl Remote for Fake {
        fn fetch(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(self.held.borrow().clone())
        }
        fn import(&self, _: &str, xml: &[u8]) -> Result<(), ServerError> {
            *self.imports.borrow_mut() += 1;
            if self.keep {
                *self.held.borrow_mut() = Some(xml.to_vec());
            } else {
                // What the Importer does to an undefined configuration table: says success,
                // keeps something else.
                *self.held.borrow_mut() = Some(entity("dropped();"));
            }
            Ok(())
        }
    }

    fn temp() -> std::path::PathBuf {
        let nonce = crate::test_nonce();
        let path = std::env::temp_dir().join(format!("twaco-push-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn target(bytes: &[u8]) -> Target<'_> {
        Target {
            key: EntityKey::new("Things", "T").unwrap(),
            document: EntityDocument {
                file_name: "T.xml",
                bytes,
            },
        }
    }

    #[test]
    fn a_target_keeps_its_key_and_document_separate() {
        let bytes = entity("a();");
        let target = target(&bytes);
        assert_eq!(target.key.to_string(), "Things/T");
        assert_eq!(target.document.file_name, "T.xml");
        assert_eq!(target.document.bytes, bytes);

        let root = temp();
        let fake = Fake::holding(None);
        push(&fake, &root, &target, true, false).unwrap();
        let baseline = Baseline::load(&root).unwrap();
        assert!(baseline
            .get(target.key.collection(), target.key.name())
            .is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_row_of_the_decision_table() {
        assert_eq!(decide("w", Some("w"), None), Decision::AlreadyThere);
        assert_eq!(
            decide("w", Some("w"), Some(("l", "b"))),
            Decision::AlreadyThere
        );
        assert_eq!(
            decide("l", Some("s"), Some(("l", "s"))),
            Decision::AlreadyThere
        );
        assert_eq!(decide("w", None, None), Decision::Create);
        assert_eq!(
            decide("w", None, Some(("l", "b"))),
            Decision::Refuse(Refusal::DeletedOnServer)
        );
        assert_eq!(
            decide("w", Some("s"), None),
            Decision::Refuse(Refusal::UnknownAncestor { server: "s".into() })
        );
        assert_eq!(decide("w", Some("b"), Some(("l", "b"))), Decision::Update);
        assert_eq!(
            decide("w", Some("s"), Some(("l", "b"))),
            Decision::Refuse(Refusal::Conflict {
                server: "s".into(),
                baseline: "b".into()
            })
        );
    }

    #[test]
    fn a_dry_run_imports_nothing_and_writes_no_baseline() {
        let root = temp();
        let fake = Fake::holding(None);
        let bytes = entity("a();");
        let outcome = push(&fake, &root, &target(&bytes), false, false).unwrap();
        assert_eq!(outcome, Outcome::WouldDo(Decision::Create));
        assert_eq!(*fake.imports.borrow(), 0);
        assert!(!root.join(".twaco/baseline.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn create_then_update_then_nothing_to_do() {
        let root = temp();
        let fake = Fake::holding(None);
        let first = entity("a();");
        assert_eq!(
            push(&fake, &root, &target(&first), true, false).unwrap(),
            Outcome::Pushed { created: true }
        );
        let second = entity("b();");
        assert_eq!(
            push(&fake, &root, &target(&second), true, false).unwrap(),
            Outcome::Pushed { created: false }
        );
        assert_eq!(
            push(&fake, &root, &target(&second), true, false).unwrap(),
            Outcome::AlreadyThere
        );
        assert_eq!(*fake.imports.borrow(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn differing_sides_at_their_own_baselines_need_no_import_or_recording() {
        let root = temp();
        let working = entity("local();");
        let server = entity("server();");
        let mut baseline = Baseline::default();
        baseline.set(
            "Things",
            "T",
            normalise::hash(&working).unwrap(),
            normalise::hash(&server).unwrap(),
        );
        baseline.write(&root).unwrap();
        let before = std::fs::read(root.join(super::super::baseline::RELATIVE_PATH)).unwrap();
        let fake = Fake::holding(Some(server));

        assert_eq!(
            push(&fake, &root, &target(&working), true, false).unwrap(),
            Outcome::AlreadyThere
        );
        assert_eq!(*fake.imports.borrow(), 0);
        assert_eq!(
            std::fs::read(root.join(super::super::baseline::RELATIVE_PATH)).unwrap(),
            before
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_server_side_change_is_refused_unless_forced() {
        let root = temp();
        let fake = Fake::holding(None);
        let mine = entity("mine();");
        push(&fake, &root, &target(&mine), true, false).unwrap();
        // Someone else saves in Composer.
        *fake.held.borrow_mut() = Some(entity("theirs();"));

        let edited = entity("mine2();");
        let outcome = push(&fake, &root, &target(&edited), true, false).unwrap();
        assert!(matches!(
            outcome,
            Outcome::Refused(Refusal::Conflict { .. })
        ));
        assert_eq!(*fake.imports.borrow(), 1, "a refusal sends nothing");

        let forced = push(&fake, &root, &target(&edited), true, true).unwrap();
        assert_eq!(forced, Outcome::Pushed { created: false });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_read_back_mismatch_leaves_the_baseline_untouched() {
        let root = temp();
        let mut fake = Fake::holding(None);
        let first = entity("a();");
        push(&fake, &root, &target(&first), true, false).unwrap();
        let before = std::fs::read(root.join(".twaco/baseline.json")).unwrap();

        fake.keep = false;
        let second = entity("b();");
        let error = push(&fake, &root, &target(&second), true, false).unwrap_err();
        assert!(matches!(error, PushError::NotKept { .. }), "{error}");
        assert_eq!(
            std::fs::read(root.join(".twaco/baseline.json")).unwrap(),
            before
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A server whose read-back after an import fails, as a dropped connection would.
    struct FailsAfterImport {
        imported: RefCell<bool>,
    }

    impl Remote for FailsAfterImport {
        fn fetch(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            if *self.imported.borrow() {
                Err(ServerError::Transport {
                    method: crate::core::server::Method::Get,
                    url: "http://server/Things/T".to_string(),
                    why: "connection reset".to_string(),
                })
            } else {
                Ok(None)
            }
        }
        fn import(&self, _: &str, _: &[u8]) -> Result<(), ServerError> {
            *self.imported.borrow_mut() = true;
            Ok(())
        }
    }

    #[test]
    fn a_failed_read_back_says_the_server_changed_and_records_nothing() {
        let root = temp();
        let remote = FailsAfterImport {
            imported: RefCell::new(false),
        };
        let bytes = entity("a();");
        let error = push(&remote, &root, &target(&bytes), true, false).unwrap_err();
        assert!(matches!(error, PushError::Unverified(_)), "{error}");
        assert!(error.to_string().contains("the import succeeded"));
        assert!(!root.join(".twaco/baseline.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// Every decision, with and without `--apply` and `--force`: what is imported, and whether
    /// the baseline file changes. The invariant is that nothing is sent or recorded unless the
    /// decision allows it and `--apply` asks for it.
    #[test]
    fn nothing_is_sent_or_recorded_unless_the_decision_and_the_flags_allow_it() {
        let working = entity("mine();");
        let other = entity("theirs();");
        let hash_of = |bytes: &[u8]| normalise::hash(bytes).unwrap();
        let ancestor = entity("ancestor();");
        // (label, server holds, baseline local/server, pushes when applied, only when forced,
        // records when applied without importing)
        type Row<'a> = (
            &'a str,
            Option<&'a [u8]>,
            Option<(&'a [u8], &'a [u8])>,
            bool,
            bool,
            bool,
        );
        let rows: [Row; 7] = [
            ("already there", Some(&working), None, false, false, true),
            (
                "in sync with differing sides",
                Some(&other),
                Some((&working, &other)),
                false,
                false,
                false,
            ),
            ("create", None, None, true, false, false),
            (
                "update",
                Some(&other),
                Some((&other, &other)),
                true,
                false,
                false,
            ),
            (
                "deleted on server",
                None,
                Some((&other, &other)),
                false,
                true,
                false,
            ),
            ("unknown ancestor", Some(&other), None, false, true, false),
            (
                "conflict",
                Some(&other),
                Some((&ancestor, &ancestor)),
                false,
                true,
                false,
            ),
        ];
        for (label, server, ancestor, pushes, forced, records_without_import) in rows {
            for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
                let root = temp();
                if let Some((local, server)) = ancestor {
                    let mut baseline = Baseline::default();
                    baseline.set("Things", "T", hash_of(local), hash_of(server));
                    baseline.write(&root).unwrap();
                }
                let path = root.join(".twaco/baseline.json");
                let before = std::fs::read(&path).ok();
                let fake = Fake::holding(server.map(<[u8]>::to_vec));

                push(&fake, &root, &target(&working), apply, force).unwrap();

                let sent = *fake.imports.borrow();
                let expected_sends = apply && (pushes || (forced && force));
                assert_eq!(
                    sent,
                    usize::from(expected_sends),
                    "{label} apply={apply} force={force}"
                );
                let after = std::fs::read(&path).ok();
                let records = apply && (expected_sends || records_without_import);
                if records {
                    assert_ne!(
                        after, before,
                        "{label} apply={apply} force={force}: baseline must record"
                    );
                } else {
                    assert_eq!(
                        after, before,
                        "{label} apply={apply} force={force}: baseline must not move"
                    );
                }
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }

    #[test]
    fn a_file_holding_two_entities_is_not_pushed() {
        let root = temp();
        let fake = Fake::holding(None);
        let two = b"<Entities><Things><Thing name=\"A\"></Thing><Thing name=\"B\"></Thing></Things></Entities>";
        let error = push(&fake, &root, &target(two), true, false).unwrap_err();
        assert!(matches!(error, PushError::Working(_)), "{error}");
        assert_eq!(*fake.imports.borrow(), 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
