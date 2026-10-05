//! TypeScript declarations for the entities in a solution.
//!
//! Repository declarations can be completed by the deliberately small, offline platform cache.

use super::config::Solution;
use super::datashape::{self, Aspect};
use super::entity_key::ServiceTarget;
use super::scan::{self, ScanError, Token};
use super::server::{Client, ServerError};
use super::sidecar;
use super::workspace;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BASE: &str = include_str!("types_base.d.ts");
const ENTITY_COLLECTIONS: &[&str] = &["Things", "ThingTemplates", "ThingShapes"];
const PLAIN_COLLECTIONS: &[&str] = &[
    "Mashups",
    "Users",
    "Groups",
    "Projects",
    "Networks",
    "Organizations",
    "Subsystems",
    "MediaEntities",
    "StyleDefinitions",
    "StateDefinitions",
    "LocalizationTables",
    "Dashboards",
    "Logs",
    "ModelTags",
    "Notifications",
    "Authenticators",
    "Applications",
];

#[derive(Debug)]
pub enum TypesError {
    Workspace(workspace::WorkspaceError),
    Remote(ServerError),
    Platform(String),
}

impl fmt::Display for TypesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypesError::Workspace(error) => write!(f, "{error}"),
            TypesError::Remote(error) => write!(f, "{error}"),
            TypesError::Platform(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for TypesError {}

impl From<workspace::WorkspaceError> for TypesError {
    fn from(value: workspace::WorkspaceError) -> Self {
        Self::Workspace(value)
    }
}

impl From<ServerError> for TypesError {
    fn from(value: ServerError) -> Self {
        Self::Remote(value)
    }
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub entities: usize,
    pub data_shapes: usize,
    pub services: usize,
    pub files_written: usize,
    pub skipped: Vec<String>,
    pub gitignore_covers_types: bool,
}

#[derive(Debug, Default)]
pub struct Refresh {
    pub files_written: Option<usize>,
    pub warning: Option<String>,
}

/// Keep an opted-in solution's declarations current after another command wrote source data.
/// Failure is advisory: the command's own writes have already succeeded and remain successful.
pub fn refresh_after_write(solution: &Solution, wrote: bool) -> Refresh {
    if !wrote || !solution.root.join(".twaco/types").exists() {
        return Refresh::default();
    }
    match write(solution) {
        Ok(outcome) => Refresh {
            files_written: Some(outcome.files_written),
            warning: None,
        },
        Err(error) => Refresh {
            files_written: None,
            warning: Some(error.to_string()),
        },
    }
}

#[derive(Debug)]
pub struct PlatformOutcome {
    pub templates: usize,
    pub shapes: usize,
    pub resources: usize,
    pub skipped: Vec<String>,
    pub types: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeFinding {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
pub struct CheckOutcome {
    pub declarations: Outcome,
    pub findings: Vec<TypeFinding>,
    pub affected_services: usize,
    pub services: usize,
    pub elapsed: Duration,
}

/// One finding in the JSON Lines protocol consumed by a declared `[[check]]` hook.
pub fn finding_json(finding: &TypeFinding) -> String {
    json!({
        "schema": 1,
        "gate": "types",
        "file": finding.file,
        "line": finding.line,
        "rule": format!("TS{}", finding.code),
        "message": format!("{} (column {})", finding.message, finding.column),
    })
    .to_string()
}

pub fn check_summary(outcome: &CheckOutcome) -> String {
    format!(
        "types: {} finding(s) in {} of {} services (tsc in {:.1} s)",
        outcome.findings.len(),
        outcome.affected_services,
        outcome.services,
        outcome.elapsed.as_secs_f64()
    )
}

#[derive(Debug)]
pub struct CheckError(String);

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CheckError {}

impl From<TypesError> for CheckError {
    fn from(value: TypesError) -> Self {
        Self(value.to_string())
    }
}

/// Captured compiler output, kept independent of `std::process::Output` for no-node tests.
pub struct CompilerOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Injectable boundary around TypeScript. Production uses a child process; tests use canned text.
pub trait CompilerRunner {
    fn run(
        &self,
        program: &OsStr,
        arguments: &[OsString],
        current_dir: &Path,
    ) -> std::io::Result<CompilerOutput>;
}

struct ProcessCompiler;

impl CompilerRunner for ProcessCompiler {
    fn run(
        &self,
        program: &OsStr,
        arguments: &[OsString],
        current_dir: &Path,
    ) -> std::io::Result<CompilerOutput> {
        let output = Command::new(program)
            .args(arguments)
            .current_dir(current_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        Ok(CompilerOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

const PLATFORM_TIMEOUT: Duration = Duration::from_secs(120);

/// The read-only calls needed to build the platform cache. Kept small for fake-server tests.
pub trait Remote {
    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError>;
}

impl Remote for Client {
    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError> {
        self.call_service(target, service, parameters, PLATFORM_TIMEOUT)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Platform {
    #[serde(default)]
    templates: BTreeMap<String, PlatformMeta>,
    #[serde(default)]
    shapes: BTreeMap<String, PlatformMeta>,
    #[serde(default)]
    resources: BTreeMap<String, PlatformMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PlatformMeta {
    #[serde(default)]
    services: BTreeMap<String, PlatformService>,
    #[serde(default)]
    properties: BTreeMap<String, PlatformProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlatformService {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default)]
    inputs: BTreeMap<String, PlatformInput>,
    result: PlatformValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlatformInput {
    #[serde(rename = "baseType")]
    base_type: String,
    #[serde(rename = "dataShape", default, skip_serializing_if = "Option::is_none")]
    data_shape: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlatformValue {
    #[serde(rename = "baseType")]
    base_type: String,
    #[serde(rename = "dataShape", default, skip_serializing_if = "Option::is_none")]
    data_shape: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlatformProperty {
    #[serde(rename = "baseType")]
    base_type: String,
    #[serde(rename = "dataShape", default, skip_serializing_if = "Option::is_none")]
    data_shape: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypedValue {
    pub(crate) base_type: String,
    pub(crate) data_shape: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parameter {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) value: TypedValue,
    pub(crate) required: bool,
    pub(crate) default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Service {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: Vec<Parameter>,
    pub(crate) result: TypedValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Property {
    name: String,
    description: String,
    value: TypedValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Member {
    Service(Service),
    Property(Property),
}

impl Member {
    fn name(&self) -> &str {
        match self {
            Member::Service(value) => &value.name,
            Member::Property(value) => &value.name,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Entity {
    pub(crate) name: String,
    pub(crate) collection: String,
    pub(crate) project: String,
    pub(crate) template: Option<String>,
    pub(crate) shapes: Vec<String>,
    pub(crate) members: Vec<Member>,
    pub(crate) script_services: BTreeSet<String>,
}

impl Entity {
    /// `("service" | "property", name)` for each member this entity declares itself.
    pub(crate) fn member_list(&self) -> Vec<(&'static str, &str)> {
        self.members
            .iter()
            .map(|member| match member {
                Member::Service(service) => ("service", service.name.as_str()),
                Member::Property(property) => ("property", property.name.as_str()),
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct DataShape {
    name: String,
    fields: Vec<datashape::Field>,
}

#[derive(Debug, Default)]
pub(crate) struct Model {
    pub(crate) entities: Vec<Entity>,
    data_shapes: Vec<DataShape>,
}

#[derive(Debug)]
struct Generated {
    datashapes: String,
    entities: String,
    collections: String,
}

/// Generate and atomically update the four shared declaration files.
pub fn write(solution: &Solution) -> Result<Outcome, TypesError> {
    let (model, mut skipped) = load_model(solution);
    let platform_path = solution.root.join(".twaco/platform.json");
    let platform = match std::fs::read(&platform_path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(platform) => Some(platform),
            Err(error) => {
                skipped.push(format!(
                    "{} is malformed and was ignored: {error}",
                    platform_path.display()
                ));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            skipped.push(format!(
                "{} could not be read and was ignored: {error}",
                platform_path.display()
            ));
            None
        }
    };
    write_model(solution, &model, platform.as_ref(), skipped)
}

/// Regenerate editor declarations and type-check every service in one compiler project.
pub fn check(solution: &Solution) -> Result<CheckOutcome, CheckError> {
    check_with(solution, &ProcessCompiler)
}

pub(crate) fn check_with(
    solution: &Solution,
    compiler: &dyn CompilerRunner,
) -> Result<CheckOutcome, CheckError> {
    // This is deliberately the public generation path: checking has exactly the same cache
    // handling and declaration writes as plain `twaco types`.
    let declarations = write(solution)?;
    let (model, _) = load_model(solution);
    let projects = write_check_project(solution, &model)?;
    let command = compiler_command(solution);
    let (program, configured_arguments) = command
        .split_first()
        .expect("compiler discovery always returns a program");
    let config_path = solution.root.join(".twaco/types/check/tsconfig.json");
    let mut arguments = configured_arguments.to_vec();
    arguments.extend([
        OsString::from("-p"),
        config_path.into_os_string(),
        OsString::from("--pretty"),
        OsString::from("false"),
    ]);
    let started = Instant::now();
    let output = compiler
        .run(program, &arguments, &solution.root)
        .map_err(|error| {
            CheckError(format!(
                "cannot start TypeScript compiler {}: {error}; install one with `npm install \
             --save-dev typescript` in the solution root, or set `[types] tsc` in twaco.toml",
                program.to_string_lossy()
            ))
        })?;
    let elapsed = started.elapsed();
    let stdout = std::str::from_utf8(&output.stdout).map_err(|error| {
        CheckError(format!(
            "could not read TypeScript compiler output: {error}"
        ))
    })?;
    let stderr = std::str::from_utf8(&output.stderr).map_err(|error| {
        CheckError(format!(
            "could not read TypeScript compiler output: {error}"
        ))
    })?;
    let combined = if stdout.is_empty() {
        stderr.to_string()
    } else if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    };
    let parsed = parse_compiler_output(&combined);
    if !output.success && parsed.is_empty() {
        let argv = std::iter::once(program.as_os_str())
            .chain(arguments.iter().map(OsString::as_os_str))
            .map(|argument| format!("{:?}", argument.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ");
        let detail = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
            .unwrap_or("(stderr was blank)");
        return Err(CheckError(format!(
            "TypeScript compiler exited non-zero without a parsable finding\n\
             ran: {argv}; stderr: {detail}; install one with `npm install --save-dev typescript` \
             in the solution root, or set `[types] tsc` in twaco.toml"
        )));
    }

    let mut affected = BTreeSet::new();
    let mut findings = Vec::new();
    for finding in parsed {
        let (file, line, service) = map_finding(solution, &projects, &finding);
        if let Some(service) = service {
            affected.insert(service);
        }
        findings.push(TypeFinding {
            file,
            line,
            column: finding.column,
            code: finding.code,
            message: finding.message,
        });
    }
    findings.sort_by(|a, b| {
        (&a.file, a.line, a.column, &a.code, &a.message)
            .cmp(&(&b.file, b.line, b.column, &b.code, &b.message))
    });
    Ok(CheckOutcome {
        declarations,
        findings,
        affected_services: affected.len(),
        services: projects.len(),
        elapsed,
    })
}

pub(crate) fn load_model(solution: &Solution) -> (Model, Vec<String>) {
    let discovered = workspace::discover(solution);
    let mut model = Model::default();
    let mut skipped = discovered.unreadable;
    for file in discovered.entities {
        if file.info.collection != "DataShapes"
            && !ENTITY_COLLECTIONS.contains(&file.info.collection.as_str())
        {
            continue;
        }
        let bytes = match std::fs::read(&file.path) {
            Ok(bytes) => bytes,
            Err(error) => {
                skipped.push(format!("{}: {error}", file.path.display()));
                continue;
            }
        };
        match parse_document(&bytes, &file.info.collection, &file.info.name) {
            Ok(Parsed::Entity(mut entity)) => {
                entity.project = file.found_under;
                model.entities.push(entity);
            }
            Ok(Parsed::DataShape(shape)) => model.data_shapes.push(shape),
            Err(error) => skipped.push(format!("{}: {error}", file.path.display())),
        }
    }
    model
        .entities
        .sort_by(|a, b| (&a.name, &a.collection).cmp(&(&b.name, &b.collection)));
    model.data_shapes.sort_by(|a, b| a.name.cmp(&b.name));
    (model, skipped)
}

fn write_model(
    solution: &Solution,
    model: &Model,
    platform: Option<&Platform>,
    mut skipped: Vec<String>,
) -> Result<Outcome, TypesError> {
    let generated = generate(model, platform);
    let directory = solution.root.join(".twaco").join("types");
    let files = [
        ("twx.d.ts", ensure_lf_end(BASE)),
        ("datashapes.d.ts", generated.datashapes),
        ("entities.d.ts", generated.entities),
        ("collections.d.ts", generated.collections),
    ];
    let mut files_written = 0;
    for (name, content) in files {
        files_written += usize::from(workspace::write_lf_if_changed(
            &directory.join(name),
            &content,
        )?);
    }

    let (services, service_files_written) = write_service_projects(solution, model, &mut skipped)?;
    files_written += service_files_written;
    skipped.sort();

    Ok(Outcome {
        entities: model.entities.len(),
        data_shapes: model.data_shapes.len(),
        services,
        files_written,
        skipped,
        gitignore_covers_types: gitignore_covers_types(&solution.root),
    })
}

/// Fetch a complete new cache before replacing the old one, then regenerate declarations from it.
pub fn fetch_platform(
    remote: &dyn Remote,
    solution: &Solution,
) -> Result<PlatformOutcome, TypesError> {
    let (model, model_skipped) = load_model(solution);
    let local_templates: BTreeSet<&str> = model
        .entities
        .iter()
        .filter(|entity| entity.collection == "ThingTemplates")
        .map(|entity| entity.name.as_str())
        .collect();
    let local_shapes: BTreeSet<&str> = model
        .entities
        .iter()
        .filter(|entity| entity.collection == "ThingShapes")
        .map(|entity| entity.name.as_str())
        .collect();
    let mut templates = BTreeSet::from(["GenericThing".to_string()]);
    let mut shapes = BTreeSet::new();
    for entity in &model.entities {
        if let Some(template) = &entity.template {
            if !local_templates.contains(template.as_str()) {
                templates.insert(template.clone());
            }
        }
        for shape in &entity.shapes {
            if !local_shapes.contains(shape.as_str()) {
                shapes.insert(shape.clone());
            }
        }
    }

    let mut platform = Platform::default();
    let mut skipped = Vec::new();
    for name in templates {
        fetch_one(
            remote,
            "ThingTemplates",
            &name,
            "GetInstanceMetadataAsJSON",
            &mut platform.templates,
            &mut skipped,
        )?;
    }
    for name in shapes {
        fetch_one(
            remote,
            "ThingShapes",
            &name,
            "GetInstanceMetadataAsJSON",
            &mut platform.shapes,
            &mut skipped,
        )?;
    }

    let listing = required_reply(
        remote.call(
            &ServiceTarget::platform("Resources", "EntityServices"),
            "GetEntityList",
            &json!({ "type": "Resource", "maxItems": 1000 }),
        )?,
        "Resources/EntityServices.GetEntityList",
    )?;
    let resource_names = listing
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            TypesError::Platform(
                "Resources/EntityServices.GetEntityList returned no rows array".to_string(),
            )
        })?
        .iter()
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    TypesError::Platform(
                        "Resources/EntityServices.GetEntityList returned a row without a name"
                            .to_string(),
                    )
                })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    for name in resource_names {
        fetch_one(
            remote,
            "Resources",
            &name,
            "GetMetadataAsJSON",
            &mut platform.resources,
            &mut skipped,
        )?;
    }

    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&platform).expect("platform cache structs are JSON encodable")
    );
    workspace::write_lf_if_changed(&solution.root.join(".twaco/platform.json"), &text)?;
    let types = write_model(solution, &model, Some(&platform), model_skipped)?;
    Ok(PlatformOutcome {
        templates: platform.templates.len(),
        shapes: platform.shapes.len(),
        resources: platform.resources.len(),
        skipped,
        types,
    })
}

fn fetch_one(
    remote: &dyn Remote,
    collection: &str,
    name: &str,
    service: &str,
    destination: &mut BTreeMap<String, PlatformMeta>,
    skipped: &mut Vec<String>,
) -> Result<(), TypesError> {
    let target = ServiceTarget::entity(collection, name).map_err(ServerError::from)?;
    match remote.call(&target, service, &json!({})) {
        Ok(reply) => {
            let reply = required_reply(reply, &format!("{collection}/{name}.{service}"))?;
            destination.insert(name.to_string(), trim_metadata(&reply)?);
            Ok(())
        }
        Err(error) if error.is_not_found() => {
            skipped.push(format!("{collection}/{name}: {error}"));
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn required_reply(reply: Option<Value>, call: &str) -> Result<Value, TypesError> {
    reply.ok_or_else(|| TypesError::Platform(format!("{call} returned an empty body")))
}

fn trim_metadata(value: &Value) -> Result<PlatformMeta, TypesError> {
    let object =
        |value: Option<&Value>, what: &str| -> Result<BTreeMap<String, Value>, TypesError> {
            match value {
                None | Some(Value::Null) => Ok(BTreeMap::new()),
                Some(Value::Object(map)) => {
                    Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                }
                Some(_) => Err(TypesError::Platform(format!(
                    "platform metadata {what} is not an object"
                ))),
            }
        };
    let mut services = BTreeMap::new();
    for (name, raw) in object(value.get("serviceDefinitions"), "serviceDefinitions")? {
        let inputs = object(
            raw.pointer("/Inputs/fieldDefinitions"),
            "service Inputs.fieldDefinitions",
        )?
        .into_iter()
        .map(|(name, field)| {
            let aspects = field.get("aspects");
            Ok((
                name,
                PlatformInput {
                    base_type: required_string(&field, "baseType")?,
                    data_shape: optional_string(aspects.and_then(|v| v.get("dataShape"))),
                    required: aspects
                        .and_then(|v| v.get("isRequired"))
                        .is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true")),
                    description: optional_string(field.get("description")).unwrap_or_default(),
                },
            ))
        })
        .collect::<Result<_, TypesError>>()?;
        let outputs = raw.get("Outputs").unwrap_or(&Value::Null);
        services.insert(
            name,
            PlatformService {
                description: optional_string(raw.get("description")).unwrap_or_default(),
                inputs,
                result: PlatformValue {
                    base_type: required_string(outputs, "baseType")?,
                    data_shape: optional_string(outputs.get("dataShape")),
                },
            },
        );
    }
    let mut properties = BTreeMap::new();
    for (name, raw) in object(value.get("propertyDefinitions"), "propertyDefinitions")? {
        properties.insert(
            name,
            PlatformProperty {
                base_type: required_string(&raw, "baseType")?,
                data_shape: optional_string(raw.pointer("/aspects/dataShape")),
                description: optional_string(raw.get("description")).unwrap_or_default(),
            },
        );
    }
    Ok(PlatformMeta {
        services,
        properties,
    })
}

fn required_string(value: &Value, field: &str) -> Result<String, TypesError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| TypesError::Platform(format!("platform metadata has no string {field}")))
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn write_service_projects(
    solution: &Solution,
    model: &Model,
    skipped: &mut Vec<String>,
) -> Result<(usize, usize), TypesError> {
    let known_shapes: BTreeSet<&str> = model
        .data_shapes
        .iter()
        .map(|shape| shape.name.as_str())
        .collect();
    let data_shape_ids = identifiers(
        model.data_shapes.iter().map(|shape| shape.name.as_str()),
        "D_",
    );
    let entity_keys: Vec<String> = model
        .entities
        .iter()
        .map(|entity| format!("{}\0{}", entity.name, entity.collection))
        .collect();
    let entity_ids = identifiers(entity_keys.iter().map(String::as_str), "E_");

    let mut services = 0;
    let mut files_written = 0;
    for entity in &model.entities {
        let entity_key = format!("{}\0{}", entity.name, entity.collection);
        let services_dir = solution.src_root().join(&entity.name).join("services");
        for member in &entity.members {
            let Member::Service(service) = member else {
                continue;
            };
            let directory = services_dir.join(&service.name);
            let script_path = directory.join("script.js");
            if !script_path.is_file() {
                continue;
            }
            let script = match std::fs::read_to_string(&script_path) {
                Ok(script) => script,
                Err(error) => {
                    skipped.push(format!("{}: {error}", script_path.display()));
                    continue;
                }
            };
            let jsconfig = render_jsconfig(&solution.root, &directory);
            let globals = render_globals(
                service,
                &entity_ids[&entity_key],
                &script,
                &known_shapes,
                &data_shape_ids,
            );
            files_written += usize::from(workspace::write_lf_if_changed(
                &directory.join("jsconfig.json"),
                &jsconfig,
            )?);
            files_written += usize::from(workspace::write_lf_if_changed(
                &directory.join("twaco-globals.d.ts"),
                &globals,
            )?);
            services += 1;
        }
    }
    Ok((services, files_written))
}

fn render_jsconfig(root: &Path, service_dir: &Path) -> String {
    let depth = service_dir
        .strip_prefix(root)
        .expect("a service sidecar is inside the solution root")
        .components()
        .count();
    let relative_root = std::iter::repeat_n("..", depth)
        .collect::<Vec<_>>()
        .join("/");
    let value = serde_json::json!({
        "compilerOptions": {
            "allowJs": true,
            "checkJs": false,
            "noEmit": true,
            "target": "ES2015",
            "lib": ["ES2015"],
            "types": []
        },
        "include": [
            "script.js",
            "twaco-globals.d.ts",
            format!("{relative_root}/.twaco/types/*.d.ts")
        ]
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("the jsconfig value is JSON encodable")
    )
}

fn render_globals(
    service: &Service,
    entity_id: &str,
    script: &str,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
) -> String {
    let mut out = format!("declare const me: twx.{entity_id};\n");
    for parameter in &service.parameters {
        if script_declares(script, &parameter.name) {
            render_skipped_global(&mut out, &parameter.name);
            continue;
        }
        if !parameter.description.is_empty() {
            out.push_str(&format!("/** {} */\n", jsdoc_text(&parameter.description)));
        }
        out.push_str(&format!(
            "declare let {}: {};\n",
            parameter.name,
            type_name(&parameter.value, known_shapes, data_shape_ids)
        ));
    }
    if service.result.base_type.eq_ignore_ascii_case("NOTHING") {
        return out;
    }
    if script_declares(script, "result") {
        render_skipped_global(&mut out, "result");
    } else {
        out.push_str(&format!(
            "declare let result: {};\n",
            type_name(&service.result, known_shapes, data_shape_ids)
        ));
    }
    out
}

#[derive(Debug)]
struct CheckProject {
    generated_name: String,
    script_path: PathBuf,
    globals_path: PathBuf,
    header_lines: usize,
}

fn check_globals(
    service: &Service,
    entity_id: &str,
    script: &str,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut globals = vec![("me".to_string(), format!("twx.{entity_id}"))];
    globals.extend(
        service
            .parameters
            .iter()
            .filter(|parameter| !script_declares(script, &parameter.name))
            .map(|parameter| {
                (
                    parameter.name.clone(),
                    type_name(&parameter.value, known_shapes, data_shape_ids),
                )
            }),
    );
    if !service.result.base_type.eq_ignore_ascii_case("NOTHING")
        && !script_declares(script, "result")
    {
        globals.push((
            "result".to_string(),
            type_name(&service.result, known_shapes, data_shape_ids),
        ));
    }
    globals
}

fn write_check_project(
    solution: &Solution,
    model: &Model,
) -> Result<Vec<CheckProject>, CheckError> {
    struct Pending {
        script_path: PathBuf,
        globals_path: PathBuf,
        script: String,
        globals: Vec<(String, String)>,
    }

    let known_shapes: BTreeSet<&str> = model
        .data_shapes
        .iter()
        .map(|shape| shape.name.as_str())
        .collect();
    let data_shape_ids = identifiers(
        model.data_shapes.iter().map(|shape| shape.name.as_str()),
        "D_",
    );
    let entity_keys: Vec<String> = model
        .entities
        .iter()
        .map(|entity| format!("{}\0{}", entity.name, entity.collection))
        .collect();
    let entity_ids = identifiers(entity_keys.iter().map(String::as_str), "E_");
    let mut pending = Vec::new();
    for entity in &model.entities {
        let entity_key = format!("{}\0{}", entity.name, entity.collection);
        let services_dir = solution.src_root().join(&entity.name).join("services");
        for member in &entity.members {
            let Member::Service(service) = member else {
                continue;
            };
            let directory = services_dir.join(&service.name);
            let script_path = directory.join("script.js");
            if !script_path.is_file() {
                continue;
            }
            let script = std::fs::read_to_string(&script_path)
                .map_err(|error| CheckError(format!("{}: {error}", script_path.display())))?;
            pending.push(Pending {
                globals: check_globals(
                    service,
                    &entity_ids[&entity_key],
                    &script,
                    &known_shapes,
                    &data_shape_ids,
                ),
                globals_path: directory.join("twaco-globals.d.ts"),
                script_path,
                script,
            });
        }
    }
    pending.sort_by(|a, b| a.script_path.cmp(&b.script_path));

    let directory = solution.root.join(".twaco/types/check");
    match std::fs::remove_dir_all(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(CheckError(format!("{}: {error}", directory.display()))),
    }
    std::fs::create_dir_all(&directory)
        .map_err(|error| CheckError(format!("{}: {error}", directory.display())))?;

    let mut projects = Vec::new();
    for (index, item) in pending.into_iter().enumerate() {
        let generated_name = format!("s{index:04}.js");
        let mut source = String::from("export {};\n");
        for (name, value_type) in &item.globals {
            source.push_str(&format!("/** @type {{{value_type}}} */ var {name};\n"));
        }
        let header_lines = 1 + item.globals.len();
        source.push_str(&item.script);
        let path = directory.join(&generated_name);
        std::fs::write(&path, source)
            .map_err(|error| CheckError(format!("{}: {error}", path.display())))?;
        projects.push(CheckProject {
            generated_name,
            script_path: item.script_path,
            globals_path: item.globals_path,
            header_lines,
        });
    }
    let tsconfig = json!({
        "compilerOptions": {
            "allowJs": true,
            "checkJs": true,
            "noEmit": true,
            "target": "ES2015",
            "lib": ["ES2015"],
            "types": [],
            "module": "ES2015",
            "moduleDetection": "force",
            "strict": false
        },
        "include": ["*.js", "../*.d.ts"]
    });
    let config_path = directory.join("tsconfig.json");
    std::fs::write(
        &config_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&tsconfig).expect("tsconfig is JSON encodable")
        ),
    )
    .map_err(|error| CheckError(format!("{}: {error}", config_path.display())))?;
    Ok(projects)
}

fn compiler_command(solution: &Solution) -> Vec<OsString> {
    if let Some(configured) = &solution.types.tsc {
        return configured.iter().map(OsString::from).collect();
    }
    let local = solution.root.join("node_modules/typescript/bin/tsc");
    if local.is_file() {
        return vec![OsString::from("node"), local.into_os_string()];
    }
    vec![OsString::from(if cfg!(windows) {
        "tsc.cmd"
    } else {
        "tsc"
    })]
}

#[derive(Debug, PartialEq, Eq)]
struct CompilerFinding {
    file: String,
    line: usize,
    column: usize,
    code: String,
    message: String,
}

fn parse_compiler_output(output: &str) -> Vec<CompilerFinding> {
    let mut findings: Vec<CompilerFinding> = Vec::new();
    for line in output.lines() {
        if let Some(finding) = parse_compiler_line(line) {
            findings.push(finding);
        } else if line.starts_with([' ', '\t']) {
            if let Some(previous) = findings.last_mut() {
                let continuation = line.trim();
                if !continuation.is_empty() {
                    previous.message.push(' ');
                    previous.message.push_str(continuation);
                }
            }
        }
    }
    findings
}

fn parse_compiler_line(line: &str) -> Option<CompilerFinding> {
    const MARKER: &str = "): error TS";
    let marker = line.find(MARKER)?;
    let before = &line[..marker];
    let coordinates = before.rfind('(')?;
    let file = &before[..coordinates];
    let (line_number, column) = before[coordinates + 1..].split_once(',')?;
    let after_code = &line[marker + MARKER.len()..];
    let (code, message) = after_code.split_once(':')?;
    if file.is_empty() || code.is_empty() || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(CompilerFinding {
        file: file.to_string(),
        line: line_number.parse().ok()?,
        column: column.parse().ok()?,
        code: code.to_string(),
        message: message.trim_start().to_string(),
    })
}

fn map_finding(
    solution: &Solution,
    projects: &[CheckProject],
    finding: &CompilerFinding,
) -> (String, usize, Option<usize>) {
    let basename = finding
        .file
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&finding.file);
    if let Some((index, project)) = projects
        .iter()
        .enumerate()
        .find(|(_, project)| project.generated_name == basename)
    {
        if finding.line <= project.header_lines {
            return (
                relative_path(solution, &project.globals_path),
                finding.line,
                Some(index),
            );
        }
        return (
            relative_path(solution, &project.script_path),
            finding.line - project.header_lines,
            Some(index),
        );
    }
    let path = Path::new(&finding.file);
    let file = if path.is_absolute() {
        relative_path(solution, path)
    } else {
        finding.file.replace('\\', "/")
    };
    (file, finding.line, None)
}

fn relative_path(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn render_skipped_global(out: &mut String, name: &str) {
    out.push_str(&format!(
        "// Skipped {name}: script.js declares it at the start of a line.\n"
    ));
}

fn script_declares(script: &str, name: &str) -> bool {
    script.lines().any(|line| {
        ["let", "const", "var", "function"].iter().any(|keyword| {
            let Some(after_keyword) = line.strip_prefix(keyword) else {
                return false;
            };
            let Some(first) = after_keyword.chars().next() else {
                return false;
            };
            if !first.is_whitespace() {
                return false;
            }
            let after_name = after_keyword.trim_start().strip_prefix(name);
            after_name.is_some_and(|rest| {
                let is_word = |character: char| character.is_alphanumeric() || character == '_';
                name.chars().last().is_some_and(is_word) != rest.chars().next().is_some_and(is_word)
            })
        })
    })
}

enum Parsed {
    Entity(Entity),
    DataShape(DataShape),
}

fn parse_document(src: &[u8], collection: &str, name: &str) -> Result<Parsed, ParseError> {
    let tokens = scan::tokenize(src).map_err(ParseError::from)?;
    let entity_at = sidecar::entity_element(&tokens, src)
        .ok_or_else(|| ParseError("not a ThingWorx entity export".to_string()))?;
    let name = attribute(&tokens[entity_at], src, "name")?
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| name.to_string());
    if collection == "DataShapes" {
        return datashape::extract(src)
            .map(|fields| Parsed::DataShape(DataShape { name, fields }))
            .map_err(|error| ParseError(error.to_string()));
    }
    let host = sidecar::member_host_of(&tokens, src)
        .ok_or_else(|| ParseError("entity has no member host".to_string()))?;
    let template_attribute = if collection == "Things" {
        "thingTemplate"
    } else {
        "baseThingTemplate"
    };
    let template =
        attribute(&tokens[entity_at], src, template_attribute)?.filter(|value| !value.is_empty());
    let mut shapes = Vec::new();
    // Things and templates export this beside their nested local ThingShape. Shape entities
    // keep their member sections directly, and accepting both locations also handles older
    // source-control layouts without descending into unrelated metadata.
    let parents = if entity_at == host {
        vec![entity_at]
    } else {
        vec![entity_at, host]
    };
    for parent in parents {
        if let Some(&section) = scan::child_tags(&tokens, src, "ImplementedShapes", parent).first()
        {
            for at in scan::child_tags(&tokens, src, "ImplementedShape", section) {
                if let Some(shape) =
                    attribute(&tokens[at], src, "name")?.filter(|value| !value.is_empty())
                {
                    shapes.push(shape);
                }
            }
        }
    }

    let mut members = Vec::new();
    let services = sidecar::named_children_of(
        &tokens,
        src,
        host,
        "ServiceDefinitions",
        "ServiceDefinition",
    )
    .map_err(|error| ParseError(error.to_string()))?;
    for (service_name, at) in services {
        members.push(Member::Service(parse_service(
            &tokens,
            src,
            at,
            service_name,
        )?));
    }
    let properties = sidecar::named_children_of(
        &tokens,
        src,
        host,
        "PropertyDefinitions",
        "PropertyDefinition",
    )
    .map_err(|error| ParseError(error.to_string()))?;
    for (property_name, at) in properties {
        members.push(Member::Property(Property {
            name: property_name,
            description: attribute(&tokens[at], src, "description")?.unwrap_or_default(),
            value: typed_value(&tokens[at], src)?,
        }));
    }
    members.sort_by(|a, b| a.name().cmp(b.name()));
    let mut script_services = BTreeSet::new();
    if let Some(&section) = scan::child_tags(&tokens, src, "ServiceImplementations", host).first() {
        for implementation in scan::child_tags(&tokens, src, "ServiceImplementation", section) {
            if sidecar::handler_of(&tokens, src, implementation)
                .map_err(|error| ParseError(error.to_string()))?
                == "Script"
                && sidecar::code_element_of(&tokens, src, implementation).is_some()
            {
                if let Some(name) =
                    attribute(&tokens[implementation], src, "name")?.filter(|name| !name.is_empty())
                {
                    script_services.insert(name);
                }
            }
        }
    }
    Ok(Parsed::Entity(Entity {
        name,
        collection: collection.to_string(),
        project: String::new(),
        template,
        shapes,
        members,
        script_services,
    }))
}

fn parse_service(
    tokens: &[Token],
    src: &[u8],
    at: usize,
    name: String,
) -> Result<Service, ParseError> {
    let mut parameters = Vec::new();
    if let Some(&section) = scan::child_tags(tokens, src, "ParameterDefinitions", at).first() {
        for parameter_at in scan::child_tags(tokens, src, "FieldDefinition", section) {
            parameters.push(Parameter {
                name: attribute(&tokens[parameter_at], src, "name")?.unwrap_or_default(),
                description: attribute(&tokens[parameter_at], src, "description")?
                    .unwrap_or_default(),
                value: typed_value(&tokens[parameter_at], src)?,
                required: attribute(&tokens[parameter_at], src, "aspect.isRequired")?.as_deref()
                    == Some("true"),
                default: attribute(&tokens[parameter_at], src, "aspect.defaultValue")?,
            });
        }
    }
    parameters.sort_by(|a, b| a.name.cmp(&b.name));
    let result = scan::child_tags(tokens, src, "ResultType", at)
        .first()
        .map(|&result_at| typed_value(&tokens[result_at], src))
        .transpose()?
        .unwrap_or(TypedValue {
            base_type: "NOTHING".to_string(),
            data_shape: None,
        });
    Ok(Service {
        name,
        description: attribute(&tokens[at], src, "description")?.unwrap_or_default(),
        parameters,
        result,
    })
}

fn typed_value(tag: &Token, src: &[u8]) -> Result<TypedValue, ParseError> {
    Ok(TypedValue {
        base_type: attribute(tag, src, "baseType")?.unwrap_or_default(),
        data_shape: attribute(tag, src, "aspect.dataShape")?.filter(|value| !value.is_empty()),
    })
}

fn attribute(tag: &Token, src: &[u8], name: &str) -> Result<Option<String>, ParseError> {
    scan::attribute(src, tag, name)
        .map(|value| {
            value.map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
        })
        .map_err(ParseError::from)
}

#[derive(Debug)]
struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<ScanError> for ParseError {
    fn from(value: ScanError) -> Self {
        Self(value.to_string())
    }
}

fn generate(model: &Model, platform: Option<&Platform>) -> Generated {
    let data_shape_names: BTreeSet<&str> = model
        .data_shapes
        .iter()
        .map(|shape| shape.name.as_str())
        .collect();
    let data_shape_ids = identifiers(
        model.data_shapes.iter().map(|shape| shape.name.as_str()),
        "D_",
    );
    let entity_keys: Vec<String> = model
        .entities
        .iter()
        .map(|entity| format!("{}\0{}", entity.name, entity.collection))
        .collect();
    let entity_ids = identifiers(entity_keys.iter().map(String::as_str), "E_");

    Generated {
        datashapes: render_datashapes(model, &data_shape_names, &data_shape_ids),
        entities: render_entities(
            model,
            platform,
            &data_shape_names,
            &data_shape_ids,
            &entity_ids,
        ),
        collections: render_collections(
            model,
            platform,
            &data_shape_names,
            &data_shape_ids,
            &entity_ids,
        ),
    }
}

fn identifiers<'a>(
    names: impl IntoIterator<Item = &'a str>,
    prefix: &str,
) -> BTreeMap<String, String> {
    let mut names: Vec<&str> = names.into_iter().collect();
    names.sort();
    let mut counts = BTreeMap::<String, usize>::new();
    let mut taken = BTreeSet::<String>::new();
    let mut out = BTreeMap::new();
    for name in names {
        let source_name = name
            .split_once('\0')
            .map_or(name, |(entity_name, _)| entity_name);
        let stem: String = source_name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        let base = format!("{prefix}{stem}");
        // A suffixed identifier can itself be another name's plain one (`A.B` twice beside
        // `A_B_2`), so a candidate is taken only once nothing holds it.
        let count = counts.entry(base.clone()).or_default();
        let mut identifier = base.clone();
        while taken.contains(&identifier) {
            *count += 1;
            identifier = format!("{base}_{}", *count + 1);
        }
        taken.insert(identifier.clone());
        out.insert(name.to_string(), identifier);
    }
    out
}

fn render_datashapes(
    model: &Model,
    known_shapes: &BTreeSet<&str>,
    identifiers: &BTreeMap<String, String>,
) -> String {
    let mut out = String::from("declare namespace twx.ds {\n");
    for shape in &model.data_shapes {
        out.push_str(&format!(
            "    /** {} (DataShapes) */\n",
            jsdoc_text(&shape.name)
        ));
        out.push_str(&format!("    interface {} {{\n", identifiers[&shape.name]));
        let mut fields = shape.fields.iter().collect::<Vec<_>>();
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        for field in fields {
            render_description(&mut out, 8, &field.description);
            let data_shape = match field.aspects.get("dataShape") {
                Some(Aspect::Text(value)) => Some(value.as_str()),
                _ => None,
            };
            let value = TypedValue {
                base_type: field.base_type.clone(),
                data_shape: data_shape.map(str::to_string),
            };
            out.push_str(&format!(
                "        {}?: {};\n",
                single_quoted(&field.name),
                type_name(&value, known_shapes, identifiers)
            ));
        }
        out.push_str("    }\n\n");
    }
    out.push_str("}\n");
    out
}

fn render_entities(
    model: &Model,
    platform: Option<&Platform>,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
    entity_ids: &BTreeMap<String, String>,
) -> String {
    let mut out = String::from("declare namespace twx {\n");
    for entity in &model.entities {
        let key = format!("{}\0{}", entity.name, entity.collection);
        out.push_str(&format!(
            "    /** {} ({}) */\n    interface {} {{\n",
            jsdoc_text(&entity.name),
            jsdoc_text(&entity.collection),
            entity_ids[&key]
        ));
        let (members, open) = flattened_members(entity, model, platform);
        for member in members.values() {
            match member {
                Member::Property(property) => {
                    render_description(&mut out, 8, &property.description);
                    out.push_str(&format!(
                        "        {}: {};\n",
                        member_name(&property.name),
                        type_name(&property.value, known_shapes, data_shape_ids)
                    ));
                }
                Member::Service(service) => {
                    render_service(&mut out, service, known_shapes, data_shape_ids)
                }
            }
        }
        if open {
            out.push_str("        [member: string]: any;\n");
        }
        out.push_str("    }\n\n");
    }
    out.push_str("}\n");
    out
}

fn flattened_members<'a>(
    entity: &'a Entity,
    model: &'a Model,
    platform: Option<&'a Platform>,
) -> (BTreeMap<String, Member>, bool) {
    let by_collection = |collection: &str, name: &str| {
        model
            .entities
            .iter()
            .find(|candidate| candidate.collection == collection && candidate.name == name)
    };
    let mut members = BTreeMap::new();
    let mut visited_templates = BTreeSet::new();
    let mut visited_shapes = BTreeSet::new();
    let mut current = Some(entity);
    let mut open = entity.collection == "ThingShapes";
    let mut platform_template = None;
    while let Some(item) = current {
        for member in &item.members {
            members
                .entry(member.name().to_string())
                .or_insert_with(|| member.clone());
        }
        for shape_name in &item.shapes {
            if visited_shapes.insert(shape_name.as_str()) {
                if let Some(shape) = by_collection("ThingShapes", shape_name) {
                    for member in &shape.members {
                        members
                            .entry(member.name().to_string())
                            .or_insert_with(|| member.clone());
                    }
                }
            }
        }
        let Some(template_name) = item.template.as_deref() else {
            break;
        };
        if !visited_templates.insert(template_name) {
            break;
        }
        match by_collection("ThingTemplates", template_name) {
            Some(template) => current = Some(template),
            None => {
                if let Some(meta) = platform.and_then(|cache| cache.templates.get(template_name)) {
                    platform_template = Some(meta);
                } else {
                    open = true;
                }
                break;
            }
        }
    }
    if let Some(platform) = platform {
        // External shapes are less specific than every repository member, but more specific than
        // the complete external template response merged below.
        let mut external = BTreeMap::new();
        let mut current = Some(entity);
        let mut visited = BTreeSet::new();
        while let Some(item) = current {
            for name in &item.shapes {
                if by_collection("ThingShapes", name).is_none() && visited.insert(name.as_str()) {
                    if let Some(meta) = platform.shapes.get(name) {
                        merge_platform_meta(&mut external, meta);
                    }
                }
            }
            current = item
                .template
                .as_deref()
                .and_then(|name| by_collection("ThingTemplates", name));
        }
        for (name, member) in external {
            members.entry(name).or_insert(member);
        }
        if let Some(meta) = platform_template {
            merge_platform_meta(&mut members, meta);
        }
        if entity.collection == "ThingShapes" {
            if let Some(generic) = platform.templates.get("GenericThing") {
                merge_platform_meta(&mut members, generic);
            }
        }
    }
    (members, open)
}

fn merge_platform_meta(members: &mut BTreeMap<String, Member>, meta: &PlatformMeta) {
    for (name, property) in &meta.properties {
        members.entry(name.clone()).or_insert_with(|| {
            Member::Property(Property {
                name: name.clone(),
                description: property.description.clone(),
                value: TypedValue {
                    base_type: property.base_type.clone(),
                    data_shape: property.data_shape.clone(),
                },
            })
        });
    }
    for (name, service) in &meta.services {
        members.entry(name.clone()).or_insert_with(|| {
            Member::Service(Service {
                name: name.clone(),
                description: service.description.clone(),
                parameters: service
                    .inputs
                    .iter()
                    .map(|(name, input)| Parameter {
                        name: name.clone(),
                        description: input.description.clone(),
                        value: TypedValue {
                            base_type: input.base_type.clone(),
                            data_shape: input.data_shape.clone(),
                        },
                        required: input.required,
                        default: None,
                    })
                    .collect(),
                result: TypedValue {
                    base_type: service.result.base_type.clone(),
                    data_shape: service.result.data_shape.clone(),
                },
            })
        });
    }
}

fn render_service(
    out: &mut String,
    service: &Service,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
) {
    let described_parameters: Vec<&Parameter> = service
        .parameters
        .iter()
        .filter(|parameter| !parameter.description.is_empty())
        .collect();
    if !service.description.is_empty() || !described_parameters.is_empty() {
        out.push_str("        /**\n");
        for line in jsdoc_lines(&service.description) {
            out.push_str(&format!("         * {line}\n"));
        }
        for parameter in described_parameters {
            out.push_str(&format!(
                "         * @param {} {}\n",
                jsdoc_text(&parameter.name),
                jsdoc_text(&parameter.description)
            ));
        }
        out.push_str("         */\n");
    }
    out.push_str("        ");
    out.push_str(&member_name(&service.name));
    if service.parameters.is_empty() {
        out.push_str("()");
    } else {
        let required = service
            .parameters
            .iter()
            .any(|parameter| parameter.required);
        out.push_str("(params");
        if !required {
            out.push('?');
        }
        out.push_str(": { ");
        for (index, parameter) in service.parameters.iter().enumerate() {
            if index > 0 {
                out.push_str("; ");
            }
            out.push_str(&member_name(&parameter.name));
            if !parameter.required {
                out.push('?');
            }
            out.push_str(": ");
            out.push_str(&type_name(&parameter.value, known_shapes, data_shape_ids));
        }
        out.push_str(" })");
    }
    out.push_str(": ");
    out.push_str(&type_name(&service.result, known_shapes, data_shape_ids));
    out.push_str(";\n");
}

fn type_name(
    value: &TypedValue,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
) -> String {
    let upper = value.base_type.to_ascii_uppercase();
    match upper.as_str() {
        "NUMBER" | "INTEGER" | "LONG" => "number".to_string(),
        "BOOLEAN" => "boolean".to_string(),
        "DATETIME" => "Date".to_string(),
        "JSON" => "any".to_string(),
        "NOTHING" => "void".to_string(),
        "LOCATION" => "twx.LOCATION".to_string(),
        "INFOTABLE" => value
            .data_shape
            .as_deref()
            .filter(|name| known_shapes.contains(name))
            .and_then(|name| data_shape_ids.get(name))
            .map_or_else(
                || "twx.INFOTABLE<any>".to_string(),
                |identifier| format!("twx.INFOTABLE<twx.ds.{identifier}>"),
            ),
        "STRING" | "TEXT" | "HTML" | "HYPERLINK" | "IMAGELINK" | "PASSWORD" | "GUID" | "XML" => {
            "string".to_string()
        }
        // A query is a JSON object ({ filters, sorts }), passed as one, not its text.
        "QUERY" => "any".to_string(),
        _ if upper.ends_with("NAME") => "string".to_string(),
        _ => "any".to_string(),
    }
}

fn render_collections(
    model: &Model,
    platform: Option<&Platform>,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
    entity_ids: &BTreeMap<String, String>,
) -> String {
    let mut out = String::from("declare namespace twx {\n");
    for (collection, map) in [
        ("Things", "ThingsMap"),
        ("ThingTemplates", "ThingTemplatesMap"),
        ("ThingShapes", "ThingShapesMap"),
    ] {
        out.push_str(&format!("    interface {map} {{\n"));
        for entity in model
            .entities
            .iter()
            .filter(|entity| entity.collection == collection)
        {
            let key = format!("{}\0{}", entity.name, entity.collection);
            out.push_str(&format!(
                "        {}: twx.{};\n",
                double_quoted(&entity.name),
                entity_ids[&key]
            ));
        }
        out.push_str("    }\n\n");
    }
    if let Some(platform) = platform {
        let resource_ids = identifiers(platform.resources.keys().map(String::as_str), "R_");
        out.push_str("    interface ResourcesMap {\n");
        for name in platform.resources.keys() {
            out.push_str(&format!(
                "        {}: twx.{};\n",
                double_quoted(name),
                resource_ids[name]
            ));
        }
        out.push_str("    }\n\n");
        for (name, meta) in &platform.resources {
            out.push_str(&format!(
                "    /** {} (Resources) */\n    interface {} {{\n",
                jsdoc_text(name),
                resource_ids[name]
            ));
            let mut members = BTreeMap::new();
            merge_platform_meta(&mut members, meta);
            for member in members.values() {
                match member {
                    Member::Property(property) => {
                        render_description(&mut out, 8, &property.description);
                        out.push_str(&format!(
                            "        {}: {};\n",
                            member_name(&property.name),
                            type_name(&property.value, known_shapes, data_shape_ids)
                        ));
                    }
                    Member::Service(service) => {
                        render_service(&mut out, service, known_shapes, data_shape_ids);
                    }
                }
            }
            out.push_str("    }\n\n");
        }
    }
    out.push_str("}\n\n");
    for (collection, map) in [
        ("Things", "ThingsMap"),
        ("ThingTemplates", "ThingTemplatesMap"),
        ("ThingShapes", "ThingShapesMap"),
    ] {
        out.push_str(&format!(
            "declare const {collection}: twx.{map} & {{ [name: string]: any }};\n"
        ));
    }
    out.push_str("declare const DataShapes: { [name: string]: any };\n");
    if platform.is_some() {
        out.push_str("declare const Resources: twx.ResourcesMap & { [name: string]: any };\n");
    } else {
        out.push_str("declare const Resources: { [name: string]: any };\n");
    }
    for collection in PLAIN_COLLECTIONS {
        out.push_str(&format!(
            "declare const {collection}: {{ [name: string]: any }};\n"
        ));
    }
    out
}

fn render_description(out: &mut String, indent: usize, description: &str) {
    if description.is_empty() {
        return;
    }
    let padding = " ".repeat(indent);
    out.push_str(&format!("{padding}/**\n"));
    for line in jsdoc_lines(description) {
        out.push_str(&format!("{padding} * {line}\n"));
    }
    out.push_str(&format!("{padding} */\n"));
}

fn jsdoc_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        value
            .replace("*/", "*\\/")
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn jsdoc_text(value: &str) -> String {
    value.replace("*/", "*\\/").replace(['\r', '\n'], " ")
}

fn member_name(value: &str) -> String {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '$')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '$'
        });
    if valid {
        value.to_string()
    } else {
        single_quoted(value)
    }
}

fn single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn double_quoted(value: &str) -> String {
    serde_json::to_string(value).expect("a Rust string is always JSON encodable")
}

fn ensure_lf_end(value: &str) -> String {
    let mut value = value.replace("\r\n", "\n");
    if !value.ends_with('\n') {
        value.push('\n');
    }
    value
}

fn gitignore_covers_types(root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join(".gitignore")) else {
        return false;
    };
    let entries: BTreeSet<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.trim_start_matches('/').trim_end_matches('/'))
        .collect();
    let shared = entries.iter().any(|line| {
        let line = line.trim_start_matches('/').trim_end_matches('/');
        matches!(
            line,
            ".twaco" | ".twaco/**" | ".twaco/types" | ".twaco/types/**"
        )
    });
    shared
        && entries.contains("**/services/*/jsconfig.json")
        && entries.contains("**/services/*/twaco-globals.d.ts")
}

#[cfg(test)]
mod tests {
    use super::super::server::Method;
    use super::*;
    use std::cell::RefCell;

    fn document(collection: &str, body: &str) -> String {
        format!("<Entities><{collection}>{body}</{collection}></Entities>")
    }

    fn entity(collection: &str, body: &str) -> Entity {
        let name_start = body.find("name=\"").unwrap() + 6;
        let name_end = name_start + body[name_start..].find('"').unwrap();
        match parse_document(
            document(collection, body).as_bytes(),
            collection,
            &body[name_start..name_end],
        )
        .unwrap()
        {
            Parsed::Entity(entity) => entity,
            Parsed::DataShape(_) => panic!("expected entity"),
        }
    }

    fn shape(body: &str) -> DataShape {
        match parse_document(
            document("DataShapes", body).as_bytes(),
            "DataShapes",
            "Rows",
        )
        .unwrap()
        {
            Parsed::DataShape(shape) => shape,
            Parsed::Entity(_) => panic!("expected DataShape"),
        }
    }

    fn rendered(model: Model) -> Generated {
        generate(&model, None)
    }

    fn platform_property(base_type: &str) -> PlatformProperty {
        PlatformProperty {
            base_type: base_type.to_string(),
            data_shape: None,
            description: String::new(),
        }
    }

    fn temporary_root(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "twaco-types-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn fixture_solution(
        label: &str,
        entities: &[(&str, &str, &str)],
    ) -> (std::path::PathBuf, Solution) {
        let root = temporary_root(label);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\ncollections = [\"Things\", \"ThingTemplates\", \"ThingShapes\"]\n").unwrap();
        for (collection, name, xml) in entities {
            std::fs::create_dir_all(root.join(collection)).unwrap();
            std::fs::write(root.join(collection).join(format!("{name}.xml")), xml).unwrap();
        }
        let solution = Solution::discover(&root).unwrap();
        (root, solution)
    }

    type FakeReplies = BTreeMap<(String, String), Result<Option<Value>, ServerError>>;

    struct FakeRemote {
        calls: RefCell<Vec<(String, String, Value)>>,
        replies: RefCell<FakeReplies>,
    }

    impl FakeRemote {
        fn new(replies: FakeReplies) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                replies: RefCell::new(replies),
            }
        }
    }

    impl Remote for FakeRemote {
        fn call(
            &self,
            target: &ServiceTarget,
            service: &str,
            parameters: &Value,
        ) -> Result<Option<Value>, ServerError> {
            self.calls.borrow_mut().push((
                target.to_string(),
                service.to_string(),
                parameters.clone(),
            ));
            self.replies
                .borrow_mut()
                .remove(&(target.to_string(), service.to_string()))
                .unwrap_or_else(|| panic!("unexpected call {target}.{service}"))
        }
    }

    fn metadata(property: &str) -> Value {
        json!({
            "serviceDefinitions": {
                "Run": {
                    "description": "",
                    "Inputs": { "fieldDefinitions": {
                        "optional": { "baseType": "STRING", "description": "", "aspects": {
                            "isRequired": false, "dataShape": ""
                        }},
                        "needed": { "baseType": "INFOTABLE", "description": "Rows", "aspects": {
                            "isRequired": true, "dataShape": "ExternalRows"
                        }}
                    }},
                    "Outputs": { "baseType": "NOTHING", "dataShape": "" }
                }
            },
            "propertyDefinitions": {
                property: { "baseType": "STRING", "description": "", "aspects": { "dataShape": "" } }
            }
        })
    }

    fn ok(value: Value) -> Result<Option<Value>, ServerError> {
        Ok(Some(value))
    }

    fn service_of(entity: &Entity) -> &Service {
        entity
            .members
            .iter()
            .find_map(|member| match member {
                Member::Service(service) => Some(service),
                Member::Property(_) => None,
            })
            .unwrap()
    }

    #[test]
    fn jsconfig_points_to_types_from_flat_and_nested_src_roots() {
        let flat = render_jsconfig(
            Path::new("solution"),
            Path::new("solution/src/T/services/S"),
        );
        assert_eq!(
            flat,
            concat!(
                "{\n",
                "  \"compilerOptions\": {\n",
                "    \"allowJs\": true,\n",
                "    \"checkJs\": false,\n",
                "    \"noEmit\": true,\n",
                "    \"target\": \"ES2015\",\n",
                "    \"lib\": [\n",
                "      \"ES2015\"\n",
                "    ],\n",
                "    \"types\": []\n",
                "  },\n",
                "  \"include\": [\n",
                "    \"script.js\",\n",
                "    \"twaco-globals.d.ts\",\n",
                "    \"../../../../.twaco/types/*.d.ts\"\n",
                "  ]\n",
                "}\n"
            )
        );

        let nested = render_jsconfig(
            Path::new("solution"),
            Path::new("solution/Project Files/X-SourceControl/src/T/services/S"),
        );
        assert!(nested.contains("../../../../../../.twaco/types/*.d.ts"));
    }

    #[test]
    fn globals_use_the_owning_thing_template_and_shape_interfaces() {
        for (collection, body, expected, result) in [
            (
                "Things",
                r#"<Thing name="A.Thing"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ParameterDefinitions><FieldDefinition name="count" baseType="NUMBER" description="How many"/></ParameterDefinitions><ResultType baseType="STRING"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing>"#,
                "E_A_Thing",
                "declare let result: string;",
            ),
            (
                "ThingTemplates",
                r#"<ThingTemplate name="A.Template"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ResultType baseType="BOOLEAN"/></ServiceDefinition></ServiceDefinitions></ThingShape></ThingTemplate>"#,
                "E_A_Template",
                "declare let result: boolean;",
            ),
            (
                "ThingShapes",
                r#"<ThingShape name="A.Shape"><ServiceDefinitions><ServiceDefinition name="Run"><ResultType baseType="DATETIME"/></ServiceDefinition></ServiceDefinitions></ThingShape>"#,
                "E_A_Shape",
                "declare let result: Date;",
            ),
        ] {
            let owner = entity(collection, body);
            let globals = render_globals(
                service_of(&owner),
                expected,
                "result = 1;",
                &BTreeSet::new(),
                &BTreeMap::new(),
            );
            assert!(globals.starts_with(&format!("declare const me: twx.{expected};\n")));
            assert!(globals.contains(result));
            if collection == "Things" {
                assert!(globals.contains("/** How many */\ndeclare let count: number;"));
            }
        }
    }

    #[test]
    fn gitignore_must_cover_shared_and_per_service_generated_files() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("twaco-types-ignore-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".gitignore"), ".twaco/types/\n").unwrap();
        assert!(!gitignore_covers_types(&root));
        std::fs::write(
            root.join(".gitignore"),
            ".twaco/types/\n**/services/*/jsconfig.json\n**/services/*/twaco-globals.d.ts\n",
        )
        .unwrap();
        assert!(gitignore_covers_types(&root));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn top_level_declarations_skip_the_matching_globals_only() {
        let owner = entity(
            "Things",
            r#"<Thing name="T"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ParameterDefinitions><FieldDefinition name="p" baseType="STRING"/></ParameterDefinitions><ResultType baseType="NUMBER"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing>"#,
        );
        let service = service_of(&owner);
        for declaration in ["let result = 1;", "const result = 1;", "var result = 1;"] {
            let globals = render_globals(
                service,
                "E_T",
                declaration,
                &BTreeSet::new(),
                &BTreeMap::new(),
            );
            assert!(globals.contains("// Skipped result:"), "{declaration}");
            assert!(!globals.contains("declare let result:"), "{declaration}");
        }
        let parameter = render_globals(
            service,
            "E_T",
            "function p() {}",
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert!(parameter.contains("// Skipped p:"));
        assert!(!parameter.contains("declare let p:"));
        assert!(parameter.contains("declare let result: number;"));

        let indented = render_globals(
            service,
            "E_T",
            "function f() {\n    let result = 1;\n}",
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert!(indented.contains("declare let result: number;"));
        assert!(!indented.contains("// Skipped result:"));
    }

    #[test]
    fn void_service_has_no_result_global() {
        let owner = entity(
            "ThingShapes",
            r#"<ThingShape name="S"><ServiceDefinitions><ServiceDefinition name="Run"><ResultType baseType="NOTHING"/></ServiceDefinition></ServiceDefinitions></ThingShape>"#,
        );
        let globals = render_globals(
            service_of(&owner),
            "E_S",
            "result = 1;",
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert_eq!(globals, "declare const me: twx.E_S;\n");
    }

    #[test]
    fn service_project_generation_is_byte_deterministic() {
        let owner = entity(
            "Things",
            r#"<Thing name="T"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ParameterDefinitions><FieldDefinition name="p" baseType="BOOLEAN" description="Flag"/></ParameterDefinitions><ResultType baseType="NUMBER"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing>"#,
        );
        let first = render_globals(
            service_of(&owner),
            "E_T",
            "result = p;",
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        let second = render_globals(
            service_of(&owner),
            "E_T",
            "result = p;",
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert_eq!(first.as_bytes(), second.as_bytes());
        assert_eq!(
            render_jsconfig(Path::new("r"), Path::new("r/src/T/services/Run")),
            render_jsconfig(Path::new("r"), Path::new("r/src/T/services/Run"))
        );
    }

    #[test]
    fn maps_thingworx_base_types() {
        let known = BTreeSet::new();
        let ids = BTreeMap::new();
        for (base, expected) in [
            ("NUMBER", "number"),
            ("INTEGER", "number"),
            ("LONG", "number"),
            ("BOOLEAN", "boolean"),
            ("DATETIME", "Date"),
            ("JSON", "any"),
            ("NOTHING", "void"),
            ("LOCATION", "twx.LOCATION"),
            ("STRING", "string"),
            ("THINGNAME", "string"),
            ("QUERY", "any"),
            ("BOGUS", "any"),
        ] {
            assert_eq!(
                type_name(
                    &TypedValue {
                        base_type: base.into(),
                        data_shape: None
                    },
                    &known,
                    &ids
                ),
                expected
            );
        }
    }

    #[test]
    fn infotable_uses_only_an_in_solution_datashape() {
        let model = Model {
            data_shapes: vec![shape(
                r#"<DataShape name="Rows"><FieldDefinitions/></DataShape>"#,
            )],
            entities: vec![entity(
                "Things",
                r#"<Thing name="T"><ThingShape><PropertyDefinitions>
                <PropertyDefinition name="Known" baseType="INFOTABLE" aspect.dataShape="Rows"/>
                <PropertyDefinition name="Unknown" baseType="INFOTABLE" aspect.dataShape="Elsewhere"/>
            </PropertyDefinitions></ThingShape></Thing>"#,
            )],
        };
        let text = rendered(model).entities;
        assert!(text.contains("Known: twx.INFOTABLE<twx.ds.D_Rows>;"));
        assert!(text.contains("Unknown: twx.INFOTABLE<any>;"));
    }

    #[test]
    fn flattens_template_chain_and_shapes_with_nearest_service_winning() {
        let own = entity(
            "Things",
            r#"<Thing name="T" thingTemplate="Near"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="Repeat"><ResultType baseType="STRING"/></ServiceDefinition>
        </ServiceDefinitions></ThingShape></Thing>"#,
        );
        let near = entity(
            "ThingTemplates",
            r#"<ThingTemplate name="Near" baseThingTemplate="Base"><ThingShape>
            <PropertyDefinitions><PropertyDefinition name="NearProp" baseType="NUMBER"/></PropertyDefinitions>
            <ServiceDefinitions><ServiceDefinition name="Repeat"><ResultType baseType="NUMBER"/></ServiceDefinition></ServiceDefinitions>
        </ThingShape><ImplementedShapes><ImplementedShape name="S"/></ImplementedShapes></ThingTemplate>"#,
        );
        let base = entity(
            "ThingTemplates",
            r#"<ThingTemplate name="Base"><ThingShape>
            <PropertyDefinitions><PropertyDefinition name="BaseProp" baseType="BOOLEAN"/></PropertyDefinitions>
        </ThingShape></ThingTemplate>"#,
        );
        let implemented = entity(
            "ThingShapes",
            r#"<ThingShape name="S"><PropertyDefinitions>
            <PropertyDefinition name="ShapeProp" baseType="DATETIME"/>
        </PropertyDefinitions></ThingShape>"#,
        );
        let text = rendered(Model {
            entities: vec![base, near, implemented, own],
            data_shapes: vec![],
        })
        .entities;
        let thing = text
            .split("interface E_T")
            .nth(1)
            .unwrap()
            .split("    }")
            .next()
            .unwrap();
        assert!(thing.contains("NearProp: number"));
        assert!(thing.contains("BaseProp: boolean"));
        assert!(thing.contains("ShapeProp: Date"));
        assert!(thing.contains("Repeat(): string"));
        assert!(!thing.contains("Repeat(): number"));
    }

    #[test]
    fn opens_external_template_chains_but_not_closed_solution_chains() {
        let open = entity(
            "Things",
            r#"<Thing name="Open" thingTemplate="GenericThing"><ThingShape/></Thing>"#,
        );
        let closed = entity(
            "Things",
            r#"<Thing name="Closed" thingTemplate="Local"><ThingShape/></Thing>"#,
        );
        let local = entity(
            "ThingTemplates",
            r#"<ThingTemplate name="Local"><ThingShape/></ThingTemplate>"#,
        );
        let shape = entity("ThingShapes", r#"<ThingShape name="AlwaysOpen"/>"#);
        let text = rendered(Model {
            entities: vec![closed, local, open, shape],
            data_shapes: vec![],
        })
        .entities;
        let open_body = text
            .split("interface E_Open")
            .nth(1)
            .unwrap()
            .split("    }")
            .next()
            .unwrap();
        let closed_body = text
            .split("interface E_Closed")
            .nth(1)
            .unwrap()
            .split("    }")
            .next()
            .unwrap();
        let shape_body = text
            .split("interface E_AlwaysOpen")
            .nth(1)
            .unwrap()
            .split("    }")
            .next()
            .unwrap();
        assert!(open_body.contains("[member: string]: any"));
        assert!(!closed_body.contains("[member: string]: any"));
        assert!(shape_body.contains("[member: string]: any"));
    }

    #[test]
    fn suffixes_identifier_collisions_in_sorted_name_order() {
        let ids = identifiers(["A-B", "A.B", "A_B"], "E_");
        assert_eq!(ids["A-B"], "E_A_B");
        assert_eq!(ids["A.B"], "E_A_B_2");
        assert_eq!(ids["A_B"], "E_A_B_3");

        // A suffix never lands on another name's own identifier.
        let ids = identifiers(["A-B", "A.B", "A_B_2"], "E_");
        let unique: BTreeSet<&String> = ids.values().collect();
        assert_eq!(unique.len(), 3, "{ids:?}");
    }

    #[test]
    fn required_parameter_requires_the_argument_and_member() {
        let service = entity(
            "Things",
            r#"<Thing name="T"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="Run"><ParameterDefinitions>
                <FieldDefinition name="needed" baseType="STRING" aspect.isRequired="true"/>
                <FieldDefinition name="maybe" baseType="NUMBER"/>
            </ParameterDefinitions><ResultType baseType="NOTHING"/></ServiceDefinition>
        </ServiceDefinitions></ThingShape></Thing>"#,
        );
        let text = rendered(Model {
            entities: vec![service],
            data_shapes: vec![],
        })
        .entities;
        assert!(text.contains("Run(params: { maybe?: number; needed: string }): void;"));
    }

    #[test]
    fn escapes_a_jsdoc_terminator() {
        let property = entity(
            "Things",
            r#"<Thing name="T"><ThingShape><PropertyDefinitions>
            <PropertyDefinition name="P" baseType="STRING" description="before */ after"/>
        </PropertyDefinitions></ThingShape></Thing>"#,
        );
        let text = rendered(Model {
            entities: vec![property],
            data_shapes: vec![],
        })
        .entities;
        assert!(text.contains("before *\\/ after"));
        assert!(!text.contains("before */ after"));
    }

    #[test]
    fn generation_is_byte_deterministic() {
        let model = Model {
            entities: vec![entity("Things", r#"<Thing name="B"><ThingShape/></Thing>"#)],
            data_shapes: vec![shape(
                r#"<DataShape name="Rows"><FieldDefinitions>
                <FieldDefinition name="z" baseType="STRING"/><FieldDefinition name="a" baseType="NUMBER"/>
            </FieldDefinitions></DataShape>"#,
            )],
        };
        let first = generate(&model, None);
        let second = generate(&model, None);
        assert_eq!(first.datashapes.as_bytes(), second.datashapes.as_bytes());
        assert_eq!(first.entities.as_bytes(), second.entities.as_bytes());
        assert_eq!(first.collections.as_bytes(), second.collections.as_bytes());
    }

    #[test]
    fn platform_members_close_external_roots_and_keep_solution_precedence() {
        let model = Model {
            entities: vec![entity(
                "Things",
                r#"<Thing name="T" thingTemplate="External"><ThingShape><PropertyDefinitions>
                <PropertyDefinition name="Same" baseType="NUMBER"/>
                </PropertyDefinitions></ThingShape><ImplementedShapes><ImplementedShape name="ExternalShape"/></ImplementedShapes></Thing>"#,
            )],
            data_shapes: vec![],
        };
        let platform = Platform {
            templates: BTreeMap::from([(
                "External".into(),
                PlatformMeta {
                    properties: BTreeMap::from([
                        ("Same".into(), platform_property("BOOLEAN")),
                        ("TemplateOnly".into(), platform_property("STRING")),
                    ]),
                    ..PlatformMeta::default()
                },
            )]),
            shapes: BTreeMap::from([(
                "ExternalShape".into(),
                PlatformMeta {
                    properties: BTreeMap::from([
                        ("TemplateOnly".into(), platform_property("NUMBER")),
                        ("ShapeOnly".into(), platform_property("DATETIME")),
                    ]),
                    ..PlatformMeta::default()
                },
            )]),
            ..Platform::default()
        };
        let text = generate(&model, Some(&platform)).entities;
        let body = text
            .split("interface E_T")
            .nth(1)
            .unwrap()
            .split("    }")
            .next()
            .unwrap();
        assert!(body.contains("Same: number"));
        assert!(!body.contains("Same: boolean"));
        assert!(
            body.contains("TemplateOnly: number"),
            "shape precedes template: {body}"
        );
        assert!(body.contains("ShapeOnly: Date"));
        assert!(!body.contains("[member: string]: any"));
    }

    #[test]
    fn an_uncached_root_stays_open_and_a_shape_me_gets_generic_members() {
        let model = Model {
            entities: vec![
                entity(
                    "Things",
                    r#"<Thing name="T" thingTemplate="Missing"><ThingShape/></Thing>"#,
                ),
                entity(
                    "ThingShapes",
                    r#"<ThingShape name="S"><PropertyDefinitions><PropertyDefinition name="Own" baseType="NUMBER"/></PropertyDefinitions></ThingShape>"#,
                ),
            ],
            data_shapes: vec![],
        };
        let platform = Platform {
            templates: BTreeMap::from([(
                "GenericThing".into(),
                PlatformMeta {
                    properties: BTreeMap::from([("name".into(), platform_property("STRING"))]),
                    ..PlatformMeta::default()
                },
            )]),
            ..Platform::default()
        };
        let text = generate(&model, Some(&platform)).entities;
        let thing = text
            .split("interface E_T")
            .nth(1)
            .unwrap()
            .split("    }")
            .next()
            .unwrap();
        let shape = text
            .split("interface E_S")
            .nth(1)
            .unwrap()
            .split("    }")
            .next()
            .unwrap();
        assert!(thing.contains("[member: string]: any"));
        assert!(shape.contains("Own: number"));
        assert!(shape.contains("name: string"));
        assert!(shape.contains("[member: string]: any"));
    }

    #[test]
    fn cached_resources_get_their_own_interfaces_and_unknown_fallback() {
        let platform = Platform {
            resources: BTreeMap::from([(
                "InfoTableFunctions".into(),
                PlatformMeta {
                    properties: BTreeMap::from([("Version".into(), platform_property("STRING"))]),
                    ..PlatformMeta::default()
                },
            )]),
            ..Platform::default()
        };
        let text = generate(&Model::default(), Some(&platform)).collections;
        assert!(text.contains("\"InfoTableFunctions\": twx.R_InfoTableFunctions;"));
        assert!(text.contains("interface R_InfoTableFunctions"));
        assert!(text.contains("Version: string;"));
        assert!(
            text.contains("declare const Resources: twx.ResourcesMap & { [name: string]: any };")
        );
        let without = generate(&Model::default(), None).collections;
        assert!(without.contains("declare const Resources: { [name: string]: any };"));
        assert!(!without.contains("ResourcesMap"));
    }

    #[test]
    fn trimming_omits_empty_optional_fields_and_is_deterministic() {
        let trimmed = trim_metadata(&metadata("P")).unwrap();
        let first = serde_json::to_string_pretty(&trimmed).unwrap();
        let second = serde_json::to_string_pretty(&trimmed).unwrap();
        assert_eq!(first, second);
        assert!(!first.contains("\"description\": \"\""));
        assert!(!first.contains("\"required\": false"));
        assert!(!first.contains("\"dataShape\": \"\""));
        assert!(first.contains("\"required\": true"));
        assert!(first.contains("\"dataShape\": \"ExternalRows\""));
    }

    #[test]
    fn fetches_solution_platform_dependencies_and_all_resources() {
        let (root, solution) = fixture_solution(
            "fetch",
            &[
                (
                    "Things",
                    "T",
                    r#"<Entities><Things><Thing name="T" thingTemplate="Local" projectName="P"><ThingShape/><ImplementedShapes><ImplementedShape name="OutsideShape"/></ImplementedShapes></Thing></Things></Entities>"#,
                ),
                (
                    "ThingTemplates",
                    "Local",
                    r#"<Entities><ThingTemplates><ThingTemplate name="Local" baseThingTemplate="OutsideTemplate" projectName="P"><ThingShape/></ThingTemplate></ThingTemplates></Entities>"#,
                ),
            ],
        );
        let mut replies = BTreeMap::new();
        for (collection, name) in [
            ("ThingTemplates", "GenericThing"),
            ("ThingTemplates", "OutsideTemplate"),
            ("ThingShapes", "OutsideShape"),
        ] {
            replies.insert(
                (
                    format!("{collection}/{name}"),
                    "GetInstanceMetadataAsJSON".into(),
                ),
                ok(metadata(name)),
            );
        }
        replies.insert(
            ("Resources/EntityServices".into(), "GetEntityList".into()),
            ok(json!({ "rows": [{"name": "Zed"}, {"name": "Alpha"}] })),
        );
        for name in ["Alpha", "Zed"] {
            replies.insert(
                (format!("Resources/{name}"), "GetMetadataAsJSON".into()),
                ok(metadata(name)),
            );
        }
        let remote = FakeRemote::new(replies);
        let outcome = fetch_platform(&remote, &solution).unwrap();
        assert_eq!(
            (outcome.templates, outcome.shapes, outcome.resources),
            (2, 1, 2)
        );
        let calls = remote.calls.borrow();
        assert!(calls
            .iter()
            .any(|(target, _, _)| target == "ThingTemplates/OutsideTemplate"));
        assert!(!calls
            .iter()
            .any(|(target, _, _)| target == "ThingTemplates/Local"));
        let list = calls
            .iter()
            .find(|(target, _, _)| target == "Resources/EntityServices")
            .unwrap();
        assert_eq!(list.2, json!({ "type": "Resource", "maxItems": 1000 }));
        let cache = std::fs::read_to_string(root.join(".twaco/platform.json")).unwrap();
        assert!(cache.ends_with('\n'));
        assert!(cache.find("\"Alpha\"").unwrap() < cache.find("\"Zed\"").unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_not_found_entity_is_skipped_but_an_auth_failure_writes_nothing() {
        let (root, solution) = fixture_solution(
            "failures",
            &[(
                "Things",
                "T",
                r#"<Entities><Things><Thing name="T" thingTemplate="Absent" projectName="P"><ThingShape/></Thing></Things></Entities>"#,
            )],
        );
        let not_found = ServerError::Http {
            method: Method::Post,
            status: 404,
            url: "test/Absent".into(),
            body: String::new(),
        };
        let remote = FakeRemote::new(BTreeMap::from([
            (
                (
                    "ThingTemplates/Absent".into(),
                    "GetInstanceMetadataAsJSON".into(),
                ),
                Err(not_found),
            ),
            (
                (
                    "ThingTemplates/GenericThing".into(),
                    "GetInstanceMetadataAsJSON".into(),
                ),
                ok(metadata("name")),
            ),
            (
                ("Resources/EntityServices".into(), "GetEntityList".into()),
                ok(json!({"rows": []})),
            ),
        ]));
        let outcome = fetch_platform(&remote, &solution).unwrap();
        assert_eq!(outcome.templates, 1);
        assert_eq!(outcome.skipped.len(), 1);
        assert!(root.join(".twaco/platform.json").is_file());

        let old_cache = b"old cache that must survive";
        std::fs::write(root.join(".twaco/platform.json"), old_cache).unwrap();
        let unauthorized = ServerError::Http {
            method: Method::Post,
            status: 401,
            url: "test/GenericThing".into(),
            body: "unauthorized".into(),
        };
        let remote = FakeRemote::new(BTreeMap::from([
            (
                (
                    "ThingTemplates/Absent".into(),
                    "GetInstanceMetadataAsJSON".into(),
                ),
                Err(ServerError::Http {
                    method: Method::Post,
                    status: 404,
                    url: "test/Absent".into(),
                    body: String::new(),
                }),
            ),
            (
                (
                    "ThingTemplates/GenericThing".into(),
                    "GetInstanceMetadataAsJSON".into(),
                ),
                Err(unauthorized),
            ),
        ]));
        assert!(fetch_platform(&remote, &solution).is_err());
        assert_eq!(
            std::fs::read(root.join(".twaco/platform.json")).unwrap(),
            old_cache
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_cache_is_reported_and_ignored() {
        let (root, solution) = fixture_solution(
            "malformed",
            &[(
                "Things",
                "T",
                r#"<Entities><Things><Thing name="T" thingTemplate="GenericThing" projectName="P"><ThingShape/></Thing></Things></Entities>"#,
            )],
        );
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join(".twaco/platform.json"), "{not json").unwrap();
        let outcome = write(&solution).unwrap();
        assert!(outcome
            .skipped
            .iter()
            .any(|message| message.contains("malformed and was ignored")));
        let entities = std::fs::read_to_string(root.join(".twaco/types/entities.d.ts")).unwrap();
        assert!(entities.contains("[member: string]: any"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn writing_the_same_generated_bytes_twice_is_a_fixed_point() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("twaco-types-{}-{nonce}", std::process::id()));
        let path = directory.join("types.d.ts");
        assert!(workspace::write_lf_if_changed(&path, "one\r\ntwo\r\n").unwrap());
        let first = std::fs::read(&path).unwrap();
        assert!(!workspace::write_lf_if_changed(&path, "one\ntwo\n").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), first);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn generated_service_projects_do_not_change_check_gate_results() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("twaco-types-check-{}-{nonce}", std::process::id()));
        let service_dir = root.join("src/T/services/Run");
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(&service_dir).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\ncollections = [\"Things\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("Things/T.xml"),
            concat!(
                "<Entities><Things><Thing name=\"T\" projectName=\"P\"><ThingShape>",
                "<ServiceDefinitions><ServiceDefinition name=\"Run\"><ResultType baseType=\"NUMBER\"/>",
                "</ServiceDefinition></ServiceDefinitions>",
                "<ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\">",
                "<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>",
                "<code><![CDATA[result = 1;]]></code>",
                "</Row></Rows></ConfigurationTable></ConfigurationTables>",
                "</ServiceImplementation></ServiceImplementations>",
                "</ThingShape></Thing></Things></Entities>"
            ),
        )
        .unwrap();
        std::fs::write(service_dir.join("definition.xml"), "<ServiceDefinition name=\"Run\"><ResultType baseType=\"NUMBER\"/></ServiceDefinition>\n").unwrap();
        std::fs::write(service_dir.join("script.js"), "result = 1;\n").unwrap();

        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let before = crate::core::check::run(&solution);
        write(&solution).unwrap();
        let after = crate::core::check::run(&solution);
        assert_eq!(before.gates.len(), after.gates.len());
        for (before, after) in before.gates.iter().zip(&after.gates) {
            assert_eq!(before.name, after.name);
            assert_eq!(before.examined, after.examined, "{}", before.name);
            assert_eq!(before.findings, after.findings, "{}", before.name);
            assert_eq!(before.prose, after.prose, "{}", before.name);
            assert_eq!(before.broken, after.broken, "{}", before.name);
            assert_eq!(before.gates_the_run, after.gates_the_run, "{}", before.name);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    fn check_fixture(label: &str, script: &str) -> (PathBuf, Solution) {
        let xml = concat!(
            "<Entities><Things><Thing name=\"T\" projectName=\"P\"><ThingShape>",
            "<ServiceDefinitions><ServiceDefinition name=\"Run\"><ParameterDefinitions>",
            "<FieldDefinition name=\"input\" baseType=\"STRING\"/>",
            "</ParameterDefinitions><ResultType baseType=\"NUMBER\"/></ServiceDefinition>",
            "</ServiceDefinitions></ThingShape></Thing></Things></Entities>"
        );
        let (root, solution) = fixture_solution(label, &[("Things", "T", xml)]);
        let directory = root.join("src/T/services/Run");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("script.js"), script.as_bytes()).unwrap();
        (root, solution)
    }

    #[test]
    fn check_file_has_typed_header_and_verbatim_script_but_no_skipped_global() {
        let script = "let input = 'local';\r\nresult = input.length;\r\n";
        let (root, solution) = check_fixture("check-project", script);
        let (model, _) = load_model(&solution);
        let projects = write_check_project(&solution, &model).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].generated_name, "s0000.js");
        assert_eq!(projects[0].header_lines, 3);
        let generated = std::fs::read(root.join(".twaco/types/check/s0000.js")).unwrap();
        let header = concat!(
            "export {};\n",
            "/** @type {twx.E_T} */ var me;\n",
            "/** @type {number} */ var result;\n"
        );
        assert!(generated.starts_with(header.as_bytes()));
        assert_eq!(&generated[header.len()..], script.as_bytes());
        assert!(!String::from_utf8_lossy(&generated).contains("var input;"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_compiler_findings_continuations_and_windows_paths_with_spaces() {
        let parsed = parse_compiler_output(concat!(
            "C:\\work space\\.twaco\\types\\check\\s0001.js(12,7): error TS2345: first line\r\n",
            "  The detailed reason\r\n",
            ".twaco/types/entities.d.ts(3,2): error TS1005: ';' expected.\n"
        ));
        assert_eq!(
            parsed,
            vec![
                CompilerFinding {
                    file: "C:\\work space\\.twaco\\types\\check\\s0001.js".into(),
                    line: 12,
                    column: 7,
                    code: "2345".into(),
                    message: "first line The detailed reason".into(),
                },
                CompilerFinding {
                    file: ".twaco/types/entities.d.ts".into(),
                    line: 3,
                    column: 2,
                    code: "1005".into(),
                    message: "';' expected.".into()
                },
            ]
        );
    }

    #[test]
    fn maps_script_header_and_non_service_findings() {
        let solution: Solution = toml::from_str("[[project]]\nname = \"P\"\n").unwrap();
        let mut solution = solution;
        solution.root = PathBuf::from("C:/solution");
        let projects = vec![CheckProject {
            generated_name: "s0000.js".into(),
            script_path: solution.root.join("src/T/services/Run/script.js"),
            globals_path: solution.root.join("src/T/services/Run/twaco-globals.d.ts"),
            header_lines: 4,
        }];
        let finding = |file: &str, line| CompilerFinding {
            file: file.into(),
            line,
            column: 2,
            code: "1".into(),
            message: "x".into(),
        };
        assert_eq!(
            map_finding(&solution, &projects, &finding("s0000.js", 7)),
            ("src/T/services/Run/script.js".into(), 3, Some(0))
        );
        assert_eq!(
            map_finding(&solution, &projects, &finding("s0000.js", 2)),
            ("src/T/services/Run/twaco-globals.d.ts".into(), 2, Some(0))
        );
        assert_eq!(
            map_finding(
                &solution,
                &projects,
                &finding(".twaco/types/entities.d.ts", 9)
            ),
            (".twaco/types/entities.d.ts".into(), 9, None)
        );
    }

    #[test]
    fn compiler_discovery_prefers_configuration_then_local_then_path() {
        let (root, mut solution) = fixture_solution("compiler-order", &[]);
        assert_eq!(
            compiler_command(&solution)[0],
            OsString::from(if cfg!(windows) { "tsc.cmd" } else { "tsc" })
        );

        let local = root.join("node_modules/typescript/bin/tsc");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "").unwrap();
        assert_eq!(
            compiler_command(&solution),
            vec![OsString::from("node"), local.clone().into_os_string()]
        );

        solution.types.tsc = Some(vec!["custom-tsc".into(), "--flag".into()]);
        assert_eq!(
            compiler_command(&solution),
            vec![OsString::from("custom-tsc"), OsString::from("--flag")]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    struct FakeCompiler {
        success: bool,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        calls: RefCell<Vec<(OsString, Vec<OsString>, PathBuf)>>,
    }

    impl CompilerRunner for FakeCompiler {
        fn run(
            &self,
            program: &OsStr,
            arguments: &[OsString],
            current_dir: &Path,
        ) -> std::io::Result<CompilerOutput> {
            self.calls.borrow_mut().push((
                program.to_os_string(),
                arguments.to_vec(),
                current_dir.to_path_buf(),
            ));
            Ok(CompilerOutput {
                success: self.success,
                stdout: self.stdout.clone(),
                stderr: self.stderr.clone(),
            })
        }
    }

    #[test]
    fn compiler_errors_without_a_parsable_finding_are_a_broken_run() {
        let (root, solution) = fixture_solution("empty-compiler-error", &[]);
        let compiler = FakeCompiler {
            success: false,
            stdout: Vec::new(),
            stderr: b"\nnode: cannot find module typescript\nlong stack trace\n".to_vec(),
            calls: RefCell::new(Vec::new()),
        };
        let error = check_with(&solution, &compiler).unwrap_err().to_string();
        assert!(error.contains("non-zero without a parsable finding"));
        assert!(error.contains("ran:"));
        assert!(error.contains("--pretty"));
        assert!(error.contains("tsconfig.json"));
        assert!(error.contains("node: cannot find module typescript"));
        assert!(!error.contains("long stack trace"));
        assert!(error.contains("npm install --save-dev typescript"));
        let calls = compiler.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.iter().any(|argument| argument == "--pretty"));
        assert_eq!(calls[0].2, root);
        let _ = std::fs::remove_dir_all(&calls[0].2);
    }

    #[test]
    fn json_check_output_is_all_findings_and_no_summary() {
        let outcome = CheckOutcome {
            declarations: Outcome::default(),
            findings: vec![
                TypeFinding {
                    file: "src/A/script.js".into(),
                    line: 3,
                    column: 7,
                    code: "2345".into(),
                    message: "first".into(),
                },
                TypeFinding {
                    file: "src/B/script.js".into(),
                    line: 9,
                    column: 2,
                    code: "1005".into(),
                    message: "second".into(),
                },
            ],
            affected_services: 2,
            services: 4,
            elapsed: Duration::from_millis(1250),
        };
        let stdout: Vec<String> = outcome.findings.iter().map(finding_json).collect();
        let parsed: Vec<_> = stdout
            .iter()
            .filter_map(|line| crate::core::check::parse_finding("hook", line))
            .collect();

        assert_eq!(parsed.len(), outcome.findings.len());
        assert!(parsed.iter().all(|finding| finding.gate == "types"));
        assert_eq!(parsed[0].rule, "TS2345");
        assert_eq!(parsed[0].message, "first (column 7)");
        assert!(!stdout.iter().any(|line| line.starts_with("types:")));
        assert!(check_summary(&outcome).starts_with("types: 2 finding(s)"));
    }
}
