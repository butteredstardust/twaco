//! The command policy around one entity push.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::backup;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity::key::{EntityKey, KeyError};
use crate::core::progress::{self, Progress};
use crate::core::{lock, profile, push, workspace};
use std::fmt;
use std::path::PathBuf;

/// The arguments that affect an entity push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushRequest {
    /// The entity spelling supplied by the caller.
    pub entity: String,
    pub mode: Mode,
    pub force: bool,
    pub skip_checks: bool,
    pub backup: bool,
    pub profile: String,
}

/// One completed entity push, before either adapter projects it to its wire format.
#[derive(Debug)]
pub enum PushOutcome {
    Plan {
        entity: EntityKey,
        decision: push::Decision,
        server_owned: Vec<String>,
        effects: Effects,
    },
    Applied {
        entity: EntityKey,
        server_owned: Vec<String>,
        result: push::Outcome,
        backup: Option<String>,
        effects: Effects,
    },
}

impl PushOutcome {
    /// The access this specific outcome implies.
    #[cfg(test)]
    pub(crate) const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }
}

const PLAN_EFFECTS: Effects = Effects::new(Access::Read, Access::Read);
const ALREADY_THERE_EFFECTS: Effects = Effects::new(Access::Write, Access::Read);
const PUSHED_EFFECTS: Effects = Effects::new(Access::Write, Access::Write);

/// A remote that can both push an entity and save the current server copy before a forced push.
pub trait Remote: push::Remote + backup::Remote + crate::core::check::ScriptChecker {}

impl<T: push::Remote + backup::Remote + crate::core::check::ScriptChecker + ?Sized> Remote for T {}

/// A failure before a typed push outcome could be produced.
#[derive(Debug)]
pub enum PushCommandError {
    Lock(lock::LockError),
    Gates(String),
    Dependency(String),
    Resolve(workspace::WorkspaceError),
    Unreadable(Vec<String>),
    Read {
        path: PathBuf,
        why: std::io::Error,
    },
    Profile(profile::ProfileError),
    Key {
        label: String,
        why: KeyError,
    },
    Backup {
        label: String,
        why: backup::BackupError,
    },
    Push {
        label: String,
        why: push::PushError,
        backup: Option<String>,
    },
}

impl PushCommandError {
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Key { label, .. } | Self::Backup { label, .. } | Self::Push { label, .. } => {
                Some(label)
            }
            Self::Gates(_)
            | Self::Dependency(_)
            | Self::Lock(_)
            | Self::Resolve(_)
            | Self::Unreadable(_)
            | Self::Read { .. }
            | Self::Profile(_) => None,
        }
    }

    /// The backup already made before a later push failure, if there was one.
    pub fn backup(&self) -> Option<&str> {
        match self {
            Self::Push { backup, .. } => backup.as_deref(),
            _ => None,
        }
    }
}

impl fmt::Display for PushCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gates(why) | Self::Dependency(why) => f.write_str(why),
            Self::Lock(why) => why.fmt(f),
            Self::Resolve(why) => why.fmt(f),
            Self::Unreadable(unreadable) => write!(f, "{}", unreadable.join("\n")),
            Self::Read { path, why } => write!(f, "{}: {why}", path.display()),
            Self::Profile(why) => why.fmt(f),
            Self::Key { label, why } => write!(f, "{label}: {why}"),
            Self::Backup { label, why } => write!(f, "{label}: {why}"),
            Self::Push { label, why, .. } => {
                write!(f, "{label}: {why}")?;
                if let Some(hint) = missing_dependency_hint(&why.to_string()) {
                    write!(f, "; {hint}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for PushCommandError {}

impl Coded for PushCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Gates(_) | Self::Dependency(_) => ErrorCode::InvalidData,
            Self::Lock(why) => why.code(),
            Self::Resolve(why) => why.code(),
            Self::Unreadable(_) => ErrorCode::InvalidData,
            Self::Read { .. } => ErrorCode::IoError,
            Self::Profile(why) => why.code(),
            Self::Key { .. } => ErrorCode::InvalidArguments,
            Self::Backup { why, .. } => why.code(),
            Self::Push { why, .. } => why.code(),
        }
    }
}

/// [`execute_with_progress`] without a progress reporter.
#[cfg(test)]
pub(crate) fn execute<R, F>(
    solution: &Solution,
    request: &PushRequest,
    open: F,
    notices: &mut Notices,
) -> Result<PushOutcome, PushCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    execute_with_progress(solution, request, open, notices, &progress::NONE)
}

/// Execute one push. Applying takes the workspace lock before discovery because recording the
/// baseline is a workspace write; planning reads without taking it.
///
/// It reports the one entity pushed as one step. The message is the entity's collection and name.
pub fn execute_with_progress<R, F>(
    solution: &Solution,
    request: &PushRequest,
    open: F,
    notices: &mut Notices,
    progress: &dyn Progress,
) -> Result<PushOutcome, PushCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    execute_with_gate_reporter(solution, request, open, notices, progress, None)
}

/// Execute a push with structured gate reports instead of gate notices when a reporter is supplied.
pub fn execute_with_gate_reporter<R, F>(
    solution: &Solution,
    request: &PushRequest,
    open: F,
    notices: &mut Notices,
    progress: &dyn Progress,
    mut reporter: Option<&mut dyn FnMut(&crate::core::check::CheckReport)>,
) -> Result<PushOutcome, PushCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => {
            Some(lock_workspace(solution, "entity push", notices).map_err(PushCommandError::Lock)?)
        }
    };
    let found = workspace::discover(solution);
    let entity =
        workspace::resolve(&found.entities, &request.entity).map_err(PushCommandError::Resolve)?;
    if !found.unreadable.is_empty() {
        return Err(PushCommandError::Unreadable(found.unreadable));
    }
    let bytes = std::fs::read(&entity.path).map_err(|why| PushCommandError::Read {
        path: entity.path.clone(),
        why,
    })?;
    let mut gates = if request.skip_checks {
        notices.push(format!(
            "skipped gates: {}",
            crate::core::check::entity_gate_names(solution).join(", ")
        ));
        None
    } else {
        Some(crate::core::check::run_entity(solution, entity))
    };
    if let Some(report) = &gates {
        report_gates(report, notices, &mut reporter)?;
    }
    let profile =
        profile::load(&solution.root, &request.profile).map_err(PushCommandError::Profile)?;
    let bytes = crate::core::patch::apply(
        &bytes,
        &entity.info.collection,
        &entity.info.name,
        solution.patches_for(&request.profile),
    )
    .map_err(|why| PushCommandError::Read {
        path: entity.path.clone(),
        why: std::io::Error::other(why),
    })?;
    let remote = open(profile);
    let label = format!("{}/{}", entity.info.collection, entity.info.name);
    let key = EntityKey::new(&entity.info.collection, &entity.info.name).map_err(|why| {
        PushCommandError::Key {
            label: label.clone(),
            why,
        }
    })?;
    let file_name = entity
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{}.xml", entity.info.name));
    let target = push::Target {
        derived: crate::core::permissions::derived::Derived::read(&found.entities)
            .map_err(|e| PushCommandError::Unreadable(vec![e.to_string()]))?
            .target(&entity.info.collection, &entity.info.name),
        key: key.clone(),
        document: push::EntityDocument {
            file_name: &file_name,
            bytes: &bytes,
        },
    };
    if let Some(report) = &mut gates {
        report.gates.clear();
        report.gates.push(crate::core::check::live_parse_entity(
            solution,
            &remote,
            progress,
            &entity.path,
            &bytes,
        ));
        report_gates(report, notices, &mut reporter)?;
    }
    let index = crate::core::index::Index::build(solution);
    let prerequisites = crate::core::deploy::import_prerequisites(&bytes);
    for reference in index.references_from(&key) {
        if reference.kind.confidence() >= crate::core::index::Confidence::Resolved
            && reference.to != key.to_string()
            && !prerequisites.contains(&reference.to)
        {
            notices.push(format!(
                "{label}: reference {} is advisory; it is not an import prerequisite",
                reference.to
            ));
        }
    }
    for reference in prerequisites {
        let Ok(dependency) = EntityKey::parse(&reference) else {
            continue;
        };
        if dependency == key {
            continue;
        }
        if push::Remote::fetch(&remote, &dependency)
            .map_err(|why| {
                PushCommandError::Dependency(format!("{label}: cannot check {dependency}: {why}"))
            })?
            .is_none()
        {
            return Err(PushCommandError::Dependency(format!("{label}: referenced {dependency} is absent from the server; run `twaco entity push {dependency} --apply` first, or `twaco deploy --apply`")));
        }
    }
    let backup = if matches!(request.mode, Mode::Apply) && request.force && request.backup {
        backup::before_forced_push(
            &remote,
            solution,
            &request.profile,
            &target,
            &solution.server_owned_for(&request.profile),
            &backup::new_stamp(),
        )
        .map_err(|why| PushCommandError::Backup {
            label: label.clone(),
            why,
        })?
    } else {
        None
    };
    let parts = crate::core::server_owned::parts(
        &solution.server_owned_for(&request.profile),
        &entity.info.collection,
        &entity.info.name,
    );
    let server_owned = crate::core::server_owned::labels(&parts);
    let result = {
        // A plan sends nothing, so its phase must not say that it pushes.
        let phase = match request.mode {
            Mode::Plan => "planning push",
            Mode::Apply => "pushing entity",
        };
        let _phase = progress::phase(progress, phase, Some(1));
        progress.message(&label);
        let result = push::push_with_parts(
            &remote,
            &solution.root,
            &request.profile,
            &target,
            matches!(request.mode, Mode::Apply),
            request.force,
            &parts,
        )
        .map_err(|why| PushCommandError::Push {
            label,
            why,
            backup: backup.clone(),
        })?;
        progress.advance(1);
        result
    };
    match result {
        push::Outcome::WouldDo(decision) => Ok(PushOutcome::Plan {
            server_owned,
            entity: key,
            decision,
            effects: PLAN_EFFECTS,
        }),
        push::Outcome::AlreadyThere => Ok(PushOutcome::Applied {
            server_owned,
            entity: key,
            result,
            backup,
            effects: ALREADY_THERE_EFFECTS,
        }),
        push::Outcome::Pushed { .. } => {
            if backup.is_some() {
                backup::settle(solution);
            }
            Ok(PushOutcome::Applied {
                server_owned,
                entity: key,
                result,
                backup,
                effects: PUSHED_EFFECTS,
            })
        }
        push::Outcome::Refused(_) => Ok(PushOutcome::Applied {
            server_owned,
            entity: key,
            result,
            backup,
            effects: PLAN_EFFECTS,
        }),
    }
}

fn missing_dependency_hint(message: &str) -> Option<String> {
    if !["not found", "does not exist", "missing", "unknown"]
        .iter()
        .any(|word| message.to_ascii_lowercase().contains(word))
    {
        return None;
    }
    let pattern = regex::Regex::new(r#"(?i)\b(DataShape|ThingTemplate|ThingShape|Thing|Mashup|ValueStream)[\s:\[\"']+([A-Za-z0-9_.-]+)"#).ok()?;
    let captures = pattern.captures(message)?;
    let collection = match captures[1].to_ascii_lowercase().as_str() {
        "datashape" => "DataShapes",
        "thingtemplate" => "ThingTemplates",
        "thingshape" => "ThingShapes",
        "mashup" => "Mashups",
        "valuestream" => "ValueStreams",
        _ => "Things",
    };
    Some(format!(
        "run `twaco entity push {collection}/{} --apply` first, or `twaco deploy --apply`",
        &captures[2]
    ))
}

fn report_gates(
    report: &crate::core::check::CheckReport,
    notices: &mut Notices,
    reporter: &mut Option<&mut dyn FnMut(&crate::core::check::CheckReport)>,
) -> Result<(), PushCommandError> {
    let mut lines = Vec::new();
    for gate in &report.gates {
        lines.push(format!(
            "{}: {} finding(s), {} advisory finding(s){}",
            gate.name,
            gate.findings.len(),
            gate.warnings.len(),
            if gate.gates_the_run {
                ""
            } else {
                " (advisory)"
            }
        ));
        lines.extend(
            gate.findings
                .iter()
                .chain(&gate.warnings)
                .map(ToString::to_string),
        );
        lines.extend(gate.prose.iter().cloned());
        if let Some(why) = &gate.broken {
            lines.push(format!("{}: {why}", gate.name));
        }
        if let Some(why) = &gate.skipped {
            lines.push(format!("{} skipped: {why}", gate.name));
        }
    }
    if let Some(reporter) = reporter {
        reporter(report);
    } else if !report.blocks() {
        notices.extend(lines.clone());
    }
    if report.blocks() {
        return Err(PushCommandError::Gates(format!("gates block entity push; nothing was imported; fix the findings or pass --skip-checks\n{}", lines.join("\n"))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execute<R, F>(
        solution: &Solution,
        request: &PushRequest,
        open: F,
    ) -> Result<PushOutcome, PushCommandError>
    where
        R: Remote,
        F: FnOnce(profile::Profile) -> R,
    {
        super::execute(solution, request, open, &mut Notices::default())
    }

    use crate::core::baseline::Baseline;
    use crate::core::normalise;
    use crate::core::server::ServerError;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Fake {
        state: Arc<Mutex<FakeState>>,
    }

    struct FakeState {
        entities: Option<std::collections::BTreeMap<String, Vec<u8>>>,
        held: Option<Vec<u8>>,
        imports: usize,
        exports: usize,
        fail_export: bool,
        fail_import: bool,
        fail_parse: bool,
        parsed: usize,
    }

    impl Fake {
        fn new(held: Option<Vec<u8>>) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeState {
                    entities: None,
                    held,
                    imports: 0,
                    exports: 0,
                    fail_export: false,
                    fail_import: false,
                    fail_parse: false,
                    parsed: 0,
                })),
            }
        }

        fn imports(&self) -> usize {
            self.state.lock().unwrap().imports
        }

        fn exports(&self) -> usize {
            self.state.lock().unwrap().exports
        }
    }

    impl crate::core::check::ScriptChecker for Fake {
        fn check_script(&self, _: &str) -> Result<crate::core::server::ScriptCheck, ServerError> {
            self.state.lock().unwrap().parsed += 1;
            Ok(crate::core::server::ScriptCheck {
                status: !self.state.lock().unwrap().fail_parse,
                line_number: 2,
                column_number: 1,
                message: "invalid script".into(),
            })
        }
    }

    impl push::Remote for Fake {
        fn fetch(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            let state = self.state.lock().unwrap();
            Ok(match &state.entities {
                Some(entities) => entities.get(&key.to_string()).cloned(),
                None => state.held.clone(),
            })
        }

        fn import(&self, _: &str, xml: &[u8]) -> Result<(), ServerError> {
            let mut state = self.state.lock().unwrap();
            state.imports += 1;
            if state.fail_import {
                return Err(ServerError::InvalidUrl("import failed".to_string()));
            }
            if let Some(entities) = &mut state.entities {
                let info = crate::core::entity::parse(xml).unwrap();
                entities.insert(format!("{}/{}", info.collection, info.name), xml.to_vec());
            }
            state.held = Some(xml.to_vec());
            Ok(())
        }
    }

    impl backup::Remote for Fake {
        fn export(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            let mut state = self.state.lock().unwrap();
            state.exports += 1;
            if state.fail_export {
                return Err(ServerError::InvalidUrl("backup failed".to_string()));
            }
            Ok(state.held.clone())
        }

        fn import(&self, _: &str, xml: &[u8]) -> Result<(), ServerError> {
            push::Remote::import(self, "", xml)
        }

        fn exists(&self, _: &EntityKey) -> Result<bool, ServerError> {
            Ok(self.state.lock().unwrap().held.is_some())
        }
    }

    fn document(script: &str) -> Vec<u8> {
        format!(
            "<Entities><Things><Thing name=\"Acme.T\" projectName=\"Acme\"><code><![CDATA[{script}]]></code></Thing></Things></Entities>"
        )
        .into_bytes()
    }

    fn setup(
        server: Option<Vec<u8>>,
        baseline: Option<(&[u8], &[u8])>,
    ) -> (tempfile::TempDir, PathBuf, Solution, Fake) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-push-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"Acme\"\n").unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        let working = document("working();");
        std::fs::write(root.join("Things/Acme.T.xml"), &working).unwrap();
        if let Some((local, remote)) = baseline {
            let mut stored = Baseline::load(&root, "default").unwrap();
            stored.set(
                "Things",
                "Acme.T",
                normalise::hash(local).unwrap(),
                normalise::hash(remote).unwrap(),
            );
            stored.write(&root, "default").unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution, Fake::new(server))
    }

    fn request(mode: Mode, force: bool, backup: bool) -> PushRequest {
        PushRequest {
            entity: "Acme.T".to_string(),
            mode,
            force,
            skip_checks: true,
            backup,
            profile: "default".to_string(),
        }
    }

    fn service_document(script: &str) -> Vec<u8> {
        format!("<Entities><Things><Thing name=\"Acme.T\" projectName=\"Acme\"><ThingShape><ServiceDefinitions><ServiceDefinition name=\"Run\"/></ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>").into_bytes()
    }

    #[test]
    fn gates_block_plans_and_applies_before_import_even_with_force() {
        for mode in [Mode::Plan, Mode::Apply] {
            let (_guard, root, solution, fake) = setup(None, None);
            std::fs::write(
                root.join("Things/Acme.T.xml"),
                service_document("for (var i = 0; i < 6; i++) {\n    const row = i;\n}"),
            )
            .unwrap();
            let mut request = request(mode, true, true);
            request.skip_checks = false;
            let error = execute(&solution, &request, |_| fake.clone()).unwrap_err();
            assert!(error.to_string().contains("const-in-loop"), "{error}");
            assert!(error.to_string().contains("Things/Acme.T.xml:2"), "{error}");
            assert_eq!(fake.imports(), 0);
            assert_eq!(fake.exports(), 0);
        }
    }

    #[test]
    fn live_parse_is_scoped_and_fails_closed_before_import() {
        let (_guard, root, solution, fake) = setup(None, None);
        std::fs::write(
            root.join("Things/Acme.T.xml"),
            service_document("var result = 1;"),
        )
        .unwrap();
        fake.state.lock().unwrap().fail_parse = true;
        let mut request = request(Mode::Apply, false, false);
        request.skip_checks = false;
        let error = execute(&solution, &request, |_| fake.clone()).unwrap_err();
        assert!(error.to_string().contains("rhino"), "{error}");
        assert_eq!(fake.imports(), 0);
    }

    #[test]
    fn structured_reports_include_offline_and_live_gates_without_gate_notices() {
        for mode in [Mode::Plan, Mode::Apply] {
            for fail_parse in [false, true] {
                let (_guard, root, solution, fake) = setup(None, None);
                std::fs::write(
                    root.join("Things/Acme.T.xml"),
                    service_document("var result = 1;"),
                )
                .unwrap();
                fake.state.lock().unwrap().fail_parse = fail_parse;
                let mut request = request(mode, false, false);
                request.skip_checks = false;
                let mut notices = Notices::default();
                let mut counts = Vec::new();
                let mut findings = Vec::new();
                let mut reporter = |report: &crate::core::check::CheckReport| {
                    counts.push(report.gates.len());
                    findings.extend(
                        report
                            .gates
                            .iter()
                            .filter(|gate| gate.name == "live parse")
                            .flat_map(|gate| gate.findings.iter().map(ToString::to_string)),
                    );
                };
                let result = execute_with_gate_reporter(
                    &solution,
                    &request,
                    |_| fake.clone(),
                    &mut notices,
                    &progress::NONE,
                    Some(&mut reporter),
                );
                assert_eq!(counts, [16, 1]);
                assert!(notices.is_empty(), "{:?}", notices.lines());
                assert_eq!(result.is_err(), fail_parse);
                if fail_parse {
                    assert_eq!(findings.len(), 1);
                    assert!(findings[0].contains("Things/Acme.T.xml:2"));
                    assert_eq!(fake.imports(), 0);
                } else {
                    assert!(findings.is_empty());
                    assert_eq!(fake.imports(), usize::from(mode == Mode::Apply));
                }
            }
        }
    }

    #[test]
    fn advisories_are_visible_and_other_entities_do_not_block_or_get_parsed() {
        let (_guard, root, mut solution, fake) = setup(None, None);
        let trap = "for (var i = 0; i < 6; i++) {\n    const row = i;\n}";
        std::fs::write(
            root.join("Things/Acme.Other.xml"),
            String::from_utf8(service_document(trap))
                .unwrap()
                .replace("Acme.T", "Acme.Other"),
        )
        .unwrap();
        std::fs::write(
            root.join("Things/Acme.T.xml"),
            service_document("var result = 1;"),
        )
        .unwrap();
        let mut request = request(Mode::Plan, false, false);
        request.skip_checks = false;
        let mut notices = Notices::default();
        super::execute(&solution, &request, |_| fake.clone(), &mut notices).unwrap();
        assert_eq!(fake.state.lock().unwrap().parsed, 1);
        assert_eq!(fake.imports(), 0);
        assert!(!notices
            .lines()
            .iter()
            .any(|line| line.contains("const-in-loop")));
        std::fs::write(
            root.join("twaco.toml"),
            "[gates]\nadvisory = [\"script traps\"]\n[[project]]\nname = \"Acme\"\n",
        )
        .unwrap();
        solution = Solution::load(&root.join("twaco.toml")).unwrap();
        std::fs::write(root.join("Things/Acme.T.xml"), service_document(trap)).unwrap();
        request.mode = Mode::Apply;
        let mut notices = Notices::default();
        super::execute(&solution, &request, |_| fake.clone(), &mut notices).unwrap();
        assert_eq!(fake.imports(), 1);
        assert!(notices
            .lines()
            .iter()
            .any(|line| line.contains("const-in-loop")));
        assert!(notices
            .lines()
            .iter()
            .any(|line| line.contains("(advisory)")));
    }

    #[test]
    fn missing_references_name_the_push_order_and_server_errors_do_too() {
        let (_guard, root, solution, fake) = setup(None, None);
        std::fs::create_dir_all(root.join("DataShapes")).unwrap();
        std::fs::write(root.join("DataShapes/Acme.Rows.xml"), "<Entities><DataShapes><DataShape name=\"Acme.Rows\" projectName=\"Acme\"/></DataShapes></Entities>").unwrap();
        std::fs::write(root.join("Things/Acme.T.xml"), "<Entities><Things><Thing name=\"Acme.T\" projectName=\"Acme\"><ThingShape><PropertyDefinitions><PropertyDefinition name=\"Rows\" baseType=\"INFOTABLE\" aspect.dataShape=\"Acme.Rows\"/></PropertyDefinitions></ThingShape></Thing></Things></Entities>").unwrap();
        let error = execute(&solution, &request(Mode::Apply, false, false), |_| {
            fake.clone()
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("entity push DataShapes/Acme.Rows --apply` first"),
            "{error}"
        );
        assert_eq!(fake.imports(), 0);
        assert!(
            missing_dependency_hint("DataShape [Acme.External] does not exist")
                .unwrap()
                .contains("DataShapes/Acme.External")
        );
        assert!(missing_dependency_hint("connection reset").is_none());
    }

    #[test]
    fn mutually_calling_new_things_can_be_pushed_individually() {
        for skip_checks in [false, true] {
            let (_guard, root, solution, fake) = setup(None, None);
            fake.state.lock().unwrap().entities = Some(Default::default());
            for (name, other) in [("Acme.T", "Acme.Other"), ("Acme.Other", "Acme.T")] {
                let xml =
                    String::from_utf8(service_document(&format!("Things[\"{other}\"].Run();")))
                        .unwrap();
                let xml = xml.replacen("name=\"Acme.T\"", &format!("name=\"{name}\""), 1);
                std::fs::write(root.join(format!("Things/{name}.xml")), xml).unwrap();
            }
            for (index, name) in ["Acme.T", "Acme.Other"].iter().enumerate() {
                let mut request = request(Mode::Apply, false, false);
                request.entity = name.to_string();
                request.skip_checks = skip_checks;
                let mut notices = Notices::default();
                let outcome =
                    super::execute(&solution, &request, |_| fake.clone(), &mut notices).unwrap();
                assert!(matches!(
                    outcome,
                    PushOutcome::Applied {
                        result: push::Outcome::Pushed { .. },
                        ..
                    }
                ));
                assert_eq!(fake.imports(), index + 1);
                assert!(notices
                    .lines()
                    .iter()
                    .any(|line| line.contains("reference") && line.contains("advisory")));
            }
            assert_eq!(
                fake.state.lock().unwrap().entities.as_ref().unwrap().len(),
                2
            );
        }
    }

    #[test]
    fn typed_import_prerequisites_block_without_runtime_or_local_index_edges() {
        for (body, dependency) in [
            ("<ImplementedShapes><ImplementedShape name=\"Acme.Shape\"/></ImplementedShapes>", "ThingShapes/Acme.Shape"),
            ("<ThingShape><PropertyDefinitions><PropertyDefinition name=\"Rows\" baseType=\"INFOTABLE\" aspect.dataShape=\"Acme.Rows\"/></PropertyDefinitions></ThingShape>", "DataShapes/Acme.Rows"),
            ("<ConfigurationTables><ConfigurationTable name=\"Acme.Table\" dataShapeName=\"Acme.Rows\"/></ConfigurationTables>", "DataShapes/Acme.Rows"),
            ("<ThingProperties><dataShape>Acme.Rows</dataShape></ThingProperties>", "DataShapes/Acme.Rows"),
            ("<ThingProperties><valueStream>Acme.Values</valueStream></ThingProperties>", "Things/Acme.Values"),
            ("<ConfigurationTables><ConfigurationTable name=\"StreamProcessingSettings\"><Rows><Row><queueProvider><![CDATA[Acme.Queue]]></queueProvider></Row></Rows></ConfigurationTable></ConfigurationTables>", "QueueProviders/Acme.Queue"),
        ] {
            for mode in [Mode::Plan, Mode::Apply] {
                let (_guard, root, solution, fake) = setup(None, None);
                std::fs::write(root.join("Things/Acme.T.xml"), format!("<Entities><Things><Thing name=\"Acme.T\" projectName=\"Acme\">{body}</Thing></Things></Entities>")).unwrap();
                let error = execute(&solution, &request(mode, false, false), |_| fake.clone()).unwrap_err();
                assert!(error.to_string().contains(dependency), "{error}");
                assert_eq!(fake.imports(), 0);
            }
        }
        for attribute in ["thingTemplate", "valueStream", "queueProvider"] {
            let (_guard, root, solution, fake) = setup(None, None);
            std::fs::write(root.join("Things/Acme.T.xml"), format!("<Entities><Things><Thing name=\"Acme.T\" projectName=\"Acme\" {attribute}=\"Acme.Required\"/></Things></Entities>")).unwrap();
            let error = execute(&solution, &request(Mode::Apply, false, false), |_| {
                fake.clone()
            })
            .unwrap_err();
            assert!(error.to_string().contains("Acme.Required"), "{error}");
            assert_eq!(fake.imports(), 0);
        }
    }

    #[test]
    fn a_push_reports_one_step_for_its_entity() {
        use crate::core::progress::{Event, Recorder};
        let (_guard, _root, solution, fake) = setup(None, None);
        let recorder = Recorder::default();
        execute_with_progress(
            &solution,
            &request(Mode::Apply, false, false),
            |_| fake.clone(),
            &mut Notices::default(),
            &recorder,
        )
        .unwrap();
        assert_eq!(recorder.phases(), [("pushing entity".to_string(), Some(1))]);
        assert_eq!(recorder.advanced(), 1);
        assert!(recorder
            .events()
            .contains(&Event::Message("Things/Acme.T".to_string())));
        assert_eq!(recorder.events().last(), Some(&Event::Finish));
    }

    #[test]
    fn a_push_plan_does_not_report_that_it_pushes() {
        use crate::core::progress::Recorder;
        let (_guard, _root, solution, fake) = setup(None, None);
        let recorder = Recorder::default();
        execute_with_progress(
            &solution,
            &request(Mode::Plan, false, false),
            |_| fake.clone(),
            &mut Notices::default(),
            &recorder,
        )
        .unwrap();
        assert_eq!(recorder.phases(), [("planning push".to_string(), Some(1))]);
    }

    #[test]
    fn outcome_effects_match_the_push_result() {
        let key = EntityKey::new("Things", "Acme.T").unwrap();
        let plan = PushOutcome::Plan {
            server_owned: Vec::new(),
            entity: key.clone(),
            decision: push::Decision::Create,
            effects: PLAN_EFFECTS,
        };
        assert_eq!(plan.effects(), Effects::new(Access::Read, Access::Read));
        for (result, effects) in [
            (push::Outcome::AlreadyThere, ALREADY_THERE_EFFECTS),
            (push::Outcome::Pushed { created: false }, PUSHED_EFFECTS),
            (
                push::Outcome::Refused(push::Refusal::DeletedOnServer),
                PLAN_EFFECTS,
            ),
        ] {
            let outcome = PushOutcome::Applied {
                server_owned: Vec::new(),
                entity: key.clone(),
                result,
                backup: None,
                effects,
            };
            assert_eq!(outcome.effects(), effects);
        }
    }

    #[test]
    fn every_mode_force_backup_and_decision_combination_keeps_its_policy() {
        let working = document("working();");
        let old = document("old();");
        let changed = document("changed();");
        let cases = [
            (
                "already there",
                Some(working.clone()),
                None,
                push::Decision::AlreadyThere,
            ),
            ("create", None, None, push::Decision::Create),
            (
                "update",
                Some(old.clone()),
                Some((old.as_slice(), old.as_slice())),
                push::Decision::Update,
            ),
            (
                "deleted",
                None,
                Some((old.as_slice(), old.as_slice())),
                push::Decision::Refuse(push::Refusal::DeletedOnServer),
            ),
            (
                "unknown ancestor",
                Some(changed.clone()),
                None,
                push::Decision::Refuse(push::Refusal::UnknownAncestor {
                    server: normalise::hash(&changed).unwrap(),
                    policy_changed: false,
                }),
            ),
            (
                "conflict",
                Some(changed.clone()),
                Some((old.as_slice(), old.as_slice())),
                push::Decision::Refuse(push::Refusal::Conflict {
                    server: normalise::hash(&changed).unwrap(),
                    baseline: normalise::hash(&old).unwrap(),
                }),
            ),
        ];
        for (name, server, baseline, expected) in cases {
            for mode in [Mode::Plan, Mode::Apply] {
                for force in [false, true] {
                    for backup in [false, true] {
                        let (_dir, _, solution, remote) = setup(server.clone(), baseline);
                        let outcome = execute(&solution, &request(mode, force, backup), {
                            let remote = remote.clone();
                            move |_| remote
                        })
                        .unwrap_or_else(|error| {
                            panic!("{name}: {mode:?}, {force}, {backup}: {error}")
                        });
                        match outcome {
                            PushOutcome::Plan { decision, .. } => {
                                assert_eq!(mode, Mode::Plan, "{name}");
                                assert_eq!(decision, expected, "{name}");
                                assert_eq!(remote.imports(), 0, "{name}");
                                assert_eq!(remote.exports(), 0, "{name}");
                            }
                            PushOutcome::Applied {
                                result,
                                backup: saved,
                                ..
                            } => {
                                assert_eq!(mode, Mode::Apply, "{name}");
                                let refused = matches!(expected, push::Decision::Refuse(_));
                                match result {
                                    push::Outcome::AlreadyThere => {
                                        assert_eq!(expected, push::Decision::AlreadyThere)
                                    }
                                    push::Outcome::Pushed { .. } => {
                                        assert!(!refused || force, "{name}")
                                    }
                                    push::Outcome::Refused(refusal) => {
                                        assert!(!force, "{name}");
                                        assert_eq!(expected, push::Decision::Refuse(refusal));
                                    }
                                    push::Outcome::WouldDo(_) => unreachable!(),
                                }
                                let imported = match expected {
                                    push::Decision::AlreadyThere => 0,
                                    push::Decision::Refuse(_) if !force => 0,
                                    _ => 1,
                                };
                                assert_eq!(remote.imports(), imported, "{name}");
                                let saved_backup = matches!(
                                    expected,
                                    push::Decision::Refuse(
                                        push::Refusal::UnknownAncestor { .. }
                                            | push::Refusal::Conflict { .. }
                                    )
                                ) && force
                                    && backup;
                                assert_eq!(remote.exports(), usize::from(saved_backup), "{name}");
                                assert_eq!(saved.is_some(), saved_backup, "{name}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_forced_backup_failure_pushes_nothing_and_no_backup_skips_it() {
        let old = document("old();");
        let changed = document("changed();");
        let (_dir, _, solution, remote) = setup(
            Some(changed.clone()),
            Some((old.as_slice(), old.as_slice())),
        );
        remote.state.lock().unwrap().fail_export = true;
        let error = execute(&solution, &request(Mode::Apply, true, true), {
            let remote = remote.clone();
            move |_| remote
        })
        .unwrap_err();
        assert!(matches!(error, PushCommandError::Backup { .. }));
        assert_eq!(remote.imports(), 0);
        assert_eq!(remote.exports(), 1);

        let (_dir, _, solution, remote) =
            setup(Some(changed), Some((old.as_slice(), old.as_slice())));
        execute(&solution, &request(Mode::Apply, true, false), {
            let remote = remote.clone();
            move |_| remote
        })
        .unwrap();
        assert_eq!(remote.imports(), 1);
        assert_eq!(remote.exports(), 0);
    }

    #[test]
    fn a_forced_push_that_fails_prunes_no_older_backup() {
        let old = document("old();");
        let changed = document("changed();");
        let (_dir, root, solution, remote) = setup(
            Some(changed.clone()),
            Some((old.as_slice(), old.as_slice())),
        );
        // A full store of older sets.
        for at in 0..backup::KEEP {
            let dir = root.join(backup::DIR).join(format!("2026-{at:04}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("backup.json"),
                format!(r#"{{"created":"2026-{at:04}","reason":"r","entities":[]}}"#),
            )
            .unwrap();
        }
        assert_eq!(backup::list(&solution).len(), backup::KEEP);
        remote.state.lock().unwrap().fail_import = true;
        let failed = execute(&solution, &request(Mode::Apply, true, true), {
            let remote = remote.clone();
            move |_| remote
        });
        assert!(failed.is_err());
        assert_eq!(
            backup::list(&solution).len(),
            backup::KEEP + 1,
            "the failed push's set is kept, and no older one is pruned"
        );
        remote.state.lock().unwrap().fail_import = false;
        execute(&solution, &request(Mode::Apply, true, true), {
            let remote = remote.clone();
            move |_| remote
        })
        .unwrap();
        assert_eq!(backup::list(&solution).len(), backup::KEEP);
    }

    #[test]
    fn applying_locks_before_discovery_and_planning_does_not_lock() {
        let (_dir, root, solution, remote) = setup(None, None);
        let held = lock::acquire(&root, "test holder", &[]).unwrap();
        let plan = execute(&solution, &request(Mode::Plan, false, true), {
            let remote = remote.clone();
            move |_| remote
        });
        assert!(matches!(plan, Ok(PushOutcome::Plan { .. })));
        let applied = execute(&solution, &request(Mode::Apply, false, true), move |_| {
            remote
        });
        assert!(matches!(applied, Err(PushCommandError::Lock(_))));
        drop(held);
    }

    #[test]
    fn command_errors_keep_the_wrapped_stable_code() {
        let remote = ServerError::InvalidUrl("bad URL".to_string());
        let error = PushCommandError::Push {
            label: "Things/Acme.T".to_string(),
            why: push::PushError::Remote(remote),
            backup: None,
        };
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        let error = PushCommandError::Push {
            label: "Things/Acme.T".to_string(),
            why: push::PushError::Unverified(ServerError::InvalidUrl("bad URL".to_string())),
            backup: None,
        };
        assert_eq!(error.code(), ErrorCode::NotVerified);
    }
}
