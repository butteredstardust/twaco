//! Create the common ThingWorx entities as repository files.
//!
//! The skeletons are the 10.1 exports with volatile server metadata removed.  This module only
//! creates new files; subsequent changes are made by the scanners and sidecar synchronisers.

use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{Project, Solution};
use crate::core::datashape::{self, Aspect, Field};
use crate::core::lock::WorkspaceLock;
use crate::core::mashup_authoring::parse_visibility;
use crate::core::mashup_templates::Visibility;
use crate::core::refs;
use crate::core::transaction::{Transaction, TransactionError};
use crate::core::types;
use crate::core::workspace;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Thing,
    ThingTemplate,
    ThingShape,
    DataShape,
    Timer,
    Scheduler,
}

impl Kind {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "thing" => Self::Thing,
            "thing-template" => Self::ThingTemplate,
            "thing-shape" => Self::ThingShape,
            "data-shape" => Self::DataShape,
            "timer" => Self::Timer,
            "scheduler" => Self::Scheduler,
            _ => return None,
        })
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Thing => "thing",
            Self::ThingTemplate => "thing-template",
            Self::ThingShape => "thing-shape",
            Self::DataShape => "data-shape",
            Self::Timer => "timer",
            Self::Scheduler => "scheduler",
        }
    }

    pub fn collection(self) -> &'static str {
        match self {
            Self::Thing | Self::Timer | Self::Scheduler => "Things",
            Self::ThingTemplate => "ThingTemplates",
            Self::ThingShape => "ThingShapes",
            Self::DataShape => "DataShapes",
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewRequest {
    pub kind: Kind,
    pub name: String,
    pub project: Option<String>,
    pub description: String,
    pub visibility: Vec<String>,
    pub template: Option<String>,
    pub shapes: Vec<String>,
    pub value_stream: Option<String>,
    pub base: Option<String>,
    pub fields: Vec<String>,
    pub every: Option<String>,
    pub cron: Option<String>,
    pub run_as: Option<String>,
    pub disabled: bool,
    pub then: Option<String>,
    pub script: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Plan {
    pub request: NewRequest,
    pub project: String,
    pub entity_path: PathBuf,
    pub entity: Vec<u8>,
    pub sidecars: Vec<(PathBuf, Vec<u8>)>,
    pub duration_ms: Option<u64>,
    pub cron: Option<String>,
    pub script: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum NewError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unknown(String),
    #[error("{0}")]
    Exists(String),
    #[error("{0}")]
    Write(TransactionError),
}

impl Coded for NewError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Unknown(_) => ErrorCode::UnknownEntity,
            Self::Exists(_) => ErrorCode::AlreadyExists,
            Self::Write(error) => error.code(),
        }
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

fn project<'a>(
    solution: &'a Solution,
    requested: &Option<String>,
    kind: Kind,
) -> Result<&'a Project, NewError> {
    let project = match requested {
        Some(name) => solution
            .project(name)
            .ok_or_else(|| NewError::Unknown(format!("twaco.toml declares no project {name}")))?,
        None => match solution.projects.as_slice() {
            [only] => only,
            _ => return Err(NewError::Invalid("name the project with --project".into())),
        },
    };
    if !project.collections.is_empty()
        && !project.collections.iter().any(|c| c == kind.collection())
    {
        return Err(NewError::Invalid(format!(
            "project {} keeps only {}; add {} to its collections first",
            project.name,
            project.collections.join(", "),
            kind.collection()
        )));
    }
    Ok(project)
}

fn entity_exists(solution: &Solution, collection: &str, name: &str) -> bool {
    workspace::entities(solution)
        .iter()
        .any(|entry| entry.info.collection == collection && entry.info.name == name)
}

fn platform_has(solution: &Solution, kind: &str, name: &str) -> bool {
    types::cached_platform(solution).is_some_and(|platform| match kind {
        "ThingTemplates" => platform.templates.contains_key(name),
        "ThingShapes" => platform.shapes.contains_key(name),
        _ => false,
    })
}

fn known_entity(solution: &Solution, collection: &str, name: &str) -> bool {
    // GenericThing is the root of every platform Thing chain and is available even before a
    // repository has fetched its optional platform cache.
    (collection == "ThingTemplates" && name == "GenericThing")
        || entity_exists(solution, collection, name)
        || platform_has(solution, collection, name)
}

fn refuse_file_on_the_way(root: &Path, path: &Path) -> Result<(), NewError> {
    for folder in path.ancestors().skip(1) {
        if folder == root {
            break;
        }
        if folder
            .symlink_metadata()
            .is_ok_and(|metadata| !metadata.is_dir())
        {
            return Err(NewError::Exists(format!(
                "{} is a file where the new entity needs a folder",
                folder.display()
            )));
        }
    }
    Ok(())
}

pub fn parse_duration(text: &str) -> Result<u64, NewError> {
    let text = text.trim();
    let (number, multiplier) = if let Some(number) = text.strip_suffix("ms") {
        (number, 1)
    } else if let Some(number) = text.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = text.strip_suffix('m') {
        (number, 60_000)
    } else if let Some(number) = text.strip_suffix('h') {
        (number, 3_600_000)
    } else {
        (text, 1)
    };
    let value = number
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            NewError::Invalid(format!(
                "--every must be a positive duration such as 500ms, 30s, 5m or 1h, not {text:?}"
            ))
        })?;
    value
        .checked_mul(multiplier)
        .ok_or_else(|| NewError::Invalid("--every is too large".into()))
}

pub fn validate_cron(text: &str) -> Result<String, NewError> {
    let fields = text.split_whitespace().count();
    if !(fields == 6 || fields == 7) {
        return Err(NewError::Invalid(
            "--cron must have six or seven Quartz fields".into(),
        ));
    }
    Ok(text.to_string())
}

fn parse_fields(values: &[String]) -> Result<Vec<Field>, NewError> {
    if values.is_empty() {
        return Err(NewError::Invalid(
            "a data-shape needs at least one --field name:TYPE[:primary]".into(),
        ));
    }
    let mut names = BTreeSet::new();
    let known: BTreeSet<&str> = [
        "BOOLEAN",
        "DATETIME",
        "INTEGER",
        "LONG",
        "NUMBER",
        "STRING",
        "TEXT",
        "THINGNAME",
        "USERNAME",
        "JSON",
        "INFOTABLE",
        "BASETYPENAME",
    ]
    .into_iter()
    .collect();
    values.iter().enumerate().map(|(index, value)| {
        let parts: Vec<&str> = value.split(':').collect();
        if !(parts.len() == 2 || parts.len() == 3) || parts[0].is_empty() || !known.contains(parts[1]) || (parts.len() == 3 && parts[2] != "primary") {
            return Err(NewError::Invalid(format!("--field {value:?} must be name:TYPE or name:TYPE:primary, using a known base type")));
        }
        if !names.insert(parts[0].to_string()) { return Err(NewError::Invalid(format!("data-shape field {} is listed more than once", parts[0]))); }
        let mut aspects = BTreeMap::new();
        aspects.insert("isPrimaryKey".into(), Aspect::Bool(parts.get(2) == Some(&"primary")));
        Ok(Field { name: parts[0].into(), base_type: parts[1].into(), ordinal: (index + 1).to_string(), description: String::new(), aspects })
    }).collect()
}

fn permissions(visibility: &[Visibility]) -> String {
    let mut out = String::from("            <VisibilityPermissions>\n                <Visibility>");
    for visibility in visibility {
        out.push_str(&format!("\n                    <Principal\n                     isPermitted=\"true\"\n                     name=\"{}\"\n                     type=\"{}\"></Principal>", escape(&visibility.name), visibility.kind));
    }
    if visibility.is_empty() {
        out.push_str("</Visibility>\n            </VisibilityPermissions>");
    } else {
        out.push_str("\n                </Visibility>\n            </VisibilityPermissions>");
    }
    out
}

fn common_tail(visibility: &[Visibility]) -> String {
    format!("            <avatar></avatar>\n            <DesignTimePermissions>\n                <Create></Create>\n                <Read></Read>\n                <Update></Update>\n                <Delete></Delete>\n                <Metadata></Metadata>\n            </DesignTimePermissions>\n            <RunTimePermissions></RunTimePermissions>\n{}\n            <ConfigurationTableDefinitions></ConfigurationTableDefinitions>\n            <ConfigurationTables></ConfigurationTables>", permissions(visibility))
}

fn header(collection: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities\n majorVersion=\"10\"\n minorVersion=\"1\"\n universal=\"password\">\n    <{collection}>\n")
}
fn footer(collection: &str) -> String {
    format!("    </{collection}>\n</Entities>\n")
}

fn shapes(shapes: &[String]) -> String {
    let mut out = String::from("            <ImplementedShapes>");
    for shape in shapes {
        out.push_str(&format!("\n                <ImplementedShape\n                 name=\"{}\"\n                 type=\"ThingShape\"></ImplementedShape>", escape(shape)));
    }
    out.push_str("\n            </ImplementedShapes>");
    out
}

fn local_shape(subscription: Option<(&str, &str, &str)>) -> String {
    let mut out = String::from("            <ThingShape>\n                <PropertyDefinitions></PropertyDefinitions>\n                <ServiceDefinitions></ServiceDefinitions>\n                <EventDefinitions></EventDefinitions>\n                <ServiceMappings></ServiceMappings>\n                <ServiceImplementations></ServiceImplementations>\n                <Subscriptions>");
    if let Some((event, name, code)) = subscription {
        out.push_str(&format!("\n                    <Subscription\n                     description=\"\"\n                     enabled=\"true\"\n                     eventName=\"{event}\"\n                     name=\"{name}\"\n                     source=\"\"\n                     sourceProperty=\"\"\n                     sourceType=\"Thing\">\n                        <ServiceImplementation\n                         description=\"\"\n                         handlerName=\"Script\"\n                         name=\"{name}\">\n                            <ConfigurationTables>\n                                <ConfigurationTable\n                                 dataShapeName=\"\"\n                                 description=\"\"\n                                 isMultiRow=\"false\"\n                                 name=\"Script\"\n                                 ordinal=\"0\">\n                                    <DataShape>\n                                        <FieldDefinitions>\n                                            <FieldDefinition\n                                             baseType=\"STRING\"\n                                             description=\"code\"\n                                             name=\"code\"\n                                             ordinal=\"0\"></FieldDefinition>\n                                        </FieldDefinitions>\n                                    </DataShape>\n                                    <Rows>\n                                        <Row>\n                                            <code><![CDATA[{code}]]></code>\n                                        </Row>\n                                    </Rows>\n                                </ConfigurationTable>\n                            </ConfigurationTables>\n                        </ServiceImplementation>\n                    </Subscription>"));
    }
    out.push_str("\n                </Subscriptions>\n            </ThingShape>");
    out
}

fn scheduler_settings(kind: Kind, enabled: bool, run_as: &str, value: &str) -> String {
    let (field, description, default, element) = match kind {
        Kind::Timer => ("updateRate", "Update rate", "60000.0", "NUMBER"),
        Kind::Scheduler => (
            "schedule",
            "Execution Schedule (Cron String)",
            "0 0/1 * * * ?",
            "SCHEDULE",
        ),
        _ => unreachable!(),
    };
    let value = if kind == Kind::Scheduler {
        format!("<![CDATA[{value}]]>")
    } else {
        value.into()
    };
    format!("            <ConfigurationTables>\n                <ConfigurationTable\n                 dataShapeName=\"\"\n                 description=\"General Settings\"\n                 isHidden=\"true\"\n                 isMultiRow=\"false\"\n                 name=\"Settings\"\n                 ordinal=\"0\">\n                    <DataShape>\n                        <FieldDefinitions>\n                            <FieldDefinition\n                             aspect.defaultValue=\"true\"\n                             baseType=\"BOOLEAN\"\n                             description=\"Automatically enable {} on startup\"\n                             name=\"enabled\"\n                             ordinal=\"0\"></FieldDefinition>\n                            <FieldDefinition\n                             baseType=\"USERNAME\"\n                             description=\"User context in which to run event handlers\"\n                             name=\"runAsUser\"\n                             ordinal=\"0\"></FieldDefinition>\n                            <FieldDefinition\n                             aspect.defaultValue=\"{}\"\n                             baseType=\"{}\"\n                             description=\"{}\"\n                             name=\"{}\"\n                             ordinal=\"0\"></FieldDefinition>\n                        </FieldDefinitions>\n                    </DataShape>\n                    <Rows>\n                        <Row>\n                            <enabled>{enabled}</enabled>\n                            <runAsUser><![CDATA[{}]]></runAsUser>\n                            <{}>{value}</{}>\n                        </Row>\n                    </Rows>\n                </ConfigurationTable>\n            </ConfigurationTables>", kind.word(), default, element, description, field, run_as, field, field)
}

fn make_xml(
    request: &NewRequest,
    project: &str,
    visibility: &[Visibility],
    fields: &[Field],
    duration: Option<u64>,
    cron: Option<&str>,
    script: Option<&str>,
) -> String {
    let desc = escape(&request.description);
    let name = escape(&request.name);
    let common = common_tail(visibility);
    match request.kind {
        Kind::ThingShape => format!("{}        <ThingShape\n         aspect.isSystemObject=\"false\"\n         className=\"\"\n         description=\"{desc}\"\n         documentationContent=\"\"\n         homeMashup=\"\"\n         name=\"{name}\"\n         projectName=\"{project}\"\n         tags=\"\">\n            <PropertyDefinitions></PropertyDefinitions>\n            <ServiceDefinitions></ServiceDefinitions>\n            <EventDefinitions></EventDefinitions>\n            <ServiceMappings></ServiceMappings>\n            <ServiceImplementations></ServiceImplementations>\n            <Subscriptions></Subscriptions>\n{common}\n            <PropertyBindings></PropertyBindings>\n            <RemotePropertyBindings></RemotePropertyBindings>\n            <RemoteServiceBindings></RemoteServiceBindings>\n            <RemoteEventBindings></RemoteEventBindings>\n            <AlertConfigurations></AlertConfigurations>\n            <InstanceRunTimePermissions></InstanceRunTimePermissions>\n        </ThingShape>\n{}", header("ThingShapes"), footer("ThingShapes")),
        Kind::DataShape => {
            let field_xml: String = fields.iter().map(|field| format!("                <FieldDefinition\n                 aspect.isPrimaryKey=\"{}\"\n                 baseType=\"{}\"\n                 description=\"{}\"\n                 name=\"{}\"\n                 ordinal=\"{}\"></FieldDefinition>\n", if field.aspects.get("isPrimaryKey") == Some(&Aspect::Bool(true)) { "true" } else { "false" }, field.base_type, escape(&field.description), escape(&field.name), field.ordinal)).collect();
            format!("{}        <DataShape\n         baseDataShape=\"{}\"\n         description=\"{desc}\"\n         documentationContent=\"\"\n         homeMashup=\"\"\n         name=\"{name}\"\n         projectName=\"{project}\"\n         tags=\"\">\n{common}\n            <FieldDefinitions>\n{field_xml}            </FieldDefinitions>\n        </DataShape>\n{}", header("DataShapes"), escape(request.base.as_deref().unwrap_or("")), footer("DataShapes"))
        }
        Kind::ThingTemplate => format!("{}        <ThingTemplate\n         aspect.isSystemObject=\"false\"\n         baseThingTemplate=\"{}\"\n         description=\"{desc}\"\n         documentationContent=\"\"\n         effectiveThingPackage=\"ConfiguredThing\"\n         homeMashup=\"\"\n         inheritedValueStream=\"\"\n         name=\"{name}\"\n         projectName=\"{project}\"\n         tags=\"\"\n         thingPackage=\"\"\n         valueStream=\"\">\n{common}\n            <PropertyBindings></PropertyBindings>\n            <RemotePropertyBindings></RemotePropertyBindings>\n            <RemoteServiceBindings></RemoteServiceBindings>\n            <RemoteEventBindings></RemoteEventBindings>\n            <AlertConfigurations></AlertConfigurations>\n{}\n{}\n            <SharedConfigurationTables></SharedConfigurationTables>\n            <InstanceDesignTimePermissions>\n                <Create></Create>\n                <Read></Read>\n                <Update></Update>\n                <Delete></Delete>\n                <Metadata></Metadata>\n            </InstanceDesignTimePermissions>\n            <InstanceRunTimePermissions></InstanceRunTimePermissions>\n            <InstanceVisibilityPermissions>\n                <Visibility></Visibility>\n            </InstanceVisibilityPermissions>\n        </ThingTemplate>\n{}", header("ThingTemplates"), escape(request.base.as_deref().unwrap_or("GenericThing")), local_shape(None), shapes(&request.shapes), footer("ThingTemplates")),
        Kind::Thing | Kind::Timer | Kind::Scheduler => {
            let (template, package) = match request.kind { Kind::Timer => ("Timer", "TimerThing"), Kind::Scheduler => ("Scheduler", "SchedulerThing"), _ => (request.template.as_deref().unwrap_or("GenericThing"), "ConfiguredThing") };
            let setting = match request.kind { Kind::Timer => scheduler_settings(Kind::Timer, !request.disabled, request.run_as.as_deref().unwrap_or(""), &format!("{}.0", duration.unwrap())), Kind::Scheduler => scheduler_settings(Kind::Scheduler, !request.disabled, request.run_as.as_deref().unwrap_or(""), cron.unwrap()), _ => "            <ConfigurationTables></ConfigurationTables>".into() };
            // A Thing writes its own configuration tables (a timer's or scheduler's settings): one
            // element, not the shared empty one as well.
            let common = common
                .strip_suffix("\n            <ConfigurationTables></ConfigurationTables>")
                .unwrap_or(&common)
                .to_string();
            let subscription = match request.kind { Kind::Timer => script.map(|code| ("Timer", "TimerSubscription", code)), Kind::Scheduler => script.map(|code| ("ScheduledEvent", "SchedulerSubscription", code)), _ => None };
            format!("{}        <Thing\n         aspect.isSystemObject=\"false\"\n         description=\"{desc}\"\n         documentationContent=\"\"\n         effectiveThingPackage=\"{package}\"\n         enabled=\"true\"\n         homeMashup=\"\"\n         identifier=\"\"\n         inheritedValueStream=\"\"\n         name=\"{name}\"\n         projectName=\"{project}\"\n         published=\"false\"\n         tags=\"\"\n         thingTemplate=\"{}\"\n         valueStream=\"{}\">\n{common}\n{setting}\n{}\n            <PropertyBindings></PropertyBindings>\n            <RemotePropertyBindings></RemotePropertyBindings>\n            <RemoteServiceBindings></RemoteServiceBindings>\n            <RemoteEventBindings></RemoteEventBindings>\n            <AlertConfigurations></AlertConfigurations>\n{}\n            <ThingProperties></ThingProperties>\n        </Thing>\n{}", header("Things"), escape(template), escape(request.value_stream.as_deref().unwrap_or("")), local_shape(subscription), shapes(&request.shapes), footer("Things"))
        }
    }
}

pub fn plan(solution: &Solution, request: &NewRequest) -> Result<Plan, NewError> {
    refs::validate_new_name(&request.name).map_err(NewError::Invalid)?;
    if request.description.contains(['\n', '\r']) {
        return Err(NewError::Invalid("--description must be one line".into()));
    }
    if request.then.is_some() && request.script.is_some() {
        return Err(NewError::Invalid(
            "give --then or --script, not both".into(),
        ));
    }
    let project = project(solution, &request.project, request.kind)?;
    let visibility = request
        .visibility
        .iter()
        .map(|value| parse_visibility(value).map_err(|error| NewError::Invalid(error.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    let path = solution
        .project_root(project)
        .join(request.kind.collection())
        .join(format!("{}.xml", request.name));
    if path.exists() || entity_exists(solution, request.kind.collection(), &request.name) {
        return Err(NewError::Exists(format!(
            "{} {} already exists",
            request.kind.word(),
            request.name
        )));
    }
    crate::core::transaction::check_inside(&solution.root, &path).map_err(NewError::Invalid)?;
    refuse_file_on_the_way(&solution.root, &path)?;
    let template = request.template.as_deref().unwrap_or("GenericThing");
    if request.kind == Kind::Thing && !known_entity(solution, "ThingTemplates", template) {
        return Err(NewError::Unknown(format!(
            "no ThingTemplate {template} in the solution or platform cache"
        )));
    }
    if request.kind == Kind::ThingTemplate {
        let base = request.base.as_deref().unwrap_or("GenericThing");
        if !known_entity(solution, "ThingTemplates", base) {
            return Err(NewError::Unknown(format!(
                "no ThingTemplate {base} in the solution or platform cache"
            )));
        }
    }
    if request.kind == Kind::DataShape {
        if let Some(base) = request.base.as_deref() {
            if !base.is_empty() && !known_entity(solution, "DataShapes", base) {
                return Err(NewError::Unknown(format!(
                    "no DataShape {base} in the solution"
                )));
            }
        }
    }
    for shape in &request.shapes {
        if !known_entity(solution, "ThingShapes", shape) {
            return Err(NewError::Unknown(format!(
                "no ThingShape {shape} in the solution or platform cache"
            )));
        }
    }
    let fields = if request.kind == Kind::DataShape {
        parse_fields(&request.fields)?
    } else {
        Vec::new()
    };
    let duration = if request.kind == Kind::Timer {
        Some(parse_duration(request.every.as_deref().unwrap_or("60s"))?)
    } else {
        None
    };
    let cron = if request.kind == Kind::Scheduler {
        Some(validate_cron(
            request.cron.as_deref().unwrap_or("0 0/1 * * * ?"),
        )?)
    } else {
        None
    };
    if matches!(request.kind, Kind::Timer | Kind::Scheduler)
        && request.run_as.as_deref().is_none_or(str::is_empty)
    {
        return Err(NewError::Invalid(format!(
            "{} needs --run-as; an empty runAsUser runs events as nobody",
            request.kind.word()
        )));
    }
    let script = match (&request.then, &request.script) {
        (Some(code), None) => Some(code.clone()),
        (None, Some(path)) => Some(
            std::fs::read_to_string(path)
                .map_err(|error| NewError::Invalid(format!("{}: {error}", path.display())))?,
        ),
        _ => None,
    };
    // Both are written as CDATA, which cannot hold its own terminator.
    for value in [request.run_as.as_deref(), script.as_deref()]
        .into_iter()
        .flatten()
    {
        if value.contains("]]>") {
            return Err(NewError::Invalid(
                "--run-as and the script cannot contain `]]>`".into(),
            ));
        }
    }
    let entity = make_xml(
        request,
        &project.name,
        &visibility,
        &fields,
        duration,
        cron.as_deref(),
        script.as_deref(),
    )
    .into_bytes();
    let sidecars = if request.kind == Kind::DataShape {
        vec![(
            solution.src_root().join(&request.name).join("fields.json"),
            datashape::to_sidecar(&fields).into_bytes(),
        )]
    } else {
        Vec::new()
    };
    for (sidecar, _) in &sidecars {
        if sidecar.exists() {
            return Err(NewError::Exists(format!(
                "{} already exists",
                sidecar.display()
            )));
        }
        crate::core::transaction::check_inside(&solution.root, sidecar)
            .map_err(NewError::Invalid)?;
        refuse_file_on_the_way(&solution.root, sidecar)?;
    }
    Ok(Plan {
        request: request.clone(),
        project: project.name.clone(),
        entity_path: path,
        entity,
        sidecars,
        duration_ms: duration,
        cron,
        script,
    })
}

pub fn apply(solution: &Solution, plan: &Plan, lock: &WorkspaceLock) -> Result<(), NewError> {
    let mut transaction = Transaction::new(&solution.root, "new entity");
    transaction
        .create_file(&plan.entity_path, plan.entity.clone())
        .map_err(NewError::Write)?;
    for (path, bytes) in &plan.sidecars {
        transaction
            .create_file(path, bytes.clone())
            .map_err(NewError::Write)?;
    }
    transaction.apply(lock).map_err(NewError::Write)?;
    Ok(())
}

pub fn plan_json(solution: &Solution, plan: &Plan, applied: bool) -> Value {
    let relative = |path: &Path| {
        path.strip_prefix(&solution.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    json!({ "applied": applied, "kind": plan.request.kind.word(), "name": plan.request.name, "project": plan.project, "files": std::iter::once(relative(&plan.entity_path)).chain(plan.sidecars.iter().map(|(path, _)| relative(path))).collect::<Vec<_>>(), "every_ms": plan.duration_ms, "cron": plan.cron, "runs": plan.script.as_ref().map(|code| code.lines().next().unwrap_or_default()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::commands::{self, Mode, Notices};

    fn fixture() -> (tempfile::TempDir, Solution) {
        let dir = tempfile::Builder::new()
            .prefix("twaco-entity-new-")
            .tempdir()
            .unwrap();
        std::fs::write(
            dir.path().join("twaco.toml"),
            "[solution]\nsrc = \"src\"\n[[project]]\nname = \"Acme.Orders\"\nroot = \"app\"\n",
        )
        .unwrap();
        for collection in ["Things", "ThingTemplates", "ThingShapes", "DataShapes"] {
            std::fs::create_dir_all(dir.path().join("app").join(collection)).unwrap();
        }
        std::fs::write(dir.path().join("app/ThingTemplates/GenericThing.xml"), "<Entities><ThingTemplates><ThingTemplate name=\"GenericThing\" projectName=\"Acme.Orders\"/></ThingTemplates></Entities>").unwrap();
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn request(kind: Kind, name: &str) -> NewRequest {
        NewRequest {
            kind,
            name: name.into(),
            project: None,
            description: String::new(),
            visibility: Vec::new(),
            template: None,
            shapes: Vec::new(),
            value_stream: None,
            base: None,
            fields: Vec::new(),
            every: None,
            cron: None,
            run_as: None,
            disabled: false,
            then: None,
            script: None,
        }
    }

    fn apply(solution: &Solution, request: NewRequest) -> Plan {
        let command = commands::entity_new::Request {
            request,
            mode: Mode::Apply,
            lock_label: "test new entity",
        };
        commands::entity_new::execute(solution, &command, &mut Notices::default())
            .unwrap()
            .plan
    }

    fn assert_golden(name: &str, actual: &[u8]) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/core/entity_new/golden")
            .join(format!("{name}.xml"));
        if std::env::var_os("TWACO_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, actual).unwrap();
        }
        assert_eq!(
            std::fs::read(&path).unwrap_or_else(|error| panic!(
                "{}: set TWACO_BLESS=1 to create it ({error})",
                path.display()
            )),
            actual,
            "{name} differs from its 10.1 golden export"
        );
    }

    #[test]
    fn each_kind_plan_has_the_101_export_skeleton() {
        let (_dir, solution) = fixture();
        let mut thing = request(Kind::Thing, "Acme.Orders.Manager");
        thing.shapes.push("Acme.Orders.Audit_TS".into());
        std::fs::write(solution.root.join("app/ThingShapes/Acme.Orders.Audit_TS.xml"), "<Entities><ThingShapes><ThingShape name=\"Acme.Orders.Audit_TS\" projectName=\"Acme.Orders\"/></ThingShapes></Entities>").unwrap();
        let mut template = request(Kind::ThingTemplate, "Acme.Orders.Manager_TT");
        template.base = Some("GenericThing".into());
        let shape = request(Kind::ThingShape, "Acme.Orders.Audit2_TS");
        let mut data = request(Kind::DataShape, "Acme.Orders.Order_DS");
        data.fields = vec!["id:STRING:primary".into(), "created:DATETIME".into()];
        let mut timer = request(Kind::Timer, "Acme.Orders.Refresh_TM");
        timer.every = Some("30s".into());
        timer.run_as = Some("Administrator".into());
        timer.then = Some("me.Refresh();".into());
        let mut scheduler = request(Kind::Scheduler, "Acme.Orders.Nightly_SC");
        scheduler.run_as = Some("Administrator".into());
        scheduler.cron = Some("0 0 2 * * ?".into());
        let plans = [thing, template, shape, data, timer, scheduler]
            .map(|request| plan(&solution, &request).unwrap());
        for (name, plan) in [
            "thing",
            "thing-template",
            "thing-shape",
            "data-shape",
            "timer",
            "scheduler",
        ]
        .iter()
        .zip(&plans)
        {
            assert_golden(name, &plan.entity);
        }
        for plan in &plans {
            let xml = std::str::from_utf8(&plan.entity).unwrap();
            assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
            assert!(xml.contains("majorVersion=\"10\""));
            assert!(xml.contains("minorVersion=\"1\""));
        }
        assert!(std::str::from_utf8(&plans[0].entity)
            .unwrap()
            .contains("enabled=\"true\""));
        assert!(std::str::from_utf8(&plans[1].entity)
            .unwrap()
            .contains("SharedConfigurationTables"));
        assert!(std::str::from_utf8(&plans[3].entity)
            .unwrap()
            .contains("aspect.isPrimaryKey=\"true\""));
        assert_eq!(String::from_utf8(plans[3].sidecars[0].1.clone()).unwrap(), "[\n  {\n    \"name\": \"id\",\n    \"baseType\": \"STRING\",\n    \"ordinal\": \"1\",\n    \"description\": \"\",\n    \"aspects\": {\n      \"isPrimaryKey\": true\n    }\n  },\n  {\n    \"name\": \"created\",\n    \"baseType\": \"DATETIME\",\n    \"ordinal\": \"2\",\n    \"description\": \"\",\n    \"aspects\": {\n      \"isPrimaryKey\": false\n    }\n  }\n]\n");
        assert!(std::str::from_utf8(&plans[4].entity)
            .unwrap()
            .contains("<updateRate>30000.0</updateRate>"));
        assert!(std::str::from_utf8(&plans[4].entity)
            .unwrap()
            .contains("eventName=\"Timer\""));
        assert!(std::str::from_utf8(&plans[5].entity)
            .unwrap()
            .contains("<![CDATA[0 0 2 * * ?]]>"));
    }

    #[test]
    fn applies_every_kind_and_sidecars_are_in_sync() {
        let (_dir, solution) = fixture();
        let mut data = request(Kind::DataShape, "Acme.Orders.Order_DS");
        data.fields = vec!["id:STRING:primary".into()];
        let mut timer = request(Kind::Timer, "Acme.Orders.Refresh_TM");
        timer.run_as = Some("Administrator".into());
        let mut scheduler = request(Kind::Scheduler, "Acme.Orders.Nightly_SC");
        scheduler.run_as = Some("Administrator".into());
        for request in [
            request(Kind::Thing, "Acme.Orders.Manager"),
            request(Kind::ThingTemplate, "Acme.Orders.Manager_TT"),
            request(Kind::ThingShape, "Acme.Orders.Audit_TS"),
            data,
            timer,
            scheduler,
        ] {
            apply(&solution, request);
        }
        let entities = workspace::entities(&solution);
        let data = entities
            .iter()
            .find(|entity| entity.info.name == "Acme.Orders.Order_DS")
            .unwrap();
        assert_eq!(
            datashape::extract(&std::fs::read(&data.path).unwrap()).unwrap(),
            datashape::from_sidecar(
                &std::fs::read_to_string(workspace::fields_path(&solution, data)).unwrap()
            )
            .unwrap()
        );
        let sync = crate::core::commands::sync::execute(
            &solution,
            &crate::core::commands::sync::SyncRequest {
                target: crate::core::commands::sync::SyncTarget {
                    project: None,
                    entities: Vec::new(),
                    all: true,
                    reject_entities_with_all: false,
                    missing_target: "name an entity, or pass --all",
                },
                mode: Mode::Plan,
                allow_structural: false,
                relayout: false,
                lock_label: "test sync",
            },
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(sync.report.changed, 0, "{:?}", sync.report.log.lines);
        assert_eq!(sync.report.failed, 0, "{:?}", sync.report.log.lines);
        let failing: Vec<_> = crate::core::check::run(&solution)
            .gates
            .into_iter()
            .filter(|gate| !gate.ok() && gate.name != "project")
            .collect();
        // This fixture intentionally has no service implementations, which only the project gate reports.
        assert!(failing.is_empty(), "{failing:?}");
    }

    #[test]
    fn durations_render_as_server_numbers_and_bad_values_are_refused() {
        let (_dir, solution) = fixture();
        for (text, expected) in [
            ("500ms", "500.0"),
            ("30s", "30000.0"),
            ("5m", "300000.0"),
            ("1h", "3600000.0"),
            ("2500", "2500.0"),
        ] {
            let mut request = request(Kind::Timer, &format!("Acme.Orders.{expected}_TM"));
            request.every = Some(text.into());
            request.run_as = Some("Administrator".into());
            assert!(
                std::str::from_utf8(&plan(&solution, &request).unwrap().entity)
                    .unwrap()
                    .contains(&format!("<updateRate>{expected}</updateRate>"))
            );
        }
        for value in ["", "5x", "-1s", "0"] {
            assert_eq!(
                parse_duration(value).unwrap_err().code(),
                ErrorCode::InvalidArguments
            );
        }
    }

    #[test]
    fn quartz_cron_accepts_six_or_seven_fields_only() {
        assert!(validate_cron("0 0 2 * * ?").is_ok());
        assert!(validate_cron("0 0 2 * * ? 2027").is_ok());
        for value in ["0 0 2 * *", ""] {
            assert_eq!(
                validate_cron(value).unwrap_err().code(),
                ErrorCode::InvalidArguments
            );
        }
    }

    #[test]
    fn invalid_requests_have_the_promised_codes() {
        let (_dir, solution) = fixture();
        let code = |request| plan(&solution, &request).unwrap_err().code();
        let mut timer = request(Kind::Timer, "Acme.Orders.Timer_TM");
        assert_eq!(code(timer.clone()), ErrorCode::InvalidArguments);
        timer.run_as = Some("Administrator".into());
        timer.then = Some("x".into());
        timer.script = Some("nope.js".into());
        assert_eq!(code(timer.clone()), ErrorCode::InvalidArguments);
        timer.script = None;
        timer.then = Some("var s = \"]]>\";".into());
        assert_eq!(code(timer.clone()), ErrorCode::InvalidArguments);
        timer.then = None;
        timer.run_as = Some("a]]>b".into());
        assert_eq!(code(timer), ErrorCode::InvalidArguments);
        let data = request(Kind::DataShape, "Acme.Orders.Empty_DS");
        assert_eq!(code(data), ErrorCode::InvalidArguments);
        let mut duplicate = request(Kind::DataShape, "Acme.Orders.Duplicate_DS");
        duplicate.fields = vec!["id:STRING".into(), "id:STRING".into()];
        assert_eq!(code(duplicate), ErrorCode::InvalidArguments);
        let mut unknown = request(Kind::Thing, "Acme.Orders.Unknown");
        unknown.template = Some("No.Template".into());
        assert_eq!(code(unknown), ErrorCode::UnknownEntity);
        let mut shape = request(Kind::Thing, "Acme.Orders.UnknownShape");
        shape.shapes.push("No.Shape".into());
        assert_eq!(code(shape), ErrorCode::UnknownEntity);
        let mut base = request(Kind::DataShape, "Acme.Orders.UnknownBase_DS");
        base.fields.push("id:STRING".into());
        base.base = Some("No.Shape".into());
        assert_eq!(code(base), ErrorCode::UnknownEntity);
        assert_eq!(
            code(request(Kind::ThingShape, "bad name!")),
            ErrorCode::InvalidArguments
        );
    }

    #[test]
    fn existing_entity_or_file_is_never_replaced_for_each_kind() {
        let (_dir, solution) = fixture();
        for kind in [
            Kind::Thing,
            Kind::ThingTemplate,
            Kind::ThingShape,
            Kind::DataShape,
            Kind::Timer,
            Kind::Scheduler,
        ] {
            let mut request = request(kind, &format!("Acme.Orders.{}_Existing", kind.word()));
            if kind == Kind::DataShape {
                request.fields.push("id:STRING".into());
            }
            if matches!(kind, Kind::Timer | Kind::Scheduler) {
                request.run_as = Some("Administrator".into());
            }
            let path = solution
                .project_root(solution.project("Acme.Orders").unwrap())
                .join(kind.collection())
                .join(format!("{}.xml", request.name));
            std::fs::write(path, "in the way").unwrap();
            assert_eq!(
                plan(&solution, &request).unwrap_err().code(),
                ErrorCode::AlreadyExists
            );
        }
    }

    #[test]
    fn an_existing_entity_name_is_refused_for_each_kind() {
        let (_dir, solution) = fixture();
        for kind in [
            Kind::Thing,
            Kind::ThingTemplate,
            Kind::ThingShape,
            Kind::DataShape,
            Kind::Timer,
            Kind::Scheduler,
        ] {
            let mut request = request(kind, &format!("Acme.Orders.{}_Created", kind.word()));
            if kind == Kind::DataShape {
                request.fields.push("id:STRING".into());
            }
            if matches!(kind, Kind::Timer | Kind::Scheduler) {
                request.run_as = Some("Administrator".into());
            }
            apply(&solution, request.clone());
            assert_eq!(
                plan(&solution, &request).unwrap_err().code(),
                ErrorCode::AlreadyExists
            );
        }
    }

    #[test]
    fn every_golden_entity_has_each_section_once() {
        // The goldens are blessed from twaco's own output, so check their structure independently:
        // a section written twice (two ConfigurationTables) would lose one on import.
        let sections = [
            "avatar",
            "DesignTimePermissions",
            "RunTimePermissions",
            "VisibilityPermissions",
            "ConfigurationTableDefinitions",
            "ConfigurationTables",
            "ThingShape",
            "ImplementedShapes",
            "FieldDefinitions",
            "PropertyBindings",
            "SharedConfigurationTables",
        ];
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/entity_new/golden");
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let src = std::fs::read(&path).unwrap();
            let tokens = crate::core::scan::tokenize(&src).unwrap();
            let entity = crate::core::sidecar::entity_element(&tokens, &src).unwrap();
            for section in sections {
                let count = crate::core::scan::child_tags(&tokens, &src, section, entity).len();
                assert!(
                    count <= 1,
                    "{} has {count} {section} sections",
                    path.display()
                );
            }
        }
    }
}
