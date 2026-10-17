//! Create the common ThingWorx entities as repository files.
//!
//! The skeletons are the 10.1 exports with volatile server metadata removed.  This module only
//! creates new files; subsequent changes are made by the scanners and sidecar synchronisers.

use crate::core::accumulated::accumulated_json;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{Project, Solution};
use crate::core::datashape::{self, Aspect, Field};
use crate::core::datatable;
use crate::core::lock::WorkspaceLock;
use crate::core::mashup::authoring::parse_visibility;
use crate::core::mashup::templates::Visibility;
use crate::core::refs;
use crate::core::transaction::{Transaction, TransactionError};
use crate::core::types;
use crate::core::workspace;
use base64::Engine;
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
    DataTable,
    Stream,
    ValueStream,
    Media,
    Style,
    State,
    Menu,
    Theme,
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
            "data-table" => Self::DataTable,
            "stream" => Self::Stream,
            "value-stream" => Self::ValueStream,
            "media" => Self::Media,
            "style" => Self::Style,
            "state" => Self::State,
            "menu" => Self::Menu,
            "theme" => Self::Theme,
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
            Self::DataTable => "data-table",
            Self::Stream => "stream",
            Self::ValueStream => "value-stream",
            Self::Media => "media",
            Self::Style => "style",
            Self::State => "state",
            Self::Menu => "menu",
            Self::Theme => "theme",
        }
    }

    pub(crate) fn collection(self) -> &'static str {
        match self {
            Self::Thing
            | Self::Timer
            | Self::Scheduler
            | Self::DataTable
            | Self::Stream
            | Self::ValueStream => "Things",
            Self::ThingTemplate => "ThingTemplates",
            Self::ThingShape => "ThingShapes",
            Self::DataShape => "DataShapes",
            Self::Media => "MediaEntities",
            Self::Style => "StyleDefinitions",
            Self::State => "StateDefinitions",
            Self::Menu => "Menus",
            Self::Theme => "StyleThemes",
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
    pub data_shape: Option<String>,
    pub indexes: Vec<String>,
    pub file: Option<PathBuf>,
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub secondary_background: Option<String>,
    pub line_color: Option<String>,
    pub line_style: Option<String>,
    pub line_thickness: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub text_size: Option<String>,
    pub image: Option<String>,
    pub display_string: Option<String>,
    pub state_type: Option<String>,
    pub states: Vec<String>,
    pub items: Vec<String>,
    pub default: bool,
    pub label: Option<String>,
    pub from: Option<String>,
    pub props: Vec<String>,
    pub(crate) theme_content: Option<String>,
}

impl NewRequest {
    /// A request for `kind` named `name` with nothing else set.
    pub fn blank(kind: Kind, name: &str) -> Self {
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
            data_shape: None,
            indexes: Vec::new(),
            file: None,
            background: None,
            foreground: None,
            secondary_background: None,
            line_color: None,
            line_style: None,
            line_thickness: None,
            bold: false,
            italic: false,
            underline: false,
            text_size: None,
            image: None,
            display_string: None,
            state_type: None,
            states: Vec::new(),
            items: Vec::new(),
            default: false,
            label: None,
            from: None,
            props: Vec::new(),
            theme_content: None,
        }
    }
}

#[derive(Debug)]
pub struct Plan {
    pub request: NewRequest,
    pub project: String,
    pub(crate) entity_path: PathBuf,
    pub(crate) entity: Vec<u8>,
    pub(crate) sidecars: Vec<(PathBuf, Vec<u8>)>,
    pub(crate) duration_ms: Option<u64>,
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

fn entity_file(solution: &Solution, collection: &str, name: &str) -> Option<workspace::EntityFile> {
    workspace::entities(solution)
        .into_iter()
        .find(|entry| entry.info.collection == collection && entry.info.name == name)
}

fn entity_content(path: &Path) -> Result<String, NewError> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| NewError::Invalid(format!("{}: {error}", path.display())))?;
    let start = source
        .find("<content>")
        .ok_or_else(|| NewError::Invalid(format!("{} has no content", path.display())))?
        + "<content>".len();
    let end = source[start..]
        .find("</content>")
        .map(|end| start + end)
        .ok_or_else(|| NewError::Invalid(format!("{} has unclosed content", path.display())))?;
    Ok(source[start..end]
        .trim()
        .strip_prefix("<![CDATA[")
        .and_then(|value| value.strip_suffix("]]>"))
        .unwrap_or(&source[start..end])
        .trim()
        .to_string())
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

pub(crate) fn parse_duration(text: &str) -> Result<u64, NewError> {
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

pub(crate) fn validate_cron(text: &str) -> Result<String, NewError> {
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

/// One `<ImplementedShape>`, as written at 16 spaces.
pub(crate) fn implemented_shape_xml(shape: &str) -> String {
    format!(
        "                <ImplementedShape\n                 name=\"{}\"\n                 type=\"ThingShape\"></ImplementedShape>",
        escape(shape)
    )
}

fn shapes(shapes: &[String]) -> String {
    let mut out = String::from("            <ImplementedShapes>");
    for shape in shapes {
        out.push('\n');
        out.push_str(&implemented_shape_xml(shape));
    }
    out.push_str("\n            </ImplementedShapes>");
    out
}

/// One script `<Subscription>`, as written at 20 spaces. `source` empty is the entity itself.
pub(crate) fn subscription_xml(
    name: &str,
    event: &str,
    source: &str,
    source_property: &str,
    code: &str,
) -> String {
    let (name, event) = (escape(name), escape(event));
    let (source, source_property) = (escape(source), escape(source_property));
    format!("                    <Subscription\n                     description=\"\"\n                     enabled=\"true\"\n                     eventName=\"{event}\"\n                     name=\"{name}\"\n                     source=\"{source}\"\n                     sourceProperty=\"{source_property}\"\n                     sourceType=\"Thing\">\n                        <ServiceImplementation\n                         description=\"\"\n                         handlerName=\"Script\"\n                         name=\"{name}\">\n                            <ConfigurationTables>\n                                <ConfigurationTable\n                                 dataShapeName=\"\"\n                                 description=\"\"\n                                 isMultiRow=\"false\"\n                                 name=\"Script\"\n                                 ordinal=\"0\">\n                                    <DataShape>\n                                        <FieldDefinitions>\n                                            <FieldDefinition\n                                             baseType=\"STRING\"\n                                             description=\"code\"\n                                             name=\"code\"\n                                             ordinal=\"0\"></FieldDefinition>\n                                        </FieldDefinitions>\n                                    </DataShape>\n                                    <Rows>\n                                        <Row>\n                                            <code><![CDATA[{code}]]></code>\n                                        </Row>\n                                    </Rows>\n                                </ConfigurationTable>\n                            </ConfigurationTables>\n                        </ServiceImplementation>\n                    </Subscription>")
}

fn local_shape(subscription: Option<(&str, &str, &str)>) -> String {
    let mut out = String::from("            <ThingShape>\n                <PropertyDefinitions></PropertyDefinitions>\n                <ServiceDefinitions></ServiceDefinitions>\n                <EventDefinitions></EventDefinitions>\n                <ServiceMappings></ServiceMappings>\n                <ServiceImplementations></ServiceImplementations>\n                <Subscriptions>");
    if let Some((event, name, code)) = subscription {
        out.push('\n');
        out.push_str(&subscription_xml(name, event, "", "", code));
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

pub(crate) fn cdata(value: &str) -> String {
    format!("<![CDATA[{value}]]>")
}

fn plain_entity(kind: &str, tag: &str, request: &NewRequest, project: &str, body: &str) -> String {
    format!("{}        <{tag}\n         description=\"{}\"\n         documentationContent=\"\"\n         homeMashup=\"\"\n         name=\"{}\"\n         projectName=\"{project}\"\n         tags=\"\">\n{}\n{body}\n        </{tag}>\n{}", header(kind), escape(&request.description), escape(&request.name), common_tail(&[]), footer(kind))
}

fn data_thing_settings(
    provider: &str,
    data_shape: Option<&str>,
    fields: &[Field],
    indexes: &[(String, String)],
) -> String {
    let settings = data_shape.map(|shape| format!("\n                <ConfigurationTable\n                 dataShapeName=\"\"\n                 description=\"Data Shape Configuration\"\n                 isMultiRow=\"false\"\n                 name=\"Settings\"\n                 ordinal=\"0\">\n                    <DataShape><FieldDefinitions><FieldDefinition\n                     aspect.friendlyName=\"Data Shape\"\n                     baseType=\"DATASHAPENAME\"\n                     description=\"Data shape\"\n                     name=\"dataShape\"\n                     ordinal=\"0\"></FieldDefinition></FieldDefinitions></DataShape>\n                    <Rows><Row><accumulatedDataShape><json>{}</json></accumulatedDataShape><dataShape>{}</dataShape></Row></Rows>\n                </ConfigurationTable>", cdata(&accumulated_json(fields)), cdata(shape))).unwrap_or_default();
    let index_table = if provider == "DataTable" {
        let rows: String = indexes.iter().map(|(name, fields)| format!("\n                        <Row><fieldNames>{}</fieldNames><name>{}</name></Row>", cdata(fields), cdata(name))).collect();
        format!("\n                <ConfigurationTable\n                 dataShapeName=\"\"\n                 description=\"Index Settings\"\n                 isMultiRow=\"true\"\n                 name=\"Indexes\"\n                 ordinal=\"1\">\n                    <DataShape><FieldDefinitions><FieldDefinition\n                     aspect.friendlyName=\"Index Field Names\"\n                     baseType=\"STRING\"\n                     description=\"Index fields\"\n                     name=\"fieldNames\"\n                     ordinal=\"1\"></FieldDefinition><FieldDefinition\n                     aspect.friendlyName=\"Index Name\"\n                     baseType=\"STRING\"\n                     description=\"Index name\"\n                     name=\"name\"\n                     ordinal=\"0\"></FieldDefinition></FieldDefinitions></DataShape>\n                    <Rows>{rows}\n                    </Rows>\n                </ConfigurationTable>")
    } else {
        String::new()
    };
    let performance = match provider {
        "DataTable" => "Data Table",
        "Stream" => "Stream",
        _ => "Value Stream",
    };
    format!("            <ConfigurationTables>\n                <ConfigurationTable\n                 dataShapeName=\"\"\n                 description=\"Data Thing Configuration\"\n                 isMultiRow=\"false\"\n                 name=\"DataThingSettings\"\n                 ordinal=\"0\">\n                    <DataShape><FieldDefinitions><FieldDefinition\n                     baseType=\"STRING\"\n                     description=\"Persistence Provider Name\"\n                     name=\"persistenceProvider\"\n                     ordinal=\"0\"></FieldDefinition></FieldDefinitions></DataShape>\n                    <Rows><Row><persistenceProvider><![CDATA[ThingworxPersistenceProvider]]></persistenceProvider></Row></Rows>\n                </ConfigurationTable>{settings}{index_table}\n                <ConfigurationTable\n                 dataShapeName=\"\"\n                 description=\"Configurable options to tune {performance} performance\"\n                 isMultiRow=\"false\"\n                 name=\"PersistenceProviderCustomSettings\"\n                 ordinal=\"2\">\n                    <DataShape><FieldDefinitions><FieldDefinition\n                     aspect.friendlyName=\"Persistence Provider Custom Settings Table\"\n                     baseType=\"INFOTABLE\"\n                     description=\"Persistence Provider Custom Config Table\"\n                     name=\"customConfigTable\"\n                     ordinal=\"0\"></FieldDefinition></FieldDefinitions></DataShape>\n                    <Rows><Row><customConfigTable><infoTable><DataShape><FieldDefinitions></FieldDefinitions></DataShape><Rows></Rows></infoTable></customConfigTable></Row></Rows>\n                </ConfigurationTable>\n            </ConfigurationTables>")
}

fn shape_fields(solution: &Solution, shape: &str) -> Result<Vec<Field>, NewError> {
    let file = entity_file(solution, "DataShapes", shape)
        .ok_or_else(|| NewError::Unknown(format!("no DataShape {shape} in the solution")))?;
    let src = std::fs::read(&file.path)
        .map_err(|error| NewError::Invalid(format!("{}: {error}", file.path.display())))?;
    datashape::extract(&src).map_err(|error| NewError::Invalid(error.to_string()))
}

fn data_thing(request: &NewRequest, project: &str, template: &str, settings: String) -> String {
    format!("{}        <Thing\n         aspect.isSystemObject=\"false\"\n         description=\"{}\"\n         documentationContent=\"\"\n         effectiveThingPackage=\"{}Thing\"\n         enabled=\"true\"\n         homeMashup=\"\"\n         identifier=\"\"\n         inheritedValueStream=\"\"\n         name=\"{}\"\n         projectName=\"{project}\"\n         published=\"false\"\n         tags=\"\"\n         thingTemplate=\"{template}\"\n         valueStream=\"\">\n{}\n{settings}\n{}\n            <PropertyBindings></PropertyBindings>\n            <RemotePropertyBindings></RemotePropertyBindings>\n            <RemoteServiceBindings></RemoteServiceBindings>\n            <RemoteEventBindings></RemoteEventBindings>\n            <AlertConfigurations></AlertConfigurations>\n            <ImplementedShapes></ImplementedShapes>\n            <ThingProperties></ThingProperties>\n        </Thing>\n{}", header("Things"), escape(&request.description), template, escape(&request.name), common_tail(&[]).strip_suffix("\n            <ConfigurationTables></ConfigurationTables>").unwrap(), local_shape(None), footer("Things"))
}

fn media_xml(request: &NewRequest, project: &str) -> String {
    let tables = "            <ConfigurationTables>\n                <ConfigurationTable dataShapeName=\"\" description=\"Use Existing Flow Configuration\" isMultiRow=\"false\" name=\"UseExistingFlowConfiguration\" ordinal=\"0\"><DataShape><FieldDefinitions><FieldDefinition aspect.friendlyName=\"Connector Name\" baseType=\"STRING\" description=\"Connector Name\" name=\"connectorName\" ordinal=\"0\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"Ignore SSL Errors\" baseType=\"BOOLEAN\" description=\"Ignore SSL errors\" name=\"ignoreSSLErrors\" ordinal=\"1\"></FieldDefinition></FieldDefinitions></DataShape><Rows><Row><connectorName></connectorName><ignoreSSLErrors>false</ignoreSSLErrors></Row></Rows></ConfigurationTable>\n                <ConfigurationTable dataShapeName=\"\" description=\"Content Source Connection information\" isMultiRow=\"false\" name=\"ContentSourceConnectionInformation\" ordinal=\"1\"><DataShape><FieldDefinitions><FieldDefinition aspect.defaultValue=\"none\" aspect.friendlyName=\"AuthenticationType\" aspect.selectOptions=\"none:none|basic:basic|SSO:SSO|OAuthM2M:OAuthM2M|SessionUser:SessionUser\" baseType=\"STRING\" description=\"Authentication type\" name=\"authenticationType\" ordinal=\"4\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"AuthorizationServerID\" baseType=\"STRING\" description=\"Authorization Server ID\" name=\"authorizationServerId\" ordinal=\"7\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"Dynamic Content\" baseType=\"BOOLEAN\" description=\"Display dynamic content from external URL\" name=\"dynamicContent\" ordinal=\"0\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"Headers To Forward on POSTs\" baseType=\"STRING\" description=\"Headers to forward on dynamic content using POST\" name=\"HeadersToForward\" ordinal=\"9\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"Ignore SSL Errors\" baseType=\"BOOLEAN\" description=\"Ignore SSL errors\" name=\"ignoreSSLErrors\" ordinal=\"2\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"DoNotCache-CacheControl-ResponseHeader\" baseType=\"BOOLEAN\" description=\"enable No-Cache Header to initiate a clean load dynamic content\" name=\"noCacheHeaderCheckbox\" ordinal=\"10\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"AuthPassword\" baseType=\"PASSWORD\" description=\"Password for basic authentication\" name=\"password\" ordinal=\"6\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"TestConnectionURL\" baseType=\"STRING\" description=\"The URL to test the connection\" name=\"testUrl\" ordinal=\"8\"></FieldDefinition><FieldDefinition aspect.defaultValue=\"60.0\" aspect.friendlyName=\"Timeout\" baseType=\"NUMBER\" description=\"Timeout\" name=\"timeout\" ordinal=\"3\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"URL\" baseType=\"STRING\" description=\"The URL to the content source\" name=\"url\" ordinal=\"1\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"AuthUsername\" baseType=\"STRING\" description=\"Username for basic authentication\" name=\"userId\" ordinal=\"5\"></FieldDefinition></FieldDefinitions></DataShape><Rows><Row><HeadersToForward></HeadersToForward><authenticationType><![CDATA[none]]></authenticationType><authorizationServerId></authorizationServerId><dynamicContent>false</dynamicContent><ignoreSSLErrors>false</ignoreSSLErrors><noCacheHeaderCheckbox>false</noCacheHeaderCheckbox><password></password><testUrl></testUrl><timeout>60.0</timeout><url></url><userId></userId></Row></Rows></ConfigurationTable>\n                <ConfigurationTable dataShapeName=\"\" description=\"List of authorization server scopes\" isMultiRow=\"true\" name=\"AuthorizationServerScopesSettings\" ordinal=\"2\"><DataShape><FieldDefinitions><FieldDefinition baseType=\"STRING\" description=\"A single scope for the authorization server.\" name=\"scope\" ordinal=\"0\"></FieldDefinition></FieldDefinitions></DataShape><Rows></Rows></ConfigurationTable>\n                <ConfigurationTable dataShapeName=\"\" description=\"SSL Connection Configuration\" isMultiRow=\"false\" name=\"SSLConnectionConfiguration\" ordinal=\"3\"><DataShape><FieldDefinitions><FieldDefinition aspect.friendlyName=\"SSLKeyStorePassword\" baseType=\"PASSWORD\" description=\"SSL Keystore Password\" name=\"SSLKeyStorePassword\" ordinal=\"2\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"SSLKeyStorePath\" baseType=\"STRING\" description=\"Absolute path to the client SSL keystore\" name=\"SSLKeyStorePath\" ordinal=\"1\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"SSLTrustStorePassword\" baseType=\"PASSWORD\" description=\"SSL Truststore Password\" name=\"SSLTrustStorePassword\" ordinal=\"4\"></FieldDefinition><FieldDefinition aspect.friendlyName=\"SSLTrustStorePath\" baseType=\"STRING\" description=\"Absolute path to the SSL client truststore\" name=\"SSLTrustStorePath\" ordinal=\"3\"></FieldDefinition></FieldDefinitions></DataShape><Rows><Row><SSLKeyStorePassword></SSLKeyStorePassword><SSLKeyStorePath></SSLKeyStorePath><SSLTrustStorePassword></SSLTrustStorePassword><SSLTrustStorePath></SSLTrustStorePath></Row></Rows></ConfigurationTable>\n                <ConfigurationTable dataShapeName=\"\" description=\"Session User Configuration\" isMultiRow=\"false\" name=\"SessionUserConfigurationTable\" ordinal=\"4\"><DataShape><FieldDefinitions><FieldDefinition aspect.friendlyName=\"SessionUserQueryParameter\" baseType=\"STRING\" description=\"Session User Query Parameter\" name=\"SessionUserQueryParameter\" ordinal=\"1\"></FieldDefinition></FieldDefinitions></DataShape><Rows><Row><SessionUserQueryParameter></SessionUserQueryParameter></Row></Rows></ConfigurationTable>\n            </ConfigurationTables>";
    let common_tail = common_tail(&[]);
    let common = common_tail
        .strip_suffix("\n            <ConfigurationTables></ConfigurationTables>")
        .unwrap();
    format!("{}        <MediaEntity description=\"{}\" documentationContent=\"\" homeMashup=\"\" name=\"{}\" projectName=\"{project}\" tags=\"\">\n{common}\n{tables}\n            <content>{}</content>\n        </MediaEntity>\n{}", header("MediaEntities"), escape(&request.description), escape(&request.name), cdata(&media_content(request.file.as_ref().expect("validated media")).expect("validated media")), footer("MediaEntities"))
}

fn style_content(request: &NewRequest) -> String {
    jackson_pretty(&style_value(request))
}

fn style_value(request: &NewRequest) -> Value {
    let value = |option: &Option<String>| {
        option
            .as_deref()
            .map(|item| color(item).expect("validated color"))
            .unwrap_or_default()
    };
    json!({
        "backgroundColor": value(&request.background),
        "displayString": request.display_string.as_deref().unwrap_or(""),
        "fontEmphasisBold": request.bold,
        "fontEmphasisItalic": request.italic,
        "fontEmphasisUnderline": request.underline,
        "foregroundColor": value(&request.foreground),
        "image": request.image.as_deref().unwrap_or(""),
        "lineColor": value(&request.line_color),
        "lineStyle": request.line_style.as_deref().unwrap_or("solid"),
        "lineThickness": serde_json::from_str::<Value>(
            request.line_thickness.as_deref().unwrap_or("1"),
        )
        .expect("validated line thickness"),
        "secondaryBackgroundColor": value(&request.secondary_background),
        "textSize": request.text_size.as_deref().unwrap_or("normal"),
    })
}

/// Render JSON as the layout written by ThingWorx's Jackson exporter.
///
/// `serde_json` preserves object insertion order here, retaining a source theme's custom keys.
pub(crate) fn jackson_pretty(value: &Value) -> String {
    fn write(value: &Value, level: usize, out: &mut String) {
        match value {
            Value::Object(object) => {
                out.push('{');
                if !object.is_empty() {
                    out.push('\n');
                    for (index, (key, value)) in object.iter().enumerate() {
                        out.push_str(&"  ".repeat(level + 1));
                        out.push_str(&serde_json::to_string(key).unwrap());
                        out.push_str(" : ");
                        write(value, level + 1, out);
                        if index + 1 != object.len() {
                            out.push(',');
                        }
                        out.push('\n');
                    }
                    out.push_str(&"  ".repeat(level));
                }
                out.push('}');
            }
            Value::Array(array) if array.is_empty() => out.push_str("[ ]"),
            Value::Array(array) => {
                out.push_str("[ ");
                for (index, value) in array.iter().enumerate() {
                    write(value, level, out);
                    if index + 1 != array.len() {
                        out.push_str(", ");
                    }
                }
                out.push_str(" ]");
            }
            value => out.push_str(&serde_json::to_string(value).unwrap()),
        }
    }

    let mut out = String::new();
    write(value, 0, &mut out);
    out
}

pub(crate) fn color(value: &str) -> Result<String, NewError> {
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(format!(
                "rgba({},{},{},1)",
                u8::from_str_radix(&hex[0..2], 16).unwrap(),
                u8::from_str_radix(&hex[2..4], 16).unwrap(),
                u8::from_str_radix(&hex[4..6], 16).unwrap()
            ));
        }
    }
    let parts: Vec<_> = value
        .strip_prefix("rgba(")
        .and_then(|v| v.strip_suffix(')'))
        .map(|v| v.split(',').collect())
        .unwrap_or_default();
    if parts.len() == 4 && parts.iter().all(|v| v.trim().parse::<f64>().is_ok()) {
        return Ok(value.to_string());
    }
    Err(NewError::Invalid(format!(
        "{value:?} is not a #rrggbb or rgba(r,g,b,a) color"
    )))
}

pub(crate) fn media_content(path: &Path) -> Result<String, NewError> {
    let bytes = std::fs::read(path)
        .map_err(|error| NewError::Invalid(format!("{}: {error}", path.display())))?;
    if bytes.len() > 10 * 1024 * 1024 {
        return Err(NewError::Invalid(
            "--file is over the 10 MB media limit".into(),
        ));
    }
    let svg = bytes
        .iter()
        .skip_while(|b| b.is_ascii_whitespace())
        .copied()
        .collect::<Vec<_>>()
        .starts_with(b"<svg");
    let image = bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || svg;
    if !image {
        return Err(NewError::Invalid(
            "--file must contain a PNG, JPEG, GIF, or SVG image".into(),
        ));
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The style flags of `request`: colours, line style and thickness, text size and image.
pub(crate) fn validate_style(solution: &Solution, request: &NewRequest) -> Result<(), NewError> {
    for value in [
        &request.background,
        &request.foreground,
        &request.secondary_background,
        &request.line_color,
    ]
    .into_iter()
    .flatten()
    {
        color(value)?;
    }
    if let Some(value) = &request.line_style {
        if !["solid", "dashed", "dotted", "none"].contains(&value.as_str()) {
            return Err(NewError::Invalid(
                "--line-style must be solid, dashed, dotted, or none".into(),
            ));
        }
    }
    if let Some(value) = &request.line_thickness {
        value
            .parse::<f64>()
            .ok()
            .filter(|value| *value >= 0.0)
            .ok_or_else(|| {
                NewError::Invalid("--line-thickness must be a non-negative number".into())
            })?;
    }
    if let Some(value) = &request.text_size {
        if !["xsmall", "small", "normal", "large", "xlarge", "xxlarge"].contains(&value.as_str()) {
            return Err(NewError::Invalid(
                "--text-size must be xsmall, small, normal, large, xlarge, or xxlarge".into(),
            ));
        }
    }
    if let Some(image) = &request.image {
        if !entity_exists(solution, "MediaEntities", image) {
            return Err(NewError::Unknown(format!(
                "no MediaEntity {image} in the solution or platform cache"
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_state_request(request: &NewRequest) -> Result<(), NewError> {
    let state_type = request
        .state_type
        .as_deref()
        .ok_or_else(|| NewError::Invalid("state needs --type numeric|string".into()))?;
    if !["numeric", "string"].contains(&state_type) {
        return Err(NewError::Invalid("--type must be numeric or string".into()));
    }
    if request.states.is_empty() {
        return Err(NewError::Invalid(
            "a state definition needs one or more --state".into(),
        ));
    }
    let mut catch_all = false;
    for spec in &request.states {
        let head = spec.split(';').next().unwrap();
        let comparator = ["<=", ">=", "==", "<", ">", "*"]
            .into_iter()
            .find(|prefix| head.starts_with(prefix))
            .ok_or_else(|| NewError::Invalid(format!("--state {spec:?} has no comparator")))?;
        if state_type == "string" && !["==", "*"].contains(&comparator) {
            return Err(NewError::Invalid(format!(
                "{comparator} is not allowed for string states"
            )));
        }
        if comparator == "*" {
            if catch_all {
                return Err(NewError::Invalid(
                    "a state definition has more than one catch-all".into(),
                ));
            }
            catch_all = true;
            if !head.starts_with("*=") {
                return Err(NewError::Invalid(
                    "a catch-all state must be *=label".into(),
                ));
            }
        } else {
            let (value, label) = head[comparator.len()..]
                .split_once('=')
                .ok_or_else(|| NewError::Invalid(format!("--state {spec:?} must have =label")))?;
            if value.is_empty() || label.is_empty() {
                return Err(NewError::Invalid(format!(
                    "--state {spec:?} must have a value and label"
                )));
            }
            if state_type == "numeric" && value.parse::<f64>().is_err() {
                return Err(NewError::Invalid(format!(
                    "numeric state value {value:?} is not a number"
                )));
            }
        }
        for style in spec.split(';').skip(1) {
            let (key, value) = style.split_once(':').ok_or_else(|| {
                NewError::Invalid(format!("state style {style:?} must be key:value"))
            })?;
            match key {
                "background" | "foreground" | "secondary-background" | "line-color" => {
                    color(value)?;
                }
                "line-style" => {
                    if !["solid", "dashed", "dotted", "none"].contains(&value) {
                        return Err(NewError::Invalid(
                            "state line-style must be solid, dashed, dotted, or none".into(),
                        ));
                    }
                }
                "line-thickness" => {
                    value.parse::<f64>().map_err(|_| {
                        NewError::Invalid("state line-thickness must be a number".into())
                    })?;
                }
                "bold" | "italic" | "underline" => {
                    if !["true", "false"].contains(&value) {
                        return Err(NewError::Invalid(format!(
                            "state {key} must be true or false"
                        )));
                    }
                }
                "text-size" => {
                    if !["xsmall", "small", "normal", "large", "xlarge", "xxlarge"].contains(&value)
                    {
                        return Err(NewError::Invalid("state text-size is not known".into()));
                    }
                }
                "image" => {}
                _ => return Err(NewError::Invalid(format!("unknown state style {key}"))),
            }
        }
    }
    Ok(())
}

fn state_comparator(head: &str) -> (&str, &str) {
    for comparator in ["<=", ">=", "==", "<", ">"] {
        if let Some(rest) = head.strip_prefix(comparator) {
            return (comparator, rest);
        }
    }
    ("*", "")
}

fn apply_state_style(style: &mut NewRequest, key: &str, value: &str) {
    match key {
        "background" => style.background = Some(value.into()),
        "foreground" => style.foreground = Some(value.into()),
        "secondary-background" => style.secondary_background = Some(value.into()),
        "line-color" => style.line_color = Some(value.into()),
        "line-style" => style.line_style = Some(value.into()),
        "line-thickness" => style.line_thickness = Some(value.into()),
        "bold" => style.bold = value == "true",
        "italic" => style.italic = value == "true",
        "underline" => style.underline = value == "true",
        "text-size" => style.text_size = Some(value.into()),
        "image" => style.image = Some(value.into()),
        _ => unreachable!("state styles are validated before rendering"),
    }
}

pub(crate) fn render_state_content(request: &NewRequest) -> String {
    let definitions: Vec<Value> = request
        .states
        .iter()
        .map(|spec| {
            let (head, styles) = spec.split_once(';').unwrap_or((spec, ""));
            let (comparator, rest) = state_comparator(head);
            let (value, label) = if comparator == "*" {
                (
                    "",
                    head.strip_prefix("*=").expect("validated catch-all state"),
                )
            } else {
                rest.split_once('=').expect("validated state")
            };
            let mut style = request.clone();
            for item in styles.split(';').filter(|item| !item.is_empty()) {
                let (key, value) = item.split_once(':').expect("validated state style");
                apply_state_style(&mut style, key, value);
            }
            let mut definition = serde_json::Map::new();
            if request.state_type.as_deref() == Some("numeric") && comparator != "*" {
                definition.insert("comparator".into(), Value::String(comparator.into()));
            }
            definition.insert("defaultStyleDefinition".into(), style_value(&style));
            definition.insert("defaultValue".into(), Value::String(value.into()));
            definition.insert("description".into(), Value::String(String::new()));
            definition.insert("displayString".into(), Value::String(label.into()));
            definition.insert("name".into(), Value::String(label.into()));
            Value::Object(definition)
        })
        .collect();
    jackson_pretty(&json!({
        "stateDefinitions": definitions,
        "stateType": request.state_type.as_deref().unwrap(),
    }))
}

/// One `<MenuItem>`, preceded by its newline, as written at 16 spaces.
pub(crate) fn menu_item_xml(title: &str, target: &str, is_default: bool) -> String {
    let (target, title) = (escape(target), escape(title));
    format!("\n                <MenuItem description=\"\" imageURL=\"\" isDefault=\"{is_default}\" linkDestination=\"{target}\" linkTarget=\"Self\" linkType=\"Mashup\" title=\"{title}\">\n                    <GroupReferences><Members></Members></GroupReferences>\n                </MenuItem>")
}

/// One effectiveMenu `<Row>` for item `index` of `menu`, preceded by its newline, at 20 spaces.
/// What an effectiveMenu row repeats of its menu item.
pub(crate) struct MenuRow<'a> {
    pub(crate) title: &'a str,
    pub(crate) target: &'a str,
    pub(crate) is_default: bool,
    pub(crate) description: &'a str,
    pub(crate) image_url: &'a str,
    pub(crate) link_target: &'a str,
}

pub(crate) fn menu_row_xml(menu: &str, index: usize, row: &MenuRow<'_>) -> String {
    // An empty string is an empty element; any other is CDATA, as exports write them.
    let text = |value: &str| {
        if value.is_empty() {
            String::new()
        } else {
            cdata(value)
        }
    };
    let (target, title) = (cdata(row.target), cdata(row.title));
    let (description, image) = (text(row.description), text(row.image_url));
    let (is_default, link_target) = (row.is_default, cdata(row.link_target));
    format!("\n                    <Row>\n                        <description>{description}</description>\n                        <imageURL>{image}</imageURL>\n                        <isDefault>{is_default}</isDefault>\n                        <linkDestination>{target}</linkDestination>\n                        <linkTarget>{link_target}</linkTarget>\n                        <linkType><![CDATA[Mashup]]></linkType>\n                        <menuId><![CDATA[{menu}_{index}]]></menuId>\n                        <parentMenuId><![CDATA[{menu}]]></parentMenuId>\n                        <title>{title}</title>\n                    </Row>")
}

fn menu_xml(request: &NewRequest, project: &str) -> String {
    let label = request.label.as_deref().unwrap_or("");
    let root_title = if label.is_empty() {
        String::new()
    } else {
        cdata(label)
    };
    let items: String = request
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let (title, target) = item.split_once('=').expect("validated menu item");
            menu_item_xml(title, target, request.default && index == 0)
        })
        .collect();
    let rows: String = request
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let (title, target) = item.split_once('=').expect("validated menu item");
            let row = MenuRow {
                title,
                target,
                is_default: request.default && index == 0,
                description: "",
                image_url: "",
                link_target: "Self",
            };
            menu_row_xml(&request.name, index, &row)
        })
        .collect();
    format!("{}        <Menu\n         description=\"{}\"\n         documentationContent=\"\"\n         homeMashup=\"\"\n         imageURL=\"\"\n         menuLabel=\"{}\"\n         name=\"{}\"\n         projectName=\"{project}\"\n         tags=\"\">\n{}\n            <MenuItems>{items}\n            </MenuItems>\n            <GroupReferences><Members></Members></GroupReferences>\n            <effectiveMenu>\n                <DataShape>\n                    <FieldDefinitions>\n                        <FieldDefinition baseType=\"STRING\" description=\"Description for tooltip\" name=\"description\" ordinal=\"0\"></FieldDefinition>\n                        <FieldDefinition baseType=\"IMAGELINK\" description=\"Optional image URL\" name=\"imageURL\" ordinal=\"0\"></FieldDefinition>\n                        <FieldDefinition aspect.defaultValue=\"false\" baseType=\"BOOLEAN\" description=\"Is a default menu item\" name=\"isDefault\" ordinal=\"0\"></FieldDefinition>
                        <FieldDefinition aspect.defaultValue=\"false\" baseType=\"BOOLEAN\" description=\"Is disabled\" name=\"isDisabled\" ordinal=\"0\"></FieldDefinition>
                        <FieldDefinition aspect.defaultValue=\"false\" baseType=\"BOOLEAN\" description=\"Is hidden\" name=\"isHidden\" ordinal=\"0\"></FieldDefinition>\n                        <FieldDefinition aspect.isPrimaryKey=\"true\" baseType=\"STRING\" description=\"Link destination\" name=\"linkDestination\" ordinal=\"0\"></FieldDefinition>\n                        <FieldDefinition baseType=\"STRING\" description=\"Link target (Popup, Mashup, New)\" name=\"linkTarget\" ordinal=\"0\"></FieldDefinition>\n                        <FieldDefinition aspect.isPrimaryKey=\"true\" baseType=\"STRING\" description=\"Link type (Mashup, Hyperlink, Logout, or Menu)\" name=\"linkType\" ordinal=\"0\"></FieldDefinition>\n                        <FieldDefinition baseType=\"STRING\" description=\"Menu entry id\" name=\"menuId\" ordinal=\"0\"></FieldDefinition>\n                        <FieldDefinition baseType=\"STRING\" description=\"Menu entry parent id\" name=\"parentMenuId\" ordinal=\"0\"></FieldDefinition>\n                        <FieldDefinition baseType=\"STRING\" description=\"Menu entry title\" name=\"title\" ordinal=\"0\"></FieldDefinition>
                        <FieldDefinition baseType=\"STRING\" description=\"An optional field that specifies the tooltip text for the menu item\" name=\"tooltip\" ordinal=\"0\"></FieldDefinition>
                        <FieldDefinition baseType=\"IMAGELINK\" description=\"An optional field that specifies the tooltip icon for the menu item\" name=\"tooltipIcon\" ordinal=\"0\"></FieldDefinition>\n                    </FieldDefinitions>\n                </DataShape>\n                <Rows>\n                    <Row>\n                        <description></description>\n                        <imageURL></imageURL>\n                        <linkPriority>0.0</linkPriority>\n                        <linkType><![CDATA[Menu]]></linkType>\n                        <menuId><![CDATA[{}]]></menuId>\n                        <parentMenuId></parentMenuId>\n                        <title>{}</title>\n                    </Row>{rows}\n                </Rows>\n            </effectiveMenu>\n        </Menu>\n{}", header("Menus"), escape(&request.description), escape(label), escape(&request.name), common_tail(&[]), request.name, root_title, footer("Menus"))
}

fn theme_content(request: &NewRequest) -> String {
    let content = request
        .theme_content
        .clone()
        .unwrap_or_else(|| "{\"props\": [], \"version\": \"0.1\"}".into());
    let mut value: Value = serde_json::from_str(&content).expect("validated theme JSON");
    let props = value["props"].as_array_mut().expect("theme props");
    for prop in &request.props {
        let (name, value) = prop.split_once('=').expect("validated theme property");
        if let Some(existing) = props
            .iter_mut()
            .find(|item| item.get("name").and_then(Value::as_str) == Some(name))
        {
            existing["value"] = Value::String(value.into());
        } else {
            props.push(json!({"name": name, "value": value}));
        }
    }
    jackson_pretty(&value)
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
        Kind::DataTable => data_thing(
            request,
            project,
            "DataTable",
            data_thing_settings(
                "DataTable",
                request.data_shape.as_deref(),
                fields,
                &request
                    .indexes
                    .iter()
                    .map(|value| {
                        let (name, fields) = value.split_once('=').expect("validated index");
                        (name.to_string(), fields.to_string())
                    })
                    .collect::<Vec<_>>(),
            ),
        ),
        Kind::Stream => data_thing(
            request,
            project,
            "Stream",
            data_thing_settings("Stream", request.data_shape.as_deref(), fields, &[]),
        ),
        Kind::ValueStream => data_thing(
            request,
            project,
            "ValueStream",
            data_thing_settings("ValueStream", None, &[], &[]),
        ),
        Kind::Media => media_xml(request, project),
        Kind::Style => plain_entity(
            "StyleDefinitions",
            "StyleDefinition",
            request,
            project,
            &format!("            <content>{}</content>", cdata(&style_content(request))),
        ),
        Kind::State => plain_entity(
            "StateDefinitions",
            "StateDefinition",
            request,
            project,
            &format!("            <content>{}</content>", cdata(&render_state_content(request))),
        ),
        Kind::Menu => menu_xml(request, project),
        Kind::Theme => plain_entity(
            "StyleThemes",
            "StyleTheme",
            request,
            project,
            &format!("            <content>{}</content>\n            <preview></preview>", cdata(&theme_content(request))),
        ),
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

pub(crate) fn plan(solution: &Solution, request: &NewRequest) -> Result<Plan, NewError> {
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
    if matches!(request.kind, Kind::DataTable | Kind::Stream) {
        let shape = request
            .data_shape
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                NewError::Invalid(format!("{} needs --data-shape", request.kind.word()))
            })?;
        let shape_file = entity_file(solution, "DataShapes", shape)
            .ok_or_else(|| NewError::Unknown(format!("no DataShape {shape} in the solution")))?;
        let fields = datashape::extract(&std::fs::read(&shape_file.path).map_err(|error| {
            NewError::Invalid(format!("{}: {error}", shape_file.path.display()))
        })?)
        .map_err(|error| NewError::Invalid(error.to_string()))?;
        let names: BTreeSet<_> = fields.iter().map(|field| field.name.as_str()).collect();
        let mut index_names = BTreeSet::new();
        for index in &request.indexes {
            let (name, values) = index
                .split_once('=')
                .filter(|(name, values)| !name.is_empty() && !values.is_empty())
                .ok_or_else(|| {
                    NewError::Invalid(format!("--index {index:?} must be name=field1,field2"))
                })?;
            if !index_names.insert(name) {
                return Err(NewError::Invalid(format!(
                    "index {name} is listed more than once"
                )));
            }
            for field in values.split(',') {
                if !names.contains(field) {
                    return Err(NewError::Unknown(format!(
                        "DataShape {shape} has no field {field} for index {name}"
                    )));
                }
            }
        }
    } else if !request.indexes.is_empty() {
        return Err(NewError::Invalid("--index is only for data-table".into()));
    }
    if request.kind == Kind::Media {
        let path = request
            .file
            .as_deref()
            .ok_or_else(|| NewError::Invalid("media needs --file".into()))?;
        media_content(path)?;
    }
    if matches!(request.kind, Kind::Style | Kind::State) {
        validate_style(solution, request)?;
    }
    if request.kind == Kind::State {
        validate_state_request(request)?;
    }
    if request.kind == Kind::Menu && request.items.is_empty() {
        return Err(NewError::Invalid(
            "menu needs at least one --item Title=Mashup".into(),
        ));
    }
    for item in &request.items {
        if item
            .split_once('=')
            .is_none_or(|(title, target)| title.is_empty() || target.is_empty())
        {
            return Err(NewError::Invalid(format!(
                "--item {item:?} must be Title=Mashup"
            )));
        }
    }
    if request.kind == Kind::Theme {
        for prop in &request.props {
            if prop.split_once('=').is_none_or(|(name, _)| name.is_empty()) {
                return Err(NewError::Invalid(format!(
                    "--prop {prop:?} must be name=value"
                )));
            }
        }
    }
    let fields = match request.kind {
        Kind::DataShape => parse_fields(&request.fields)?,
        // A data table or stream carries its DataShape's own fields; the server leaves out
        // inherited ones, and so does its primary-key check.
        Kind::DataTable | Kind::Stream => {
            let shape = request.data_shape.as_deref().unwrap_or("");
            let fields = shape_fields(solution, shape)?;
            if request.kind == Kind::DataTable
                && !fields
                    .iter()
                    .any(|field| field.aspects.get("isPrimaryKey") == Some(&Aspect::Bool(true)))
            {
                return Err(NewError::Invalid(format!(
                    "DataShape {shape} has no primary key of its own, which a data table needs (an inherited one does not count)"
                )));
            }
            fields
        }
        _ => Vec::new(),
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
    let mut rendered = request.clone();
    if request.kind == Kind::Theme {
        if let Some(from) = &request.from {
            let source = entity_file(solution, "StyleThemes", from).ok_or_else(|| {
                NewError::Unknown(format!("no StyleTheme {from} in the solution"))
            })?;
            rendered.theme_content = Some(entity_content(&source.path)?);
            serde_json::from_str::<Value>(rendered.theme_content.as_deref().unwrap()).map_err(
                |error| {
                    NewError::Invalid(format!("StyleTheme {from} content is not JSON: {error}"))
                },
            )?;
        }
    }
    for value in [
        rendered.run_as.as_ref(),
        script.as_ref(),
        rendered.data_shape.as_ref(),
        rendered.display_string.as_ref(),
        rendered.label.as_ref(),
        rendered.theme_content.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.contains("]]>") {
            return Err(NewError::Invalid(
                "CDATA-bound values cannot contain `]]>`".into(),
            ));
        }
    }
    for value in rendered
        .states
        .iter()
        .chain(rendered.items.iter())
        .chain(rendered.props.iter())
    {
        if value.contains("]]>") {
            return Err(NewError::Invalid(
                "CDATA-bound values cannot contain `]]>`".into(),
            ));
        }
    }
    let entity = make_xml(
        &rendered,
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
    } else if request.kind == Kind::DataTable {
        vec![(
            solution
                .src_root()
                .join(&request.name)
                .join("datatable.json"),
            datatable::to_sidecar(
                &datatable::extract(&entity)
                    .map_err(|error| NewError::Invalid(error.to_string()))?,
            )
            .map_err(|error| NewError::Invalid(error.to_string()))?
            .into_bytes(),
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

pub(crate) fn apply(
    solution: &Solution,
    plan: &Plan,
    lock: &WorkspaceLock,
) -> Result<(), NewError> {
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
    let relative = |path: &Path| solution.relative(path);
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
            data_shape: None,
            indexes: Vec::new(),
            file: None,
            background: None,
            foreground: None,
            secondary_background: None,
            line_color: None,
            line_style: None,
            line_thickness: None,
            bold: false,
            italic: false,
            underline: false,
            text_size: None,
            image: None,
            display_string: None,
            state_type: None,
            states: Vec::new(),
            items: Vec::new(),
            default: false,
            label: None,
            from: None,
            props: Vec::new(),
            theme_content: None,
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
            .join("src/core/entity/new/golden")
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
    fn new_kind_plans_have_101_export_goldens() {
        let (_dir, solution) = fixture();
        std::fs::write(solution.root.join("app/DataShapes/Acme.Orders.Row_DS.xml"), "<Entities><DataShapes><DataShape name=\"Acme.Orders.Row_DS\"><FieldDefinitions><FieldDefinition aspect.isPrimaryKey=\"true\" name=\"id\" baseType=\"STRING\"/></FieldDefinitions></DataShape></DataShapes></Entities>").unwrap();
        let image = solution.root.join("pixel.png");
        std::fs::write(
            &image,
            base64::engine::general_purpose::STANDARD
                .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNg+A8AAAEBAQBcDQotAAAAAElFTkSuQmCC")
                .unwrap(),
        )
        .unwrap();
        let encoded = media_content(&image).unwrap();
        assert!(base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n"));
        let mut table = request(Kind::DataTable, "Acme.Orders.Rows_DT");
        table.data_shape = Some("Acme.Orders.Row_DS".into());
        table.indexes.push("byId=id".into());
        let mut stream = request(Kind::Stream, "Acme.Orders.Events_ST");
        stream.data_shape = Some("Acme.Orders.Row_DS".into());
        let value_stream = request(Kind::ValueStream, "Acme.Orders.Values_VS");
        let mut media = request(Kind::Media, "Acme.Orders.Pixel_MD");
        media.file = Some(image);
        let mut style = request(Kind::Style, "Acme.Orders.Good_SD");
        style.background = Some("#66cc33".into());
        style.foreground = Some("#000000".into());
        style.bold = true;
        let mut state = request(Kind::State, "Acme.Orders.Speed_SD");
        state.state_type = Some("numeric".into());
        state.states = vec![
            "<2000=Normal;background:#66cc33".into(),
            "<3000=Fast;background:#f9ef6b".into(),
            "*=Overload;background:#ff0000".into(),
        ];
        let mut menu = request(Kind::Menu, "Acme.Orders.Main_MN");
        menu.items = vec![
            "Home=Acme.Orders.Home_MU".into(),
            "Orders=Acme.Orders.Orders_MU".into(),
        ];
        menu.default = true;
        let mut theme = request(Kind::Theme, "Acme.Orders.Theme_TH");
        theme
            .props
            .push("e-button-primary-fontcolor=#ffffff".into());
        for (name, request) in [
            ("data-table", table),
            ("stream", stream),
            ("value-stream", value_stream),
            ("media", media),
            ("style", style),
            ("state", state),
            ("menu", menu),
            ("theme", theme),
        ] {
            assert_golden(name, &plan(&solution, &request).unwrap().entity);
        }
    }

    #[test]
    fn state_and_theme_content_use_the_exporter_layout() {
        let mut state = request(Kind::State, "Acme.Orders.Status_SD");
        state.state_type = Some("string".into());
        state.states = vec!["==Open=Open".into(), "*=Other".into()];
        let content = render_state_content(&state);
        let definitions = serde_json::from_str::<Value>(&content).unwrap();
        assert_eq!(definitions["stateDefinitions"][0]["defaultValue"], "Open");
        assert!(definitions["stateDefinitions"][0]
            .get("comparator")
            .is_none());
        assert!(definitions["stateDefinitions"][1]
            .get("comparator")
            .is_none());
        assert!(content.contains("\"stateDefinitions\" : [ {"));
        assert!(content.contains("      \"backgroundColor\" : \"\""));

        let (_dir, solution) = fixture();
        std::fs::create_dir_all(solution.root.join("app/StyleThemes")).unwrap();
        std::fs::write(
            solution
                .root
                .join("app/StyleThemes/Acme.Orders.Source_TH.xml"),
            "<Entities><StyleThemes><StyleTheme name=\"Acme.Orders.Source_TH\" projectName=\"Acme.Orders\"><content><![CDATA[{\"CustomCss\":\".acme{}\",\"props\":[],\"version\":\"0.1\"}]]></content></StyleTheme></StyleThemes></Entities>",
        )
        .unwrap();
        let mut theme = request(Kind::Theme, "Acme.Orders.Copy_TH");
        theme.from = Some("Acme.Orders.Source_TH".into());
        let entity = String::from_utf8(plan(&solution, &theme).unwrap().entity).unwrap();
        assert!(entity
            .contains("\"CustomCss\" : \".acme{}\",\n  \"props\" : [ ],\n  \"version\" : \"0.1\""));
    }

    #[test]
    fn new_kind_refusals_have_promised_codes() {
        let (_dir, solution) = fixture();
        let code = |request| plan(&solution, &request).unwrap_err().code();
        assert_eq!(
            code(request(Kind::DataTable, "Acme.Orders.Rows_DT")),
            ErrorCode::InvalidArguments
        );
        let mut unknown = request(Kind::Stream, "Acme.Orders.Events_ST");
        unknown.data_shape = Some("Acme.Orders.Missing_DS".into());
        assert_eq!(code(unknown), ErrorCode::UnknownEntity);
        let mut text = request(Kind::Media, "Acme.Orders.Text_MD");
        let path = solution.root.join("not-image.txt");
        std::fs::write(&path, "text").unwrap();
        text.file = Some(path);
        assert_eq!(code(text), ErrorCode::InvalidArguments);
        let mut style = request(Kind::Style, "Acme.Orders.Bad_SD");
        style.background = Some("green".into());
        assert_eq!(code(style), ErrorCode::InvalidArguments);
        let mut bad_text_size = request(Kind::Style, "Acme.Orders.TextSize_SD");
        bad_text_size.text_size = Some("huge".into());
        assert_eq!(code(bad_text_size), ErrorCode::InvalidArguments);
        let mut unknown_image = request(Kind::Style, "Acme.Orders.Image_SD");
        unknown_image.image = Some("Acme.Orders.Missing_MD".into());
        assert_eq!(code(unknown_image), ErrorCode::UnknownEntity);
        let mut state = request(Kind::State, "Acme.Orders.BadState_SD");
        state.state_type = Some("string".into());
        state.states = vec![">3=bad".into()];
        assert_eq!(code(state), ErrorCode::InvalidArguments);
        let mut stateless = request(Kind::State, "Acme.Orders.Empty_SD");
        stateless.state_type = Some("numeric".into());
        assert_eq!(code(stateless), ErrorCode::InvalidArguments);
        let mut non_numeric = request(Kind::State, "Acme.Orders.NonNumeric_SD");
        non_numeric.state_type = Some("numeric".into());
        non_numeric.states = vec!["<slow=bad".into()];
        assert_eq!(code(non_numeric), ErrorCode::InvalidArguments);
        let mut two_catch_alls = request(Kind::State, "Acme.Orders.CatchAll_SD");
        two_catch_alls.state_type = Some("numeric".into());
        two_catch_alls.states = vec!["*=first".into(), "*=second".into()];
        assert_eq!(code(two_catch_alls), ErrorCode::InvalidArguments);
        let mut numeric_catch_all_value = request(Kind::State, "Acme.Orders.Value_SD");
        numeric_catch_all_value.state_type = Some("numeric".into());
        numeric_catch_all_value.states = vec!["*3=bad".into()];
        assert_eq!(code(numeric_catch_all_value), ErrorCode::InvalidArguments);
        assert_eq!(
            code(request(Kind::Menu, "Acme.Orders.Empty_MN")),
            ErrorCode::InvalidArguments
        );
        let mut theme = request(Kind::Theme, "Acme.Orders.Theme_TH");
        theme.from = Some("Acme.Orders.Missing_TH".into());
        assert_eq!(code(theme), ErrorCode::UnknownEntity);

        let mut keyless = request(Kind::DataShape, "Acme.Orders.Keyless_DS");
        keyless.fields = vec!["id:STRING".into()];
        apply(&solution, keyless);
        let mut keyless_table = request(Kind::DataTable, "Acme.Orders.Keyless_DT");
        keyless_table.data_shape = Some("Acme.Orders.Keyless_DS".into());
        assert_eq!(code(keyless_table), ErrorCode::InvalidArguments);
        let mut keyless_stream = request(Kind::Stream, "Acme.Orders.Keyless_ST");
        keyless_stream.data_shape = Some("Acme.Orders.Keyless_DS".into());
        assert!(
            plan(&solution, &keyless_stream).is_ok(),
            "a stream needs no primary key"
        );

        let mut shape = request(Kind::DataShape, "Acme.Orders.Row_DS");
        shape.fields = vec!["id:STRING:primary".into()];
        apply(&solution, shape);
        let mut missing_index_field = request(Kind::DataTable, "Acme.Orders.Rows_DT");
        missing_index_field.data_shape = Some("Acme.Orders.Row_DS".into());
        missing_index_field.indexes = vec!["byMissing=missing".into()];
        assert_eq!(code(missing_index_field), ErrorCode::UnknownEntity);

        let large_media = solution.root.join("large.png");
        std::fs::write(&large_media, vec![0; 10 * 1024 * 1024 + 1]).unwrap();
        let mut media = request(Kind::Media, "Acme.Orders.Large_MD");
        media.file = Some(large_media);
        assert_eq!(code(media), ErrorCode::InvalidArguments);

        let mut style_cdata = request(Kind::Style, "Acme.Orders.CdataStyle_SD");
        style_cdata.display_string = Some("bad]]>value".into());
        assert_eq!(code(style_cdata), ErrorCode::InvalidArguments);
        let mut state_cdata = request(Kind::State, "Acme.Orders.CdataState_SD");
        state_cdata.state_type = Some("numeric".into());
        state_cdata.states = vec!["<3=bad]]>value".into()];
        assert_eq!(code(state_cdata), ErrorCode::InvalidArguments);
        let mut theme_cdata = request(Kind::Theme, "Acme.Orders.CdataTheme_TH");
        theme_cdata.props = vec!["color=bad]]>value".into()];
        assert_eq!(code(theme_cdata), ErrorCode::InvalidArguments);
        let mut menu_cdata = request(Kind::Menu, "Acme.Orders.CdataMenu_MN");
        menu_cdata.items = vec!["bad]]>value=Acme.Orders.Home_MU".into()];
        assert_eq!(code(menu_cdata), ErrorCode::InvalidArguments);
    }

    #[test]
    fn applies_every_new_kind_with_no_drift() {
        let (_dir, solution) = fixture();
        let image = solution.root.join("pixel.png");
        std::fs::write(
            &image,
            base64::engine::general_purpose::STANDARD
                .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNg+A8AAAEBAQBcDQotAAAAAElFTkSuQmCC")
                .unwrap(),
        )
        .unwrap();

        let mut shape = request(Kind::DataShape, "Acme.Orders.Row_DS");
        shape.fields = vec!["id:STRING:primary".into()];
        apply(&solution, shape);

        let mut table = request(Kind::DataTable, "Acme.Orders.Rows_DT");
        table.data_shape = Some("Acme.Orders.Row_DS".into());
        table.indexes = vec!["byId=id".into()];
        let mut stream = request(Kind::Stream, "Acme.Orders.Events_ST");
        stream.data_shape = Some("Acme.Orders.Row_DS".into());
        let mut media = request(Kind::Media, "Acme.Orders.Pixel_MD");
        media.file = Some(image);
        let mut style = request(Kind::Style, "Acme.Orders.Good_SD");
        style.image = Some("Acme.Orders.Pixel_MD".into());
        let mut state = request(Kind::State, "Acme.Orders.Speed_SD");
        state.state_type = Some("numeric".into());
        state.states = vec!["<3=Normal".into(), "*=Overload".into()];
        let mut menu = request(Kind::Menu, "Acme.Orders.Main_MN");
        menu.items = vec!["Home=Acme.Orders.Home_MU".into()];
        menu.default = true;
        let mut theme = request(Kind::Theme, "Acme.Orders.Theme_TH");
        theme.props = vec!["e-button-primary-fontcolor=#ffffff".into()];

        for request in [
            table,
            stream,
            request(Kind::ValueStream, "Acme.Orders.Values_VS"),
            media,
            style,
            state,
            menu,
            theme,
        ] {
            apply(&solution, request);
        }

        let table = workspace::entities(&solution)
            .into_iter()
            .find(|entity| entity.info.name == "Acme.Orders.Rows_DT")
            .expect("the new DataTable is discoverable");
        let xml = std::fs::read(&table.path).unwrap();
        assert_eq!(
            datatable::extract(&xml).unwrap(),
            datatable::from_sidecar(
                &std::fs::read_to_string(workspace::datatable_path(&solution, &table)).unwrap()
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
            .filter(|gate| !gate.ok())
            .collect();
        assert!(failing.is_empty(), "{failing:?}");
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
    fn accumulated_data_shape_json_is_in_java_hash_map_order() {
        let field = |name: &str, base_type: &str, aspects: &[(&str, Aspect)]| Field {
            name: name.into(),
            base_type: base_type.into(),
            ordinal: "9".into(),
            description: String::new(),
            aspects: aspects
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect(),
        };
        // Orders read back from real exports: keys by bucket, ordinals by name ignoring case.
        let json = accumulated_json(&[
            field("xdata", "NUMBER", &[]),
            field("guid", "STRING", &[]),
            field("zdata", "NUMBER", &[]),
            field("ydata", "NUMBER", &[]),
        ]);
        let value: Value = serde_json::from_str(&json).unwrap();
        let keys: Vec<(&String, i64)> = value["fieldDefinitions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, field)| (key, field["ordinal"].as_i64().unwrap()))
            .collect();
        let expected = [("zdata", 3), ("guid", 0), ("xdata", 1), ("ydata", 2)];
        assert_eq!(
            keys.iter()
                .map(|(k, o)| (k.as_str(), *o))
                .collect::<Vec<_>>(),
            expected
        );
        let steps = accumulated_json(&[
            field("stepNumber", "INTEGER", &[]),
            field("stepName", "STRING", &[]),
            field("stepInstruction", "STRING", &[]),
            field("jobOrder", "INTEGER", &[]),
        ]);
        assert!(steps.starts_with("{\"fieldDefinitions\":{\"stepName\":"));
        assert!(steps.contains("\"jobOrder\":{\"name\":\"jobOrder\",\"aspects\":{},\"description\":\"\",\"baseType\":\"INTEGER\",\"ordinal\":0}"));
        // As the live server wrote it for a two-field shape; a default is typed by base type.
        let live = accumulated_json(&[
            field("id", "STRING", &[("isPrimaryKey", Aspect::Bool(true))]),
            field("n", "INTEGER", &[("isPrimaryKey", Aspect::Bool(false))]),
        ]);
        assert_eq!(live, "{\"fieldDefinitions\":{\"id\":{\"name\":\"id\",\"aspects\":{\"isPrimaryKey\":true},\"description\":\"\",\"baseType\":\"STRING\",\"ordinal\":0},\"n\":{\"name\":\"n\",\"aspects\":{\"isPrimaryKey\":false},\"description\":\"\",\"baseType\":\"INTEGER\",\"ordinal\":1}}}");
        let rate = accumulated_json(&[field(
            "rate",
            "NUMBER",
            &[
                ("defaultValue", Aspect::Text("500".into())),
                ("isPrimaryKey", Aspect::Bool(false)),
            ],
        )]);
        assert!(rate.contains("\"aspects\":{\"defaultValue\":500.0,\"isPrimaryKey\":false}"));
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
            "GroupReferences",
            "ThingShape",
            "ImplementedShapes",
            "FieldDefinitions",
            "PropertyBindings",
            "SharedConfigurationTables",
        ];
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/entity/new/golden");
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
