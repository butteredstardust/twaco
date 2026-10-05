//! Deploy orchestration through import and read-back.
//!
//! Bundling and workspace selection live elsewhere. This module owns the server-facing state
//! machine: live-parse every script, reuse push's two-sided decision table, import projects in
//! dependency order, then verify every entity and persist all matching baselines in one write.

use super::baseline::{Baseline, BaselineError};
use super::entity_key::ServiceTarget;
use super::normalise;
use super::parallel;
use super::profile::Profile;
use super::push::{self, Decision};
use super::server::{Client, ScriptCheck, ServerError};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Script {
    pub entity: String,
    pub service: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub collection: String,
    pub name: String,
    /// The one-entity source document whose body is present in the bundle.
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ProjectBundle {
    pub project: String,
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub entities: Vec<Entity>,
    pub scripts: Vec<Script>,
    pub deploy: Option<ServiceCall>,
    pub post_import: Vec<ServiceCall>,
}

/// A configured call as it may safely appear in plans: parameters still contain placeholders.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceCall {
    pub target: String,
    pub service: String,
    pub parameters: Value,
}

impl fmt::Display for ServiceCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parameters = serde_json::to_string(&self.parameters)
            .expect("configured service parameters are JSON");
        write!(f, "{}.{} {parameters}", self.target, self.service)
    }
}

/// The server operations deploy needs. The parse method is deliberately narrow; this is not a
/// generic ThingWorx service-call interface.
pub trait Remote: push::Remote + Sync {
    fn check_script(&self, script: &str) -> Result<ScriptCheck, ServerError>;
    fn call_service(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, ServerError>;
}

impl Remote for Client {
    fn check_script(&self, script: &str) -> Result<ScriptCheck, ServerError> {
        Client::check_script(self, script)
    }

    fn call_service(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, ServerError> {
        Client::call_service(self, target, service, parameters, timeout)
    }
}

/// Baseline persistence is a seam for proving that a multi-entity deploy writes exactly once.
pub trait BaselineStore {
    fn load(&self) -> Result<Baseline, BaselineError>;
    fn write(&self, baseline: &Baseline) -> Result<(), BaselineError>;
}

pub struct DiskBaseline {
    root: PathBuf,
}

impl DiskBaseline {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }
}

impl BaselineStore for DiskBaseline {
    fn load(&self) -> Result<Baseline, BaselineError> {
        Baseline::load(&self.root)
    }

    fn write(&self, baseline: &Baseline) -> Result<(), BaselineError> {
        baseline.write(&self.root)
    }
}

#[derive(Clone, Debug)]
pub struct EntityPlan {
    pub project: String,
    pub collection: String,
    pub name: String,
    pub decision: Decision,
}

#[derive(Clone, Debug)]
pub struct NotKept {
    pub collection: String,
    pub name: String,
    pub sent: String,
    pub read_back: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub scripts_checked: usize,
    pub projects: Vec<String>,
    pub plans: Vec<EntityPlan>,
    pub imported: Vec<String>,
    pub kept: Vec<(String, String)>,
    pub not_kept: Vec<NotKept>,
    /// Placeholder-bearing calls only; resolved values never enter a report.
    pub calls: Vec<PlannedCall>,
    pub changed_by_deploy: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedCall {
    pub project: String,
    pub call: ServiceCall,
    pub post_import: bool,
    pub skipped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseFailure {
    pub entity: String,
    pub service: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

#[derive(Debug)]
pub enum DeployError {
    Baseline(BaselineError),
    Working {
        collection: String,
        name: String,
        why: String,
    },
    Server {
        collection: String,
        name: String,
        why: String,
    },
    ParseUnavailable {
        entity: String,
        service: String,
        source: ServerError,
    },
    ParseFailed(Vec<ParseFailure>),
    Conflicts(Vec<EntityPlan>),
    /// `imported` are the projects that imported before this one; the baseline records them.
    Import {
        project: String,
        source: ServerError,
        imported: Vec<String>,
    },
    UnknownPlaceholder {
        project: String,
        key: String,
    },
    /// Every project had imported, and the baseline records them, when a call failed.
    Call {
        project: String,
        target: String,
        service: String,
        why: String,
        imported: Vec<String>,
    },
    NotKept(Box<Report>),
    /// A failure after projects had imported, which the server therefore already holds.
    AfterImport {
        imported: Vec<String>,
        source: Box<DeployError>,
    },
    /// A failure, and then the baseline for what did import could not be written either.
    Unrecorded {
        failure: Box<DeployError>,
        why: BaselineError,
        imported: Vec<String>,
    },
}

impl fmt::Display for DeployError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeployError::Baseline(error) => write!(f, "{error}"),
            DeployError::Working { collection, name, why } => {
                write!(f, "{collection}/{name}: the bundled entity cannot be hashed: {why}")
            }
            DeployError::Server { collection, name, why } => {
                write!(f, "{collection}/{name}: the server copy cannot be checked: {why}")
            }
            DeployError::ParseUnavailable { entity, service, source } => {
                write!(f, "{entity}/{service}: live parse could not run: {source}")
            }
            DeployError::ParseFailed(failures) => {
                write!(f, "{} service script(s) failed live parse", failures.len())
            }
            DeployError::Conflicts(conflicts) => {
                write!(f, "{} entity conflict(s) refuse this deploy", conflicts.len())
            }
            DeployError::Import { project, source, imported } => {
                write!(f, "project {project} import failed: {source}")?;
                if !imported.is_empty() {
                    write!(f, "; already imported and recorded: {}", imported.join(", "))?;
                }
                Ok(())
            }
            DeployError::UnknownPlaceholder { project, key } => write!(
                f,
                "project {project} uses unknown profile key {key:?} in ${{profile:{key}}}"
            ),
            DeployError::Call { project, target, service, why, imported } => {
                write!(f, "project {project} call {target}.{service} failed: {why}")?;
                if !imported.is_empty() {
                    write!(f, "; already imported and recorded: {}", imported.join(", "))?;
                }
                Ok(())
            }
            DeployError::AfterImport { imported, source } => {
                write!(f, "{source}; already imported and recorded: {}", imported.join(", "))
            }
            DeployError::Unrecorded { failure, why, imported } => write!(
                f,
                "{failure}; and the baseline for what had imported ({}) could not be written: {why}",
                if imported.is_empty() { "nothing".to_string() } else { imported.join(", ") }
            ),
            DeployError::NotKept(report) => {
                write!(f, "{} imported entity/entities were not kept as sent", report.not_kept.len())
            }
        }
    }
}

impl std::error::Error for DeployError {}

/// What deploying each entity of the bundles would do: the working copy, the server's copy and the
/// baseline through the push decision table. Reads from the server; changes nothing.
pub fn decide_all(
    remote: &dyn Remote,
    baseline: &Baseline,
    projects: &[ProjectBundle],
) -> Result<Vec<EntityPlan>, DeployError> {
    let entities: Vec<(&ProjectBundle, &Entity)> = projects
        .iter()
        .flat_map(|project| project.entities.iter().map(move |entity| (project, entity)))
        .collect();
    let plans = parallel::map(&entities, |(project, entity)| {
        let working = normalise::hash(&entity.bytes).map_err(|error| DeployError::Working {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            why: error.to_string(),
        })?;
        let server = remote
            .fetch(&entity.collection, &entity.name)
            .map_err(|error| DeployError::Server {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                why: error.to_string(),
            })?
            .map(|bytes| normalise::hash(&bytes))
            .transpose()
            .map_err(|error| DeployError::Server {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                why: error.to_string(),
            })?;
        let decision = push::decide(
            &working,
            server.as_deref(),
            baseline
                .get(&entity.collection, &entity.name)
                .map(|entry| (entry.local.as_str(), entry.server.as_str())),
        );
        Ok(EntityPlan {
            project: project.project.clone(),
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            decision,
        })
    });
    plans.into_iter().collect()
}

/// Plan or apply all project bundles. Projects must already be in dependency order.
pub fn run(
    remote: &dyn Remote,
    baselines: &dyn BaselineStore,
    profile: &Profile,
    projects: &[ProjectBundle],
    apply: bool,
    force: bool,
    only: bool,
) -> Result<Report, DeployError> {
    let mut report = Report {
        projects: projects
            .iter()
            .map(|project| project.project.clone())
            .collect(),
        ..Report::default()
    };

    // Resolve every placeholder before the first server operation (including live parse). The
    // resolved copies are deliberately kept out of Report and every error type, so a plan and
    // diagnostics can show `${profile:key}` but can never print the substituted secret.
    let mut resolved_calls: BTreeMap<String, (Option<ServiceCall>, Vec<ServiceCall>)> =
        BTreeMap::new();
    for project in projects {
        let deploy = project
            .deploy
            .as_ref()
            .map(|call| resolve_call(call, profile, &project.project))
            .transpose()?;
        let post_import = project
            .post_import
            .iter()
            .map(|call| resolve_call(call, profile, &project.project))
            .collect::<Result<Vec<_>, _>>()?;
        resolved_calls.insert(project.project.clone(), (deploy, post_import));
        if let Some(call) = &project.deploy {
            report.calls.push(PlannedCall {
                project: project.project.clone(),
                call: call.clone(),
                post_import: false,
                skipped: false,
            });
        }
        report
            .calls
            .extend(project.post_import.iter().cloned().map(|call| PlannedCall {
                project: project.project.clone(),
                call,
                post_import: true,
                skipped: only,
            }));
    }

    let scripts: Vec<&Script> = projects
        .iter()
        .flat_map(|project| &project.scripts)
        .collect();
    let checks = parallel::map(&scripts, |script| {
        let script = *script;
        remote
            .check_script(&script.source)
            .map(|checked| (script, checked))
            .map_err(|source| DeployError::ParseUnavailable {
                entity: script.entity.clone(),
                service: script.service.clone(),
                source,
            })
    });
    let mut parse_failures = Vec::new();
    for result in checks {
        let (script, checked) = result?;
        report.scripts_checked += 1;
        if !checked.status {
            parse_failures.push(ParseFailure {
                entity: script.entity.clone(),
                service: script.service.clone(),
                line: checked.line_number,
                column: checked.column_number,
                message: checked.message,
            });
        }
    }
    if !parse_failures.is_empty() {
        return Err(DeployError::ParseFailed(parse_failures));
    }

    let mut baseline = baselines.load().map_err(DeployError::Baseline)?;
    report.plans = decide_all(remote, &baseline, projects)?;

    let conflicts: Vec<EntityPlan> = report
        .plans
        .iter()
        .filter(|plan| matches!(plan.decision, Decision::Refuse(_)))
        .cloned()
        .collect();
    if !conflicts.is_empty() && !force {
        return Err(DeployError::Conflicts(conflicts));
    }
    if !apply {
        return Ok(report);
    }

    // A failed import stops the projects after it, but the ones before it are on the server by
    // then. They are still read back and recorded, so a partial deploy does not later look like
    // someone else's change to entities this deploy wrote.
    let mut import_failure = None;
    for project in projects {
        match remote.import(&project.file_name, &project.bytes) {
            Ok(()) => report.imported.push(project.project.clone()),
            Err(source) => {
                import_failure = Some(DeployError::Import {
                    project: project.project.clone(),
                    source,
                    imported: report.imported.clone(),
                });
                break;
            }
        }
    }

    let imported_entities: Vec<&Entity> = projects
        .iter()
        .filter(|project| report.imported.contains(&project.project))
        .flat_map(|project| &project.entities)
        .collect();
    let read_backs = parallel::map(&imported_entities, |entity| {
        let entity = *entity;
        let sent = normalise::hash(&entity.bytes).map_err(|error| DeployError::Working {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            why: error.to_string(),
        })?;
        let fetched = remote.fetch(&entity.collection, &entity.name);
        let (read_back, error) = match fetched {
            Ok(Some(bytes)) => match normalise::hash(&bytes) {
                Ok(hash) => (Some(hash), None),
                Err(why) => (None, Some(why.to_string())),
            },
            Ok(None) => (None, None),
            Err(why) => (None, Some(why.to_string())),
        };
        Ok((entity, sent, read_back, error))
    });
    let mut first_read_back = BTreeMap::<(String, String), String>::new();
    for result in read_backs {
        let (entity, sent, read_back, error) = result?;
        if read_back.as_deref() == Some(sent.as_str()) {
            let read_back = read_back.expect("matching read-back is present");
            baseline.set(
                &entity.collection,
                &entity.name,
                sent.clone(),
                read_back.clone(),
            );
            first_read_back.insert((entity.collection.clone(), entity.name.clone()), read_back);
            report
                .kept
                .push((entity.collection.clone(), entity.name.clone()));
        } else {
            report.not_kept.push(NotKept {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                sent,
                read_back,
                error,
            });
        }
    }

    // Import and read-back failures stop before service calls. What did import is nevertheless
    // persisted below, preserving the partial-import rule with the same single baseline write.
    if let Some(failure) = import_failure {
        // The import's failure is the news; a baseline write failing after it is added to it,
        // never put in its place.
        if let Err(why) = baselines.write(&baseline) {
            return Err(DeployError::Unrecorded {
                failure: Box::new(failure),
                why,
                imported: report.imported.clone(),
            });
        }
        return Err(failure);
    }
    if !report.not_kept.is_empty() {
        baselines.write(&baseline).map_err(DeployError::Baseline)?;
        return Err(DeployError::NotKept(Box::new(report)));
    }

    let mut call_failure = None;
    'projects: for project in projects {
        let (deploy, post_import) = resolved_calls
            .get(&project.project)
            .expect("every project was resolved before server traffic");
        let calls = deploy
            .iter()
            .chain((!only).then_some(post_import).into_iter().flatten());
        for call in calls {
            let outcome = ServiceTarget::parse(&call.target)
                .map_err(ServerError::from)
                .and_then(|target| {
                    remote.call_service(
                        &target,
                        &call.service,
                        &call.parameters,
                        Duration::from_secs(300),
                    )
                });
            if let Err(source) = outcome {
                call_failure = Some(DeployError::Call {
                    project: project.project.clone(),
                    target: call.target.clone(),
                    service: call.service.clone(),
                    why: redact_placeholder_values(&source.to_string(), profile, projects),
                    imported: report.imported.clone(),
                });
                break 'projects;
            }
        }
    }

    if call_failure.is_none() {
        let re_reads = parallel::map(&imported_entities, |entity| {
            let entity = *entity;
            let bytes = remote
                .fetch(&entity.collection, &entity.name)
                .map_err(|why| DeployError::Server {
                    collection: entity.collection.clone(),
                    name: entity.name.clone(),
                    why: why.to_string(),
                })?
                .ok_or_else(|| DeployError::Server {
                    collection: entity.collection.clone(),
                    name: entity.name.clone(),
                    why: "the entity disappeared after its deploy calls".to_string(),
                })?;
            let hash = normalise::hash(&bytes).map_err(|why| DeployError::Server {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                why: why.to_string(),
            })?;
            Ok((entity, hash))
        });
        for result in re_reads {
            match result {
                Ok((entity, hash)) => {
                    let key = (entity.collection.clone(), entity.name.clone());
                    if first_read_back
                        .get(&key)
                        .is_some_and(|before| before != &hash)
                    {
                        // Deploy-service mutations (for example restoring an imported Database
                        // password) are part of this deploy. The working file correctly omits
                        // them; advancing only the server side keeps both sides in sync.
                        baseline
                            .set_server(&entity.collection, &entity.name, hash)
                            .map_err(DeployError::Baseline)?;
                        report.changed_by_deploy.push(key);
                    }
                }
                Err(error) => {
                    // Every project had imported by now; say so with the failure.
                    call_failure = Some(DeployError::AfterImport {
                        imported: report.imported.clone(),
                        source: Box::new(error),
                    });
                    break;
                }
            }
        }
    }

    if let Err(why) = baselines.write(&baseline) {
        return Err(match call_failure {
            Some(failure) => DeployError::Unrecorded {
                failure: Box::new(failure),
                why,
                imported: report.imported.clone(),
            },
            None => DeployError::Baseline(why),
        });
    }
    if let Some(failure) = call_failure {
        Err(failure)
    } else {
        Ok(report)
    }
}

fn resolve_call(
    call: &ServiceCall,
    profile: &Profile,
    project: &str,
) -> Result<ServiceCall, DeployError> {
    Ok(ServiceCall {
        target: call.target.clone(),
        service: call.service.clone(),
        parameters: resolve_value(&call.parameters, profile, project)?,
    })
}

fn resolve_value(value: &Value, profile: &Profile, project: &str) -> Result<Value, DeployError> {
    match value {
        Value::String(text) => {
            let key = text
                .strip_prefix("${profile:")
                .and_then(|rest| rest.strip_suffix('}'));
            if let Some(key) = key {
                let value = profile
                    .value(key)
                    .ok_or_else(|| DeployError::UnknownPlaceholder {
                        project: project.to_string(),
                        key: key.to_string(),
                    })?;
                Ok(serde_json::to_value(value).expect("TOML values serialize as JSON"))
            } else {
                Ok(value.clone())
            }
        }
        Value::Array(values) => values
            .iter()
            .map(|value| resolve_value(value, profile, project))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), resolve_value(value, profile, project)?)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Value::Object),
        _ => Ok(value.clone()),
    }
}

fn redact_placeholder_values(
    message: &str,
    profile: &Profile,
    projects: &[ProjectBundle],
) -> String {
    let mut keys = Vec::new();
    for call in projects
        .iter()
        .flat_map(|project| project.deploy.iter().chain(&project.post_import))
    {
        collect_placeholder_keys(&call.parameters, &mut keys);
    }
    keys.sort();
    keys.dedup();
    keys.into_iter().fold(message.to_string(), |redacted, key| {
        let Some(value) = profile.value(&key) else {
            return redacted;
        };
        let json = serde_json::to_value(value).expect("TOML values serialize as JSON");
        let mut renderings = vec![json.to_string()];
        if let Value::String(text) = &json {
            renderings.push(text.clone());
        }
        renderings.sort_by_key(|value| std::cmp::Reverse(value.len()));
        renderings.dedup();
        renderings.into_iter().fold(redacted, |text, rendered| {
            if rendered.is_empty() {
                text
            } else {
                text.replace(&rendered, &format!("${{profile:{key}}}"))
            }
        })
    })
}

fn collect_placeholder_keys(value: &Value, keys: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if let Some(key) = text
                .strip_prefix("${profile:")
                .and_then(|rest| rest.strip_suffix('}'))
            {
                keys.push(key.to_string());
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_placeholder_keys(value, keys);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_placeholder_keys(value, keys);
            }
        }
        _ => {}
    }
}

/// What to deploy: which projects, which entities, and whether to leave out UI collections.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlanOptions<'a> {
    pub only_projects: &'a [String],
    pub only: &'a [String],
    pub backend_only: bool,
}

/// The per-project bundles a deploy would import, in dependency order, each with the entities it
/// carries, the scripts to parse and the calls to make afterwards. Also a note per project.
/// Shared by the CLI and the MCP server, so both deploy exactly the same thing.
pub fn plan_bundles(
    solution: &super::config::Solution,
    options: PlanOptions,
) -> Result<(Vec<ProjectBundle>, Vec<String>), String> {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    let mut notes = Vec::new();
    let order = match solution.deploy_order() {
        Ok(order) => order,
        Err(error) => {
            return Err(format!("{error}"));
        }
    };
    let selected_projects: BTreeSet<&str> = if options.only_projects.is_empty() {
        order.iter().map(|project| project.name.as_str()).collect()
    } else {
        let mut selected = BTreeSet::new();
        for name in options.only_projects {
            if solution.project(name).is_none() {
                return Err(format!("this solution has no project named {name}"));
            }
            selected.insert(name.as_str());
        }
        selected
    };

    let found = super::workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(found.unreadable.join("; "));
    }
    let backend_only = options.backend_only;
    let selection = if backend_only {
        super::bundle::Selection::backend(solution)
    } else {
        super::bundle::Selection::everything()
    };

    // `projectName` owns attribution. Folder membership is used only for the rare undeclared
    // entity, which workspace already reports as such.
    let mut pool: Vec<super::workspace::EntityFile> = found
        .entities
        .into_iter()
        .filter(|entity| {
            let owner = if entity.info.project.is_empty() {
                entity.found_under.as_str()
            } else {
                entity.info.project.as_str()
            };
            selected_projects.contains(owner) && selection.wants(&entity.info.collection)
        })
        .collect();

    if !options.only.is_empty() {
        let mut narrowed = Vec::new();
        let mut seen = BTreeSet::new();
        for name in options.only {
            let entity = match super::workspace::resolve(&pool, name) {
                Ok(entity) => entity.clone(),
                Err(error) => {
                    return Err(format!("{error}"));
                }
            };
            if seen.insert(entity.path.clone()) {
                narrowed.push(entity);
            }
        }
        pool = narrowed;
    }
    if pool.is_empty() {
        return Err("the deploy selection contains no entities".to_string());
    }

    let source_order = super::bundle::source_files(solution);
    let mut projects = Vec::new();
    for project in order
        .into_iter()
        .filter(|project| selected_projects.contains(project.name.as_str()))
    {
        let mut chosen: Vec<super::workspace::EntityFile> = pool
            .iter()
            .filter(|entity| {
                if entity.info.project.is_empty() {
                    entity.found_under == project.name
                } else {
                    entity.info.project == project.name
                }
            })
            .cloned()
            .collect();
        if chosen.is_empty() {
            continue;
        }
        let wanted_paths: BTreeSet<PathBuf> =
            chosen.iter().map(|entity| entity.path.clone()).collect();
        let files: Vec<PathBuf> = source_order
            .iter()
            .filter(|path| wanted_paths.contains(*path))
            .cloned()
            .collect();
        let built = match super::bundle::build(&files, &selection) {
            Ok(built) => built,
            Err(error) => {
                return Err(format!("project {}: {error}", project.name));
            }
        };
        chosen.sort_by(|left, right| {
            (&left.info.collection, &left.info.name)
                .cmp(&(&right.info.collection, &right.info.name))
        });
        let mut entities = Vec::new();
        let mut scripts = Vec::new();
        for entity in chosen {
            let key = (entity.info.collection.clone(), entity.info.name.clone());
            if built.entities.get(&key) != Some(&1) {
                return Err(format!(
                    "project {} bundle does not contain exactly one {}/{}",
                    project.name, key.0, key.1
                ));
            }
            let bytes = match std::fs::read(&entity.path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Err(format!("{}: {error}", entity.path.display()));
                }
            };
            match super::sidecar::script_services(&bytes) {
                Ok(found_scripts) => {
                    scripts.extend(found_scripts.into_iter().map(|service| Script {
                        entity: entity.info.name.clone(),
                        service: service.name,
                        source: service.script,
                    }));
                }
                Err(error) => {
                    return Err(format!("{}: {error}", entity.path.display()));
                }
            }
            entities.push(Entity {
                collection: entity.info.collection,
                name: entity.info.name,
                bytes,
            });
        }
        notes.push(format!(
            "project {}: planned bundle has {} entities from {} file(s), {} script service(s)",
            project.name,
            built.entities.len(),
            built.files,
            scripts.len()
        ));
        projects.push(ProjectBundle {
            project: project.name.clone(),
            file_name: format!("{}.deploy.xml", project.name),
            bytes: built.bytes,
            entities,
            scripts,
            deploy: project
                .deploy
                .entry_point_thing
                .as_ref()
                .zip(project.deploy.deploy_service.as_ref())
                .map(|(thing, service)| ServiceCall {
                    target: format!("Things/{thing}"),
                    service: service.clone(),
                    parameters: toml_parameters(project.deploy.deploy_parameters.as_ref()),
                }),
            post_import: project
                .deploy
                .post_import
                .iter()
                .map(|call| ServiceCall {
                    target: call
                        .target
                        .clone()
                        .unwrap_or_else(|| format!("Things/{}", call.thing)),
                    service: call.service.clone(),
                    parameters: toml_parameters(call.parameters.as_ref()),
                })
                .collect(),
        });
    }
    if projects.is_empty() {
        return Err("the deploy selection contains no project bundle".to_string());
    }

    Ok((projects, notes))
}

pub fn toml_parameters(table: Option<&toml::Table>) -> serde_json::Value {
    table
        .map(|table| serde_json::to_value(table).expect("TOML tables serialize as JSON"))
        .unwrap_or_else(|| serde_json::json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    fn entity(name: &str, script: &str) -> Vec<u8> {
        format!(
            "<Entities><Things><Thing name=\"{name}\" projectName=\"P\"><ThingShape>\
             <ServiceDefinitions><ServiceDefinition name=\"S\"/></ServiceDefinitions>\
             <ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\">\
             <ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>\
             <code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables>\
             </ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>"
        )
        .into_bytes()
    }

    fn project(name: &str, entities: Vec<Entity>, scripts: Vec<Script>) -> ProjectBundle {
        ProjectBundle {
            project: name.to_string(),
            file_name: format!("{name}.xml"),
            bytes: format!("bundle:{name}").into_bytes(),
            entities,
            scripts,
            deploy: None,
            post_import: Vec::new(),
        }
    }

    fn profile(extra: BTreeMap<String, toml::Value>) -> Profile {
        Profile {
            url: "http://server".into(),
            username: "user".into(),
            password: "password".into(),
            app_key: None,
            extra,
        }
    }

    fn run_test(
        remote: &dyn Remote,
        baselines: &dyn BaselineStore,
        projects: &[ProjectBundle],
        apply: bool,
        force: bool,
    ) -> Result<Report, DeployError> {
        run(
            remote,
            baselines,
            &profile(BTreeMap::new()),
            projects,
            apply,
            force,
            false,
        )
    }

    fn target(name: &str, script: &str) -> Entity {
        Entity {
            collection: "Things".to_string(),
            name: name.to_string(),
            bytes: entity(name, script),
        }
    }

    #[derive(Default)]
    struct MemoryBaseline {
        value: RefCell<Baseline>,
        writes: RefCell<usize>,
        fail_write: bool,
    }

    impl BaselineStore for MemoryBaseline {
        fn load(&self) -> Result<Baseline, BaselineError> {
            Ok(self.value.borrow().clone())
        }

        fn write(&self, baseline: &Baseline) -> Result<(), BaselineError> {
            *self.writes.borrow_mut() += 1;
            if self.fail_write {
                return Err(BaselineError::Io {
                    path: "baseline.json".into(),
                    why: "disk full".into(),
                });
            }
            *self.value.borrow_mut() = baseline.clone();
            Ok(())
        }
    }

    #[derive(Default)]
    struct Fake {
        held: Mutex<BTreeMap<(String, String), Vec<u8>>>,
        imports: Mutex<Vec<String>>,
        events: Mutex<Vec<String>>,
        calls: Mutex<Vec<(String, String, Value, Duration)>>,
        import_values: BTreeMap<String, Vec<Entity>>,
        call_values: BTreeMap<String, Vec<Entity>>,
        fail_import: Option<String>,
        fail_service: Option<String>,
        fetch_failures: BTreeSet<(String, String)>,
        mismatch: BTreeSet<(String, String)>,
        parse_unreachable: bool,
        checks: Mutex<usize>,
    }

    impl push::Remote for Fake {
        fn fetch(&self, collection: &str, name: &str) -> Result<Option<Vec<u8>>, ServerError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("fetch:{collection}/{name}"));
            let key = (collection.to_string(), name.to_string());
            if self.fetch_failures.contains(&key) {
                return Err(ServerError::Transport {
                    method: super::super::server::Method::Get,
                    url: format!("http://server/{collection}/{name}"),
                    why: "offline".to_string(),
                });
            }
            Ok(self.held.lock().unwrap().get(&key).cloned())
        }

        fn import(&self, file_name: &str, _: &[u8]) -> Result<(), ServerError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("import:{file_name}"));
            self.imports.lock().unwrap().push(file_name.to_string());
            if self.fail_import.as_deref() == Some(file_name) {
                return Err(ServerError::Rejected {
                    url: "http://server/Importer".to_string(),
                    body: "failed".to_string(),
                });
            }
            for entity in self.import_values.get(file_name).into_iter().flatten() {
                let key = (entity.collection.clone(), entity.name.clone());
                let bytes = if self.mismatch.contains(&key) {
                    super::tests::entity(&entity.name, "server_changed();")
                } else {
                    entity.bytes.clone()
                };
                self.held.lock().unwrap().insert(key, bytes);
            }
            Ok(())
        }
    }

    impl Remote for Fake {
        fn check_script(&self, script: &str) -> Result<ScriptCheck, ServerError> {
            *self.checks.lock().unwrap() += 1;
            if self.parse_unreachable {
                return Err(ServerError::Transport {
                    method: super::super::server::Method::Post,
                    url: "http://server/check".to_string(),
                    why: "offline".to_string(),
                });
            }
            Ok(ScriptCheck {
                status: !script.contains("BAD"),
                line_number: 3,
                column_number: 4,
                message: "syntax error".to_string(),
            })
        }

        fn call_service(
            &self,
            target: &ServiceTarget,
            service: &str,
            parameters: &Value,
            timeout: Duration,
        ) -> Result<Option<Value>, ServerError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("call:{target}.{service}"));
            self.calls.lock().unwrap().push((
                target.to_string(),
                service.to_string(),
                parameters.clone(),
                timeout,
            ));
            if self.fail_service.as_deref() == Some(service) {
                return Err(ServerError::Rejected {
                    url: format!("http://server/{target}/Services/{service}"),
                    body: format!("service said no for {parameters}"),
                });
            }
            for entity in self.call_values.get(service).into_iter().flatten() {
                self.held.lock().unwrap().insert(
                    (entity.collection.clone(), entity.name.clone()),
                    entity.bytes.clone(),
                );
            }
            Ok(None)
        }
    }

    fn script(entity: &str, source: &str) -> Script {
        Script {
            entity: entity.to_string(),
            service: "S".to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn plan_mode_sends_nothing_and_writes_nothing() {
        let target = target("A", "ok();");
        let projects = [project("A", vec![target], vec![script("A", "ok();")])];
        let remote = Fake::default();
        let baselines = MemoryBaseline::default();
        let report = run_test(&remote, &baselines, &projects, false, false).unwrap();
        assert!(report.imported.is_empty());
        assert!(remote.imports.lock().unwrap().is_empty());
        assert_eq!(*baselines.writes.borrow(), 0);
    }

    #[test]
    fn parse_failure_aborts_before_an_import() {
        let projects = [project(
            "A",
            vec![target("A", "BAD")],
            vec![script("A", "BAD")],
        )];
        let remote = Fake::default();
        let error =
            run_test(&remote, &MemoryBaseline::default(), &projects, true, false).unwrap_err();
        assert!(matches!(error, DeployError::ParseFailed(_)));
        assert!(remote.imports.lock().unwrap().is_empty());
    }

    #[test]
    fn parse_failures_are_reported_in_input_order() {
        let projects = [project(
            "P",
            vec![],
            vec![
                script("First", "BAD one"),
                script("Good", "ok();"),
                script("Last", "BAD two"),
            ],
        )];
        let error = run_test(
            &Fake::default(),
            &MemoryBaseline::default(),
            &projects,
            false,
            false,
        )
        .unwrap_err();
        let DeployError::ParseFailed(failures) = error else {
            panic!("{error}")
        };
        assert_eq!(
            failures
                .iter()
                .map(|failure| failure.entity.as_str())
                .collect::<Vec<_>>(),
            ["First", "Last"]
        );
    }

    #[test]
    fn an_unreachable_parser_aborts_fail_closed() {
        let projects = [project(
            "A",
            vec![target("A", "ok();")],
            vec![script("A", "ok();")],
        )];
        let remote = Fake {
            parse_unreachable: true,
            ..Fake::default()
        };
        let error =
            run_test(&remote, &MemoryBaseline::default(), &projects, true, false).unwrap_err();
        assert!(matches!(error, DeployError::ParseUnavailable { .. }));
        assert!(remote.imports.lock().unwrap().is_empty());
    }

    #[test]
    fn a_conflict_aborts_without_force() {
        let working = target("A", "mine();");
        let ancestor = entity("A", "ancestor();");
        let server = entity("A", "theirs();");
        let baselines = MemoryBaseline::default();
        let ancestor = normalise::hash(&ancestor).unwrap();
        baselines
            .value
            .borrow_mut()
            .set("Things", "A", ancestor.clone(), ancestor);
        let remote = Fake::default();
        remote
            .held
            .lock()
            .unwrap()
            .insert(("Things".into(), "A".into()), server);
        let projects = [project("A", vec![working], vec![])];
        let error = run_test(&remote, &baselines, &projects, true, false).unwrap_err();
        assert!(matches!(error, DeployError::Conflicts(_)));
        assert!(remote.imports.lock().unwrap().is_empty());
    }

    #[test]
    fn the_first_fetch_error_in_input_order_is_returned() {
        let projects = [project(
            "P",
            vec![target("First", "a();"), target("Last", "b();")],
            vec![],
        )];
        let remote = Fake {
            fetch_failures: BTreeSet::from([
                ("Things".to_string(), "First".to_string()),
                ("Things".to_string(), "Last".to_string()),
            ]),
            ..Fake::default()
        };
        let error =
            run_test(&remote, &MemoryBaseline::default(), &projects, false, false).unwrap_err();
        assert!(matches!(
            error,
            DeployError::Server { collection, name, .. }
                if collection == "Things" && name == "First"
        ));
    }

    #[test]
    fn projects_import_in_order_and_first_failure_stops_the_second() {
        let a = target("A", "a();");
        let b = target("B", "b();");
        let projects = [project("A", vec![a], vec![]), project("B", vec![b], vec![])];
        let remote = Fake {
            fail_import: Some("A.xml".to_string()),
            ..Fake::default()
        };
        let error =
            run_test(&remote, &MemoryBaseline::default(), &projects, true, false).unwrap_err();
        assert!(matches!(error, DeployError::Import { project, .. } if project == "A"));
        assert_eq!(remote.imports.lock().unwrap().as_slice(), ["A.xml"]);
    }

    #[test]
    fn a_later_failure_still_records_the_projects_that_did_import() {
        let a = target("A", "a();");
        let b = target("B", "b();");
        let projects = [
            project("A", vec![a.clone()], vec![]),
            project("B", vec![b], vec![]),
        ];
        let remote = Fake {
            import_values: BTreeMap::from([("A.xml".to_string(), vec![a])]),
            fail_import: Some("B.xml".to_string()),
            ..Fake::default()
        };
        let baselines = MemoryBaseline::default();
        let error = run_test(&remote, &baselines, &projects, true, false).unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("; already imported and recorded: A"),
            "{error}"
        );
        assert!(matches!(error, DeployError::Import { project, .. } if project == "B"));
        assert_eq!(*baselines.writes.borrow(), 1);
        let baseline = baselines.value.borrow();
        let a = baseline
            .get("Things", "A")
            .expect("A reached the server and was read back");
        assert_eq!(a.local, a.server);
        assert!(baseline.get("Things", "B").is_none(), "B never imported");
    }

    #[test]
    fn a_baseline_that_cannot_be_written_adds_to_the_failure_rather_than_hiding_it() {
        let a = target("A", "a();");
        let b = target("B", "b();");
        let projects = [
            project("A", vec![a.clone()], vec![]),
            project("B", vec![b], vec![]),
        ];
        let remote = Fake {
            import_values: BTreeMap::from([("A.xml".to_string(), vec![a])]),
            fail_import: Some("B.xml".to_string()),
            ..Fake::default()
        };
        let baselines = MemoryBaseline {
            fail_write: true,
            ..MemoryBaseline::default()
        };
        let error = run_test(&remote, &baselines, &projects, true, false).unwrap_err();
        let text = error.to_string();
        assert!(
            text.starts_with("project B import failed"),
            "the import failure leads: {text}"
        );
        assert!(
            text.contains("imported (A) could not be written: "),
            "{text}"
        );
        assert!(text.contains("disk full"), "{text}");
    }

    #[test]
    fn baseline_is_written_once_and_contains_only_matching_read_backs() {
        let same = target("Same", "same();");
        let changed = target("Changed", "changed();");
        let project = project("P", vec![same.clone(), changed.clone()], vec![]);
        let remote = Fake {
            import_values: BTreeMap::from([("P.xml".to_string(), vec![same, changed])]),
            mismatch: BTreeSet::from([("Things".to_string(), "Changed".to_string())]),
            ..Fake::default()
        };
        let baselines = MemoryBaseline::default();
        let error = run_test(&remote, &baselines, &[project], true, false).unwrap_err();
        assert!(matches!(error, DeployError::NotKept(_)));
        assert_eq!(*baselines.writes.borrow(), 1);
        let baseline = baselines.value.borrow();
        assert!(baseline.get("Things", "Same").is_some());
        assert!(baseline.get("Things", "Changed").is_none());
    }

    #[test]
    fn two_projects_import_in_the_given_dependency_order() {
        let a = target("A", "a();");
        let b = target("B", "b();");
        let projects = [
            project("A", vec![a.clone()], vec![]),
            project("B", vec![b.clone()], vec![]),
        ];
        let remote = Fake {
            import_values: BTreeMap::from([
                ("A.xml".to_string(), vec![a]),
                ("B.xml".to_string(), vec![b]),
            ]),
            ..Fake::default()
        };
        run_test(&remote, &MemoryBaseline::default(), &projects, true, false).unwrap();
        assert_eq!(
            remote.imports.lock().unwrap().as_slice(),
            ["A.xml", "B.xml"]
        );
    }

    fn call(target: &str, service: &str, parameters: Value) -> ServiceCall {
        ServiceCall {
            target: target.into(),
            service: service.into(),
            parameters,
        }
    }

    #[test]
    fn calls_follow_all_import_read_backs_in_project_order_then_entities_are_re_read() {
        let a = target("A", "a();");
        let b = target("B", "b();");
        let mut pa = project("A", vec![a.clone()], vec![]);
        pa.deploy = Some(call("Things/A.Entry", "DeployA", serde_json::json!({})));
        pa.post_import = vec![call("Things/A.Seed", "SeedA", serde_json::json!({}))];
        let mut pb = project("B", vec![b.clone()], vec![]);
        pb.deploy = Some(call("Things/B.Entry", "DeployB", serde_json::json!({})));
        pb.post_import = vec![call("Things/B.Seed", "SeedB", serde_json::json!({}))];
        let remote = Fake {
            import_values: BTreeMap::from([("A.xml".into(), vec![a]), ("B.xml".into(), vec![b])]),
            ..Fake::default()
        };

        run_test(&remote, &MemoryBaseline::default(), &[pa, pb], true, false).unwrap();
        let events = remote.events.lock().unwrap();
        let start = events
            .iter()
            .position(|event| event == "import:A.xml")
            .unwrap();
        let deploy = &events[start..];
        assert_eq!(&deploy[..2], ["import:A.xml", "import:B.xml"]);
        let calls: Vec<&str> = deploy
            .iter()
            .filter(|event| event.starts_with("call:"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            calls,
            [
                "call:Things/A.Entry.DeployA",
                "call:Things/A.Seed.SeedA",
                "call:Things/B.Entry.DeployB",
                "call:Things/B.Seed.SeedB",
            ]
        );
        let first_call = deploy
            .iter()
            .position(|event| event.starts_with("call:"))
            .unwrap();
        let reads_before = deploy[2..first_call]
            .iter()
            .filter(|event| event.starts_with("fetch:"))
            .count();
        let reads_after = deploy[first_call + 4..]
            .iter()
            .filter(|event| event.starts_with("fetch:"))
            .count();
        assert_eq!((reads_before, reads_after), (2, 2));
    }

    #[test]
    fn only_runs_the_deploy_service_and_marks_post_import_skipped() {
        let a = target("A", "a();");
        let mut project = project("A", vec![a.clone()], vec![]);
        project.deploy = Some(call("Things/A", "Deploy", serde_json::json!({})));
        project.post_import = vec![call("Things/A", "Seed", serde_json::json!({}))];
        let remote = Fake {
            import_values: BTreeMap::from([("A.xml".into(), vec![a])]),
            ..Fake::default()
        };
        let report = run(
            &remote,
            &MemoryBaseline::default(),
            &profile(BTreeMap::new()),
            &[project],
            true,
            false,
            true,
        )
        .unwrap();
        assert_eq!(remote.calls.lock().unwrap()[0].1, "Deploy");
        assert_eq!(remote.calls.lock().unwrap()[0].3, Duration::from_secs(300));
        assert_eq!(remote.calls.lock().unwrap().len(), 1);
        assert!(report
            .calls
            .iter()
            .any(|planned| planned.call.service == "Seed" && planned.skipped));
    }

    #[test]
    fn a_failing_deploy_call_stops_later_calls_but_records_import_read_back_once() {
        let a = target("A", "a();");
        let mut project = project("A", vec![a.clone()], vec![]);
        project.deploy = Some(call("Things/A", "Deploy", serde_json::json!({})));
        project.post_import = vec![call("Things/A", "Never", serde_json::json!({}))];
        let remote = Fake {
            import_values: BTreeMap::from([("A.xml".into(), vec![a])]),
            fail_service: Some("Deploy".into()),
            ..Fake::default()
        };
        let baselines = MemoryBaseline::default();
        let error = run_test(&remote, &baselines, &[project], true, false).unwrap_err();
        assert!(error.to_string().contains("Things/A.Deploy"));
        assert!(error.to_string().contains("service said no"));
        assert_eq!(remote.calls.lock().unwrap().len(), 1);
        assert_eq!(*baselines.writes.borrow(), 1);
        assert!(baselines.value.borrow().get("Things", "A").is_some());
    }

    #[test]
    fn placeholders_are_substituted_only_in_the_request_and_unknown_keys_fail_up_front() {
        let secret = "a-value-that-must-not-be-rendered";
        let a = target("A", "a();");
        let mut project = project("SecretProject", vec![a.clone()], vec![]);
        project.deploy = Some(call(
            "Things/A",
            "Deploy",
            serde_json::json!({"deploymentConfig": {"password": "${profile:database_password}"}}),
        ));
        let remote = Fake {
            import_values: BTreeMap::from([("SecretProject.xml".into(), vec![a])]),
            ..Fake::default()
        };
        let active = profile(BTreeMap::from([(
            "database_password".into(),
            toml::Value::String(secret.into()),
        )]));
        let report = run(
            &remote,
            &MemoryBaseline::default(),
            &active,
            &[project.clone()],
            true,
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            remote.calls.lock().unwrap()[0].2["deploymentConfig"]["password"],
            secret
        );
        let rendered = format!("{report:?}");
        assert!(rendered.contains("${profile:database_password}"));
        assert!(!rendered.contains(secret));
        let plan_text = report
            .calls
            .iter()
            .map(|planned| planned.call.to_string())
            .collect::<String>();
        assert!(plan_text.contains("${profile:database_password}"));
        assert!(!plan_text.contains(secret));
        assert!(!format!("{active:?}").contains(secret));

        let rejected = Fake {
            import_values: BTreeMap::from([(
                "SecretProject.xml".into(),
                vec![target("A", "a();")],
            )]),
            fail_service: Some("Deploy".into()),
            ..Fake::default()
        };
        let error = run(
            &rejected,
            &MemoryBaseline::default(),
            &active,
            &[project.clone()],
            true,
            false,
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("${profile:database_password}"));
        assert!(!error.to_string().contains(secret));

        project.deploy.as_mut().unwrap().parameters =
            serde_json::json!({"password": "${profile:missing_key}"});
        let unseen = Fake::default();
        let error = run(
            &unseen,
            &MemoryBaseline::default(),
            &active,
            &[project],
            true,
            false,
            false,
        )
        .unwrap_err();
        let displayed = error.to_string();
        assert!(displayed.contains("missing_key"));
        assert!(displayed.contains("SecretProject"));
        assert!(!displayed.contains(secret));
        assert!(unseen.imports.lock().unwrap().is_empty());
        assert_eq!(*unseen.checks.lock().unwrap(), 0);
        assert!(unseen.events.lock().unwrap().is_empty());
    }

    #[test]
    fn re_read_advances_only_changed_server_sides_and_writes_once() {
        let changed = target("Changed", "before();");
        let same = target("Same", "same();");
        let after = target("Changed", "after();");
        let mut project = project("P", vec![changed.clone(), same.clone()], vec![]);
        project.deploy = Some(call("Things/Entry", "Deploy", serde_json::json!({})));
        let remote = Fake {
            import_values: BTreeMap::from([("P.xml".into(), vec![changed.clone(), same.clone()])]),
            call_values: BTreeMap::from([("Deploy".into(), vec![after.clone()])]),
            ..Fake::default()
        };
        let baselines = MemoryBaseline::default();
        let report = run_test(&remote, &baselines, &[project], true, false).unwrap();
        assert_eq!(
            report.changed_by_deploy,
            [("Things".into(), "Changed".into())]
        );
        assert_eq!(*baselines.writes.borrow(), 1);
        let baseline = baselines.value.borrow();
        let changed_entry = baseline.get("Things", "Changed").unwrap();
        assert_eq!(
            changed_entry.local,
            normalise::hash(&changed.bytes).unwrap()
        );
        assert_eq!(changed_entry.server, normalise::hash(&after.bytes).unwrap());
        let same_entry = baseline.get("Things", "Same").unwrap();
        assert_eq!(same_entry.local, same_entry.server);
    }
}
