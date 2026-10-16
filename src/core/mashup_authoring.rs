//! Creating and changing mashups in the repository, as Composer would.
//!
//! A new mashup's entity file is filled from the 10.2 template and its content written by
//! `mashup::sync`, so it is byte for byte what a sync of the same content would write. Edits to an
//! existing mashup change its `content.json` and go back through the same sync, which replaces
//! only the content section; nothing else in the entity file is re-serialised.

use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity_key::EntityKey;
use crate::core::lock::WorkspaceLock;
use crate::core::mashup;
use crate::core::mashup_address::{parse_address, Address};
use crate::core::mashup_describe::mashup_parameters;
use crate::core::mashup_edit::{self, EditError, MashupDocument, Placement};
use crate::core::mashup_templates::{self, MashupKind, Visibility};
use crate::core::transaction::{Transaction, TransactionError};
use crate::core::{bindings, catalog, scan, splice, types, widgets, workspace};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The theme a new mashup uses unless asked for another: 10.2's default.
pub const DEFAULT_THEME: &str = "PTC Convergence Theme";

#[derive(Clone, Debug)]
pub struct NewRequest {
    pub name: String,
    /// The project it belongs to; may be left out when the solution has one project.
    pub project: Option<String>,
    /// An embedded layout template name or alias.
    pub template: String,
    /// The required primary menu service for a template with a menu bar.
    pub primary_menu: Option<String>,
    /// The optional secondary menu service for a template with a menu bar.
    pub secondary_menu: Option<String>,
    /// A StyleTheme in the solution, or 10.2's default.
    pub theme: Option<String>,
    /// A master mashup: pages open inside it.
    pub master: bool,
    /// `Organization/<name>` or `OrganizationalUnit/<organization>:<unit>`; none unless asked.
    pub visibility: Vec<String>,
    /// `name:TYPE[:DIRECTION]` declarations to create with the mashup.
    pub params: Vec<String>,
}

/// One edit to a mashup parameter definition.
#[derive(Clone, Debug)]
pub struct ParameterRequest {
    pub mashup: String,
    pub action: ParameterAction,
    pub name: String,
    pub base_type: Option<String>,
    pub direction: Option<String>,
    pub description: Option<String>,
    pub default: ParameterDefault,
    pub data_shape: Option<String>,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterAction {
    Add,
    Set,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParameterDefault {
    Unchanged,
    Set(String),
    Clear,
}

#[derive(Debug)]
pub struct ParameterPlan {
    pub edits: Vec<EditPlan>,
    pub action: ParameterAction,
    pub mashup: String,
    pub name: String,
    pub base_type: String,
    pub direction: String,
    pub refreshed: Vec<(String, String)>,
    pub retyped: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug)]
pub struct NewPlan {
    pub name: String,
    pub project: String,
    pub template: mashup_templates::LayoutTemplate,
    pub master: bool,
    pub theme: String,
    pub visibility: Vec<Visibility>,
    pub entity_path: PathBuf,
    pub content_path: PathBuf,
    pub css_path: PathBuf,
    pub entity: Vec<u8>,
    pub assets: mashup::Assets,
    /// The widgets the new mashup holds, root first.
    pub widgets: Vec<mashup_edit::WidgetRef>,
    /// Menu service calls wired while creating a sidebar.
    pub menus: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct AddRequest {
    pub mashup: String,
    pub widget: String,
    pub placement: Option<Placement>,
    pub label: Option<String>,
    pub properties: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct SidecarEdit {
    pub content_path: PathBuf,
    pub content_before: Vec<u8>,
    pub content_after: Vec<u8>,
    pub css_path: PathBuf,
    /// The stylesheet is retained while content changes.
    pub css: String,
}

#[derive(Debug)]
pub struct EditPlan {
    /// The command, as the transaction journal names it (`mashup add`, `mashup bind`).
    pub command: &'static str,
    pub mashup: String,
    pub entity_path: PathBuf,
    pub entity_before: Vec<u8>,
    pub entity_after: Vec<u8>,
    /// `None` means this mashup has no sidecars and only its entity is changed.
    pub sidecars: Option<SidecarEdit>,
}

#[derive(Debug)]
pub struct AddPlan {
    pub edit: EditPlan,
    pub placed: mashup_edit::Placed,
    /// The catalogue type, such as `ptcsgrid`.
    pub widget: String,
}

/// A contained mashup is a `mashupcontainer` whose public properties mirror the child's
/// parameters.  It deliberately has its own request rather than pretending that the catalogue
/// knows the child-specific contract.
#[derive(Clone, Debug)]
pub struct AddContainedRequest {
    pub mashup: String,
    pub child: String,
    pub placement: Option<Placement>,
    pub settings: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct AddContainedPlan {
    pub edit: EditPlan,
    pub placed: mashup_edit::Placed,
    pub child: String,
    pub parameters: Vec<ContainedParameter>,
    pub settings: Vec<String>,
}

#[derive(Debug)]
pub struct ContainedParameter {
    pub name: String,
    pub base_type: String,
    pub direction: String,
}

#[derive(Clone, Debug)]
pub struct BindRequest {
    pub mashup: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug)]
pub struct BindPlan {
    pub edit: EditPlan,
    pub from: String,
    pub to: String,
    pub new_source: Option<String>,
    /// Said, not refused: the target already has another source.
    pub warnings: Vec<String>,
    /// Contained-mashup caches repaired while resolving an endpoint.
    pub refreshed: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct OnRequest {
    pub mashup: String,
    pub event: String,
    pub invoke: String,
}

#[derive(Debug)]
pub struct OnPlan {
    pub edit: EditPlan,
    pub event: String,
    pub invoke: String,
    pub new_source: Option<String>,
}

struct ExistingMashup {
    mashup: String,
    entity_path: PathBuf,
    entity_before: Vec<u8>,
    assets: mashup::Assets,
    sidecars: Option<(PathBuf, Vec<u8>, PathBuf)>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthoringError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unknown(String),
    #[error("{0}")]
    Exists(String),
    #[error("{0}")]
    Ambiguous(String),
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Edit(EditError),
    #[error("{0}")]
    Mashup(mashup::MashupError),
    #[error("{0}")]
    Unreadable(String),
    #[error("nothing was written: {0}")]
    Write(TransactionError),
}

impl Coded for AuthoringError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Unknown(_) => ErrorCode::UnknownEntity,
            Self::Exists(_) => ErrorCode::AlreadyExists,
            Self::Ambiguous(_) => ErrorCode::Ambiguous,
            Self::Refused(_) => ErrorCode::GuardRefused,
            Self::Edit(error) => error.code(),
            Self::Mashup(_) => ErrorCode::InvalidData,
            Self::Unreadable(_) => ErrorCode::InvalidData,
            Self::Write(error) => error.code(),
        }
    }
}

/// `Organization/<name>` or `OrganizationalUnit/<organization>:<unit>`.
pub fn parse_visibility(text: &str) -> Result<Visibility, AuthoringError> {
    let invalid = || {
        AuthoringError::Invalid(format!(
            "visibility {text:?} is not Organization/<name> or OrganizationalUnit/<organization>:<unit>"
        ))
    };
    let (kind, name) = text.split_once('/').ok_or_else(invalid)?;
    let well_formed = match kind {
        "Organization" => !name.is_empty() && !name.contains(':'),
        "OrganizationalUnit" => name
            .split_once(':')
            .is_some_and(|(organization, unit)| !organization.is_empty() && !unit.is_empty()),
        _ => false,
    };
    if !well_formed || name.trim() != name {
        return Err(invalid());
    }
    Ok(Visibility {
        kind: kind.to_string(),
        name: name.to_string(),
    })
}

/// What creating the mashup would write. Nothing is written.
pub fn plan_new(solution: &Solution, request: &NewRequest) -> Result<NewPlan, AuthoringError> {
    let template = mashup_templates::layout(&request.template)
        .map_err(AuthoringError::Invalid)?
        .ok_or_else(|| AuthoringError::Invalid(unknown_template(&request.template)))?;
    validate_template(&template)?;
    EntityKey::new("Mashups", request.name.as_str())
        .map_err(|error| AuthoringError::Invalid(format!("not a mashup name: {error}")))?;
    // The name is a file name twice over (the entity file, the sidecar folder), on every platform.
    if !crate::core::adopt::is_file_name(&request.name) {
        return Err(AuthoringError::Invalid(format!(
            "{:?} cannot be a file name on every platform; leave out / \\ : * ? \" < > | and a device name such as CON",
            request.name
        )));
    }
    let project = match &request.project {
        Some(name) => solution.project(name).ok_or_else(|| {
            AuthoringError::Unknown(format!("twaco.toml declares no project {name}"))
        })?,
        None => match solution.projects.as_slice() {
            [only] => only,
            _ => {
                return Err(AuthoringError::Invalid(
                    "name the project the mashup belongs to with --project".to_string(),
                ))
            }
        },
    };
    if !project.collections.is_empty() && !project.collections.iter().any(|c| c == "Mashups") {
        return Err(AuthoringError::Invalid(format!(
            "project {} keeps only {}; add Mashups to its collections first",
            project.name,
            project.collections.join(", ")
        )));
    }
    let entities = workspace::entities(solution);
    if let Some(existing) = entities
        .iter()
        .find(|e| e.info.collection == "Mashups" && e.info.name == request.name)
    {
        return Err(AuthoringError::Exists(format!(
            "mashup {} already exists: {}",
            request.name,
            existing.path.display()
        )));
    }
    let theme = match &request.theme {
        None => DEFAULT_THEME.to_string(),
        Some(theme) if theme == DEFAULT_THEME => theme.clone(),
        Some(theme) => {
            if !entities
                .iter()
                .any(|e| e.info.collection == "StyleThemes" && &e.info.name == theme)
            {
                return Err(AuthoringError::Unknown(format!(
                    "no StyleTheme {theme} in the solution; twaco does not create one (the default is {DEFAULT_THEME})"
                )));
            }
            theme.clone()
        }
    };
    let visibility = request
        .visibility
        .iter()
        .map(|text| parse_visibility(text))
        .collect::<Result<Vec<_>, _>>()?;

    let entity_path = solution
        .project_root(project)
        .join("Mashups")
        .join(format!("{}.xml", request.name));
    let dir = solution.src_root().join(&request.name).join("mashup");
    let content_path = dir.join("content.json");
    let css_path = dir.join("custom.css");
    for path in [&entity_path, &content_path, &css_path] {
        if path.symlink_metadata().is_ok() {
            return Err(AuthoringError::Exists(format!(
                "{} already exists",
                path.display()
            )));
        }
        // What apply would refuse, refused now: a link on the way, or a file where a folder goes.
        crate::core::transaction::check_inside(&solution.root, path)
            .map_err(AuthoringError::Invalid)?;
        for folder in path.ancestors().skip(1) {
            if folder == solution.root {
                break;
            }
            if folder.symlink_metadata().is_ok_and(|meta| !meta.is_dir()) {
                return Err(AuthoringError::Exists(format!(
                    "{} is a file where the mashup needs a folder",
                    folder.display()
                )));
            }
        }
    }

    let mut document = new_document(&template, request.master, &theme)?;
    let parameters: Vec<ParameterDefinition> = request
        .params
        .iter()
        .map(|text| parse_new_parameter(text))
        .collect::<Result<_, _>>()?;
    let mut seen = std::collections::BTreeSet::new();
    for parameter in &parameters {
        if !seen.insert(parameter.name.clone()) {
            return Err(AuthoringError::Exists(format!(
                "parameter {} already exists",
                parameter.name
            )));
        }
        set_content_parameter(&mut document, parameter, false);
    }
    let menu = document
        .widgets()
        .into_iter()
        .find(|widget| widget.type_id == "ptcsmenubar");
    match (menu.as_ref(), request.primary_menu.as_ref()) {
        (Some(_), None) => {
            return Err(AuthoringError::Invalid(format!(
                "--primary-menu is required for --template {}",
                template.name
            )));
        }
        (None, _) if request.primary_menu.is_some() || request.secondary_menu.is_some() => {
            return Err(AuthoringError::Invalid(
                "menus are for a template with a menu bar (sidebar, header-sidebar)".into(),
            ));
        }
        _ => {}
    }
    let mut menus = Vec::new();
    if let Some(primary_menu) = &request.primary_menu {
        let (model, _) = types::load_model(solution);
        let platform = types::cached_platform(solution);
        let parameters = Vec::new();
        let context = AuthoringContext {
            parameters: &parameters,
            model: &model,
            platform: platform.as_ref(),
        };
        for (service, property) in std::iter::once((primary_menu, "PrimaryNavData")).chain(
            request
                .secondary_menu
                .as_ref()
                .map(|service| (service, "SecondaryNavData")),
        ) {
            menu_service(&mut document, &context, service)?;
            bind_in(
                &mut document,
                Some(solution),
                &context,
                service,
                &format!("ui:{}.{property}", menu.as_ref().expect("checked menu").id),
            )?;
            on_in(&mut document, &context, "mashup:Loaded", service)?;
            menus.push((service.clone(), property.to_string()));
        }
    }
    let kind = if request.master {
        MashupKind::Master
    } else {
        MashupKind::Mashup
    };
    let assets =
        mashup::assets_from_payload(&document.to_content()).map_err(AuthoringError::Mashup)?;
    let entity_template =
        mashup_templates::entity_xml(&request.name, &project.name, kind, &visibility);
    let (mut entity, _) =
        mashup::sync(entity_template.as_bytes(), &assets).map_err(AuthoringError::Mashup)?;
    for parameter in &parameters {
        entity = splice_parameter_definition(&entity, parameter, None, false)?;
    }
    Ok(NewPlan {
        name: request.name.clone(),
        project: project.name.clone(),
        template,
        master: request.master,
        theme,
        visibility,
        entity_path,
        content_path,
        css_path,
        entity,
        assets,
        widgets: document.widgets(),
        menus,
    })
}

/// The content of a new mashup.
fn new_document(
    template: &mashup_templates::LayoutTemplate,
    master: bool,
    theme: &str,
) -> Result<MashupDocument, AuthoringError> {
    let mut document = mashup_edit::parse(mashup_templates::BLANK).map_err(AuthoringError::Edit)?;
    let root = document.find("center").map_err(AuthoringError::Edit)?;
    for (key, value) in &template.page {
        document.set_property(&root.path, key, value.clone());
    }
    if !template.children.is_empty() {
        document.set_property(&root.path, "DisplayName", json!("page"));
        for child in &template.children {
            add_template_node(&mut document, &root.path, child, template.name.as_str())?;
        }
    }
    if master {
        let index = document.root_properties_mut()["id_index"].clone();
        let Value::Object(mut root) = mashup_templates::master_root() else {
            unreachable!("the master root is an object; a test checks it");
        };
        root.insert("id_index".into(), index);
        *document.root_properties_mut() = root;
        document.set_top("mashupType", json!(MashupKind::Master.content_type()));
        // Pages open in the template's designated page container.
        let center = document
            .find(template.master_slot.as_deref().unwrap_or("center"))
            .map_err(AuthoringError::Edit)?;
        let number = document.allocate();
        let page = widgets::catalog()
            .by_type("pagemashupcontainer")
            .expect("the catalogue has the page container; a test checks it");
        document.push_child(&center.path, mashup_edit::new_widget(page, number, true));
    }
    mashup_edit::insert_sorted(document.root_properties_mut(), "StyleTheme", json!(theme));
    Ok(document)
}

#[derive(Clone, Debug)]
struct ParameterDefinition {
    name: String,
    base_type: String,
    direction: String,
    description: String,
    default: Option<Value>,
    data_shape: Option<String>,
    visible: bool,
}

fn parse_new_parameter(text: &str) -> Result<ParameterDefinition, AuthoringError> {
    let mut parts = text.split(':');
    let name = parts.next().unwrap_or_default();
    let base_type = parts.next().ok_or_else(|| {
        AuthoringError::Invalid(format!("--param {text:?} must be name:TYPE[:DIRECTION]"))
    })?;
    let direction = parts.next().unwrap_or("BOTH");
    if parts.next().is_some() {
        return Err(AuthoringError::Invalid(format!(
            "--param {text:?} must be name:TYPE[:DIRECTION]"
        )));
    }
    validate_parameter(name, base_type, direction, None)?;
    if base_type == "INFOTABLE" {
        return Err(AuthoringError::Invalid(
            "INFOTABLE parameters need --data-shape and cannot be used with mashup new".into(),
        ));
    }
    Ok(ParameterDefinition {
        name: name.into(),
        base_type: base_type.into(),
        direction: direction.into(),
        description: String::new(),
        default: None,
        data_shape: None,
        visible: !name.starts_with("ConfigForm_"),
    })
}

fn known_base_type(name: &str) -> bool {
    matches!(
        name,
        "BASE64"
            | "BLOB"
            | "BOOLEAN"
            | "DATASHAPENAME"
            | "DATETIME"
            | "GUID"
            | "HTML"
            | "HYPERLINK"
            | "IMAGE"
            | "IMAGELINK"
            | "INFOTABLE"
            | "INTEGER"
            | "JSON"
            | "LOCATION"
            | "LONG"
            | "MASHUPNAME"
            | "MEDIAENTITYNAME"
            | "MENUNAME"
            | "NAME"
            | "NUMBER"
            | "PASSWORD"
            | "PROJECTNAME"
            | "QUERY"
            | "SCHEDULE"
            | "STRING"
            | "TAG"
            | "TEXT"
            | "THINGCODE"
            | "THINGNAME"
            | "THINGSHAPENAME"
            | "THINGTEMPLATENAME"
            | "USERNAME"
            | "VARIANT"
            | "XML"
    )
}

fn validate_parameter(
    name: &str,
    base_type: &str,
    direction: &str,
    data_shape: Option<&str>,
) -> Result<(), AuthoringError> {
    if !name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) && !name.starts_with('_')
        || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(AuthoringError::Invalid(format!(
            "parameter name {name:?} is not a valid identifier"
        )));
    }
    if matches!(name, "Entity" | "ConfigurationMashup") {
        return Err(AuthoringError::Invalid(format!(
            "parameter name {name} is reserved"
        )));
    }
    if !known_base_type(base_type) {
        return Err(AuthoringError::Invalid(format!(
            "unknown ThingWorx base type {base_type}"
        )));
    }
    if !matches!(direction, "IN" | "OUT" | "BOTH") {
        return Err(AuthoringError::Invalid(
            "direction must be IN, OUT or BOTH".into(),
        ));
    }
    if base_type == "INFOTABLE" && data_shape.is_none() {
        return Err(AuthoringError::Invalid(
            "INFOTABLE parameters require --data-shape".into(),
        ));
    }
    Ok(())
}

fn default_value(base_type: &str, value: &str) -> Result<Value, AuthoringError> {
    match base_type {
        "INFOTABLE" | "JSON" => Err(AuthoringError::Invalid(
            "this parameter is structured; set it in Composer".into(),
        )),
        "BOOLEAN" => match value {
            "true" => Ok(json!(true)),
            "false" => Ok(json!(false)),
            _ => Err(AuthoringError::Invalid(
                "default must be true or false".into(),
            )),
        },
        "NUMBER" => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| AuthoringError::Invalid("default must be a JSON number".into())),
        "INTEGER" | "LONG" => value
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| AuthoringError::Invalid("default must be a whole number".into())),
        _ => Ok(Value::String(value.into())),
    }
}

fn definition_from_request(
    request: &ParameterRequest,
    existing: Option<&ParameterDefinition>,
) -> Result<ParameterDefinition, AuthoringError> {
    let base_type = request
        .base_type
        .clone()
        .or_else(|| existing.map(|p| p.base_type.clone()))
        .ok_or_else(|| {
            AuthoringError::Invalid("--type is required when adding a parameter".into())
        })?;
    let direction = request
        .direction
        .clone()
        .or_else(|| existing.map(|p| p.direction.clone()))
        .unwrap_or_else(|| "BOTH".into());
    let description = request
        .description
        .clone()
        .or_else(|| existing.map(|p| p.description.clone()))
        .unwrap_or_default();
    let data_shape = request
        .data_shape
        .clone()
        .or_else(|| existing.and_then(|p| p.data_shape.clone()));
    validate_parameter(&request.name, &base_type, &direction, data_shape.as_deref())?;
    let default = match &request.default {
        ParameterDefault::Set(value) => Some(default_value(&base_type, value)?),
        ParameterDefault::Clear => None,
        ParameterDefault::Unchanged => existing.and_then(|p| p.default.clone()),
    };
    Ok(ParameterDefinition {
        name: request.name.clone(),
        base_type,
        direction,
        description,
        default,
        data_shape,
        visible: !request.name.starts_with("ConfigForm_"),
    })
}

fn content_parameter(document: &MashupDocument, name: &str) -> Option<ParameterDefinition> {
    let properties = document.value()["UI"]["Properties"].as_object()?;
    properties
        .get("_currentParameterDefs")?
        .as_array()?
        .iter()
        .find_map(|entry| {
            (entry["ParameterName"] == name).then(|| ParameterDefinition {
                name: name.into(),
                base_type: entry["BaseType"].as_str().unwrap_or_default().into(),
                direction: entry["Aspects"]["bindingDirection"]
                    .as_str()
                    .unwrap_or("BOTH")
                    .into(),
                description: entry["Description"].as_str().unwrap_or_default().into(),
                default: entry.get("DefaultValue").cloned(),
                data_shape: entry["Aspects"]["dataShape"].as_str().map(str::to_string),
                visible: entry["Aspects"]["isVisible"] != false && !name.starts_with("ConfigForm_"),
            })
        })
}

fn parameter_entry(def: &ParameterDefinition) -> Value {
    let mut aspects = Map::new();
    aspects.insert("bindingDirection".into(), json!(def.direction));
    if let Some(shape) = &def.data_shape {
        aspects.insert("dataShape".into(), json!(shape));
    }
    aspects.insert("isMandatory".into(), json!(false));
    let mut item = Map::new();
    item.insert("Aspects".into(), Value::Object(aspects));
    item.insert("BaseType".into(), json!(def.base_type));
    if let Some(value) = &def.default {
        item.insert("DefaultValue".into(), value.clone());
    }
    item.insert("Description".into(), json!(def.description));
    item.insert("ParameterName".into(), json!(def.name));
    Value::Object(item)
}

fn set_content_parameter(document: &mut MashupDocument, def: &ParameterDefinition, remove: bool) {
    let root = document
        .value_mut()
        .as_object_mut()
        .expect("checked document");
    let properties = root
        .get_mut("UI")
        .and_then(Value::as_object_mut)
        .and_then(|ui| ui.get_mut("Properties"))
        .and_then(Value::as_object_mut)
        .expect("checked root properties");
    // The server keeps these keys sorted; appended, the read-back differs from what was sent.
    if !properties.contains_key("_currentParameterDefs") {
        mashup_edit::insert_sorted(
            properties,
            "_currentParameterDefs",
            Value::Array(Vec::new()),
        );
    }
    let defs = properties
        .get_mut("_currentParameterDefs")
        .expect("inserted above")
        .as_array_mut()
        .expect("parameter definitions must be an array");
    if remove {
        defs.retain(|entry| entry["ParameterName"] != def.name);
        properties.shift_remove(&def.name);
        return;
    }
    if let Some(entry) = defs
        .iter_mut()
        .find(|entry| entry["ParameterName"] == def.name)
    {
        *entry = parameter_entry(def);
    } else {
        defs.push(parameter_entry(def));
    }
    match &def.default {
        Some(value) => {
            mashup_edit::insert_sorted(properties, &def.name, value.clone());
        }
        None => {
            properties.shift_remove(&def.name);
        }
    }
}

fn field_xml(def: &ParameterDefinition) -> String {
    let mut attributes = format!(
        "aspect.bindingDirection=\"{}\"",
        scan::escape_attribute(&def.direction)
    );
    if let Some(shape) = &def.data_shape {
        attributes.push_str(&format!(
            " aspect.dataShape=\"{}\"",
            scan::escape_attribute(shape)
        ));
    }
    attributes.push_str(&format!(" aspect.isMandatory=\"false\" baseType=\"{}\" description=\"{}\" name=\"{}\" ordinal=\"0\"", scan::escape_attribute(&def.base_type), scan::escape_attribute(&def.description), scan::escape_attribute(&def.name)));
    format!("<FieldDefinition {attributes}/>")
}

/// Change just the parameter definition element, retaining every byte outside it.
fn splice_parameter_definition(
    src: &[u8],
    def: &ParameterDefinition,
    old_name: Option<&str>,
    remove: bool,
) -> Result<Vec<u8>, AuthoringError> {
    let tokens = scan::tokenize(src).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let entity = crate::core::sidecar::entity_element(&tokens, src)
        .ok_or_else(|| AuthoringError::Invalid("not an entity document".into()))?;
    let definitions = *scan::child_tags(&tokens, src, "ParameterDefinitions", entity)
        .first()
        .ok_or_else(|| AuthoringError::Invalid("mashup has no ParameterDefinitions".into()))?;
    let fields = scan::child_tags(&tokens, src, "FieldDefinition", definitions);
    let find = |name: &str| -> Result<Option<usize>, AuthoringError> {
        fields
            .iter()
            .copied()
            .find_map(|field| match scan::attribute(src, &tokens[field], "name") {
                Ok(Some(span))
                    if scan::decode_entities(&String::from_utf8_lossy(span.of(src))) == name =>
                {
                    Some(Ok(field))
                }
                Ok(_) => None,
                Err(e) => Some(Err(AuthoringError::Invalid(e.to_string()))),
            })
            .transpose()
    };
    let wanted = old_name.unwrap_or(&def.name);
    let found = find(wanted)?;
    if remove {
        let field = found
            .ok_or_else(|| AuthoringError::Unknown(format!("mashup has no parameter {wanted}")))?;
        let span = scan::element_span(&tokens, field)
            .ok_or_else(|| AuthoringError::Invalid("malformed FieldDefinition".into()))?;
        return splice::splice(src, &[splice::Edit::new(span, Vec::new())])
            .map_err(|e| AuthoringError::Invalid(e.to_string()));
    }
    if let Some(field) = found {
        let span = scan::element_span(&tokens, field)
            .ok_or_else(|| AuthoringError::Invalid("malformed FieldDefinition".into()))?;
        return splice::splice(src, &[splice::Edit::new(span, field_xml(def))])
            .map_err(|e| AuthoringError::Invalid(e.to_string()));
    }
    // The server keeps parameter definitions sorted by name, ignoring case: insert where it would.
    let wanted_key = def.name.to_lowercase();
    for &field in &fields {
        let name = scan::attribute(src, &tokens[field], "name")
            .map_err(|e| AuthoringError::Invalid(e.to_string()))?
            .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
            .unwrap_or_default();
        if name.to_lowercase() > wanted_key {
            let start = tokens[field].span.start;
            let indent = String::from_utf8_lossy(&src[..start])
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .to_string();
            let separator = if indent.trim().is_empty() && !indent.is_empty() {
                format!("\n{indent}")
            } else {
                String::new()
            };
            return splice::splice(
                src,
                &[splice::Edit::new(
                    scan::Span::new(start, start),
                    format!("{}{separator}", field_xml(def)),
                )],
            )
            .map_err(|e| AuthoringError::Invalid(e.to_string()));
        }
    }
    let end = scan::element_end_in(&tokens, src, definitions)
        .ok_or_else(|| AuthoringError::Invalid("malformed ParameterDefinitions".into()))?;
    let inner = scan::Span::new(tokens[definitions].span.end, tokens[end].span.start);
    let parent_indent = String::from_utf8_lossy(&src[..tokens[definitions].span.start])
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect::<String>();
    let child_indent = format!("{parent_indent}    ");
    let before = String::from_utf8_lossy(inner.of(src));
    let replacement = if before.trim().is_empty() {
        format!("\n{child_indent}{}\n{parent_indent}", field_xml(def))
    } else {
        format!(
            "{}\n{child_indent}{}\n{parent_indent}",
            before.trim_end(),
            field_xml(def)
        )
    };
    splice::splice(src, &[splice::Edit::new(inner, replacement)])
        .map_err(|e| AuthoringError::Invalid(e.to_string()))
}

fn cache_entry(def: &ParameterDefinition, value: Option<Value>) -> Value {
    let mut aspects = Map::new();
    aspects.insert("bindingDirection".into(), json!(def.direction));
    if let Some(shape) = &def.data_shape {
        aspects.insert("dataShape".into(), json!(shape));
    }
    aspects.insert("isMandatory".into(), json!("false"));
    let mut parm = Map::new();
    parm.insert("aspects".into(), Value::Object(aspects));
    parm.insert("baseType".into(), json!(def.base_type));
    parm.insert("description".into(), json!(def.description));
    parm.insert("name".into(), json!(def.name));
    parm.insert("ordinal".into(), json!(0));
    let mut entry = Map::new();
    entry.insert("BaseType".into(), json!(def.base_type));
    if let Some(value) = value {
        entry.insert("DefaultValue".into(), value);
    }
    entry.insert("Description".into(), json!(def.description));
    entry.insert("ParameterName".into(), json!(def.name));
    entry.insert("ParmDef".into(), Value::Object(parm));
    Value::Object(entry)
}

fn refresh_containers(
    value: &mut Value,
    child: &str,
    definitions: &[ParameterDefinition],
    refreshed: &mut Vec<(String, String)>,
) {
    fn visit(
        widget: &mut Value,
        child: &str,
        definitions: &[ParameterDefinition],
        refreshed: &mut Vec<(String, String)>,
    ) {
        let properties = widget.get_mut("Properties").and_then(Value::as_object_mut);
        if let Some(properties) = properties {
            if properties.get("Type").and_then(Value::as_str) == Some("mashupcontainer")
                && properties.get("Name").and_then(Value::as_str) == Some(child)
            {
                let id = properties
                    .get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let previous = properties
                    .get("MashupParameters")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut rebuilt = Vec::new();
                for def in definitions {
                    let old = previous
                        .iter()
                        .find(|entry| entry["ParameterName"] == def.name);
                    let old_type = old.and_then(|entry| entry["BaseType"].as_str());
                    let value = if old_type == Some(def.base_type.as_str()) {
                        old.and_then(|entry| entry.get("DefaultValue"))
                            .cloned()
                            .or_else(|| properties.get(&def.name).cloned())
                            .or_else(|| def.default.clone())
                    } else {
                        properties.shift_remove(&def.name);
                        def.default.clone()
                    };
                    if let Some(value) = &value {
                        mashup_edit::insert_sorted(properties, &def.name, value.clone());
                    }
                    rebuilt.push(cache_entry(def, value));
                }
                for old in previous {
                    if let Some(name) = old["ParameterName"].as_str() {
                        if !definitions.iter().any(|d| d.name == name) {
                            properties.shift_remove(name);
                        }
                    }
                }
                mashup_edit::insert_sorted(properties, "MashupParameters", Value::Array(rebuilt));
                refreshed.push((id, child.into()));
            }
        }
        if let Some(children) = widget.get_mut("Widgets").and_then(Value::as_array_mut) {
            for child_widget in children {
                visit(child_widget, child, definitions, refreshed);
            }
        }
    }
    visit(&mut value["UI"], child, definitions, refreshed);
}

fn change_parameter_bindings(
    value: &mut Value,
    child: &str,
    name: &str,
    new_type: Option<&str>,
    force: bool,
    labels: &mut Vec<String>,
) -> Result<(), AuthoringError> {
    let container_ids: std::collections::BTreeSet<String> = {
        fn collect(widget: &Value, child: &str, into: &mut std::collections::BTreeSet<String>) {
            if widget["Properties"]["Type"] == "mashupcontainer"
                && widget["Properties"]["Name"] == child
            {
                if let Some(id) = widget["Properties"]["Id"].as_str() {
                    into.insert(id.into());
                }
            }
            for w in widget["Widgets"].as_array().into_iter().flatten() {
                collect(w, child, into);
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        collect(&value["UI"], child, &mut ids);
        ids
    };
    let bindings = value["DataBindings"].as_array_mut().into_iter().flatten();
    for binding in bindings {
        let source = (container_ids.is_empty() && binding["SourceArea"].as_str() == Some("Mashup"))
            || container_ids.contains(binding["SourceId"].as_str().unwrap_or_default());
        let target = (container_ids.is_empty() && binding["TargetArea"].as_str() == Some("Mashup"))
            || container_ids.contains(binding["TargetId"].as_str().unwrap_or_default());
        if !source && !target {
            continue;
        }
        let id = binding["Id"].as_str().unwrap_or("binding").to_string();
        let maps = binding["PropertyMaps"].as_array_mut().into_iter().flatten();
        let mut remove_binding = false;
        for map in maps {
            let relevant = (source && map["SourceProperty"] == name)
                || (target && map["TargetProperty"] == name);
            if !relevant {
                continue;
            }
            if new_type.is_none() {
                if !force {
                    labels.push(id.clone());
                } else {
                    remove_binding = true;
                    labels.push(id.clone());
                }
                continue;
            }
            let new_type = new_type.unwrap();
            let other = if source {
                map["TargetPropertyBaseType"].as_str()
            } else {
                map["SourcePropertyBaseType"].as_str()
            };
            // The rule `mashup bind` applies: the same base type, or a STRING target.
            let compatible = match other {
                None => true,
                Some(other) if source => other == new_type || other == "STRING",
                Some(other) => other == new_type || new_type == "STRING",
            };
            if !compatible {
                return Err(AuthoringError::Refused(format!(
                    "binding {id} would join {new_type} with {}; change or remove it first",
                    other.unwrap_or_default()
                )));
            }
            if source {
                map["SourcePropertyBaseType"] = json!(new_type);
            } else {
                map["TargetPropertyBaseType"] = json!(new_type);
            }
            labels.push(id.clone());
        }
        if remove_binding {
            binding["__twaco_remove"] = json!(true);
        }
    }
    if new_type.is_none() && !force && !labels.is_empty() {
        return Err(AuthoringError::Refused(format!(
            "parameter {name} is still bound by {}; use --force to remove those bindings",
            labels.join(", ")
        )));
    }
    if force {
        if let Some(bindings) = value["DataBindings"].as_array_mut() {
            bindings.retain(|b| b["__twaco_remove"] != true);
        }
    }
    Ok(())
}

fn document_parameters(document: &MashupDocument) -> Vec<ParameterDefinition> {
    document.value()["UI"]["Properties"]["_currentParameterDefs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry["ParameterName"]
                .as_str()
                .map(|name| ParameterDefinition {
                    name: name.into(),
                    base_type: entry["BaseType"].as_str().unwrap_or_default().into(),
                    direction: entry["Aspects"]["bindingDirection"]
                        .as_str()
                        .unwrap_or("BOTH")
                        .into(),
                    description: entry["Description"].as_str().unwrap_or_default().into(),
                    default: entry.get("DefaultValue").cloned(),
                    data_shape: entry["Aspects"]["dataShape"].as_str().map(str::to_string),
                    visible: entry["Aspects"]["isVisible"] != false
                        && !name.starts_with("ConfigForm_"),
                })
        })
        .collect()
}

/// Plan a parameter addition, definition change or deletion, including every container that
/// displays the child mashup. Nothing is written.
pub fn plan_parameter(
    solution: &Solution,
    request: &ParameterRequest,
) -> Result<ParameterPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let entity_parameters =
        mashup_parameters(&loaded.entity_before).map_err(AuthoringError::Invalid)?;
    let mut document = parse_loaded(&loaded)?;
    let existing_xml = entity_parameters
        .iter()
        .find(|parameter| parameter.name == request.name);
    let existing = content_parameter(&document, &request.name).or_else(|| {
        existing_xml.map(|parameter| ParameterDefinition {
            name: parameter.name.clone(),
            base_type: parameter.base_type.clone(),
            direction: if parameter.direction.is_empty() {
                "BOTH".into()
            } else {
                parameter.direction.clone()
            },
            description: String::new(),
            default: None,
            data_shape: None,
            visible: !parameter.name.starts_with("ConfigForm_"),
        })
    });
    if request.action == ParameterAction::Add && existing.is_some() {
        return Err(AuthoringError::Exists(format!(
            "mashup {} already has parameter {}",
            request.mashup, request.name
        )));
    }
    if request.action != ParameterAction::Add && existing.is_none() {
        return Err(AuthoringError::Unknown(format!(
            "mashup {} has no parameter {}",
            request.mashup, request.name
        )));
    }
    if request.action == ParameterAction::Remove && existing.is_none() {
        return Err(AuthoringError::Unknown(format!(
            "mashup {} has no parameter {}",
            request.mashup, request.name
        )));
    }
    let definition = existing
        .as_ref()
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| definition_from_request(request, None))?;
    let definition = if request.action == ParameterAction::Remove {
        definition
    } else {
        definition_from_request(request, existing.as_ref())?
    };
    if definition.base_type == "INFOTABLE" {
        let shape = definition.data_shape.as_deref().unwrap_or_default();
        if !workspace::entities(solution)
            .iter()
            .any(|entity| entity.info.collection == "DataShapes" && entity.info.name == shape)
        {
            return Err(AuthoringError::Unknown(format!(
                "no DataShape {shape} in the solution"
            )));
        }
    }
    let mut changed_bindings = Vec::new();
    change_parameter_bindings(
        document.value_mut(),
        &request.mashup,
        &request.name,
        (request.action != ParameterAction::Remove).then_some(definition.base_type.as_str()),
        request.force,
        &mut changed_bindings,
    )?;
    set_content_parameter(
        &mut document,
        &definition,
        request.action == ParameterAction::Remove,
    );
    let definitions = document_parameters(&document);
    let mut child_edit = finish_edit(loaded, document, "mashup param")?;
    child_edit.entity_after = splice_parameter_definition(
        &child_edit.entity_after,
        &definition,
        existing.as_ref().map(|_| request.name.as_str()),
        request.action == ParameterAction::Remove,
    )?;
    let mut edits = vec![child_edit];
    let mut refreshed = Vec::new();
    for entity in workspace::entities(solution)
        .into_iter()
        .filter(|entity| entity.info.collection == "Mashups" && entity.info.name != request.mashup)
    {
        let bytes = std::fs::read(&entity.path).map_err(|error| {
            AuthoringError::Unreadable(format!("{}: {error}", entity.path.display()))
        })?;
        let content = mashup::extract(&bytes)
            .map_err(AuthoringError::Mashup)?
            .content;
        let value: Value = serde_json::from_str(&content).map_err(|error| {
            AuthoringError::Invalid(format!("{}: {error}", entity.path.display()))
        })?;
        if !has_container(&value, &request.mashup) {
            continue;
        }
        let parent = load_existing(solution, &entity.info.name)?;
        let mut parent_document = parse_loaded(&parent)?;
        change_parameter_bindings(
            parent_document.value_mut(),
            &request.mashup,
            &request.name,
            (request.action != ParameterAction::Remove).then_some(definition.base_type.as_str()),
            request.force,
            &mut changed_bindings,
        )?;
        refresh_containers(
            parent_document.value_mut(),
            &request.mashup,
            &definitions,
            &mut refreshed,
        );
        edits.push(finish_edit(parent, parent_document, "mashup param")?);
    }
    let retyped = if request.action == ParameterAction::Set {
        changed_bindings.clone()
    } else {
        Vec::new()
    };
    let removed = if request.action == ParameterAction::Remove {
        changed_bindings
    } else {
        Vec::new()
    };
    Ok(ParameterPlan {
        edits,
        action: request.action,
        mashup: request.mashup.clone(),
        name: request.name.clone(),
        base_type: definition.base_type,
        direction: definition.direction,
        refreshed,
        retyped,
        removed,
    })
}

fn has_container(value: &Value, name: &str) -> bool {
    fn visit(widget: &Value, name: &str) -> bool {
        (widget["Properties"]["Type"] == "mashupcontainer" && widget["Properties"]["Name"] == name)
            || widget["Widgets"]
                .as_array()
                .is_some_and(|children| children.iter().any(|child| visit(child, name)))
    }
    visit(&value["UI"], name)
}

/// Apply every affected mashup in one transaction.
pub fn apply_parameter(
    solution: &Solution,
    plan: &ParameterPlan,
    lock: &WorkspaceLock,
) -> Result<(), AuthoringError> {
    let mut transaction = Transaction::new(&solution.root, "mashup param");
    for edit in &plan.edits {
        transaction
            .replace_file(
                &edit.entity_path,
                &edit.entity_before,
                edit.entity_after.clone(),
            )
            .map_err(AuthoringError::Write)?;
        if let Some(sidecars) = &edit.sidecars {
            transaction
                .replace_file(
                    &sidecars.content_path,
                    &sidecars.content_before,
                    sidecars.content_after.clone(),
                )
                .map_err(AuthoringError::Write)?;
        }
    }
    transaction.apply(lock).map_err(AuthoringError::Write)?;
    Ok(())
}

pub fn parameter_plan_json(solution: &Solution, plan: &ParameterPlan, applied: bool) -> Value {
    let files = plan
        .edits
        .iter()
        .flat_map(|edit| {
            std::iter::once(&edit.entity_path)
                .chain(edit.sidecars.iter().map(|sidecar| &sidecar.content_path))
        })
        .map(|path| {
            path.strip_prefix(&solution.root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();
    json!({ "applied": applied, "action": format!("{:?}", plan.action).to_ascii_lowercase(), "mashup": plan.mashup, "name": plan.name, "base_type": plan.base_type, "direction": plan.direction, "containers_refreshed": plan.refreshed, "bindings_retyped": plan.retyped, "bindings_removed": plan.removed, "files": files })
}

fn unknown_template(word: &str) -> String {
    let names = mashup_templates::layouts()
        .expect("embedded templates parse; a test checks them")
        .into_iter()
        .map(|template| template.name)
        .collect::<Vec<_>>()
        .join(", ");
    format!("unknown template `{word}`; use {names}")
}

fn add_template_node(
    document: &mut MashupDocument,
    parent: &[usize],
    node: &mashup_templates::LayoutNode,
    template: &str,
) -> Result<(), AuthoringError> {
    validate_node(node, template)?;
    let container = widgets::catalog()
        .by_type("flexcontainer")
        .expect("the catalogue has the flex container; a test checks it");
    let number = document.allocate();
    document.push_child(parent, mashup_edit::new_widget(container, number, true));
    let child = document
        .find_exact(&format!("flexcontainer-{number}"))
        .map_err(AuthoringError::Edit)?;
    let name = if node.divider {
        format!("divider-{number}")
    } else {
        node.slot.clone().expect("validated slot")
    };
    document.set_property(&child.path, "DisplayName", json!(name));
    for (key, value) in &node.properties {
        document.set_property(&child.path, key, value.clone());
    }
    if let Some(fixed) = node.fixed.as_deref().or(node.divider.then_some("1px")) {
        document.set_property(&child.path, "flex-size", json!("fixed"));
        document.set_property(
            &child.path,
            "iconClass",
            json!("widgets-flexcontainer-fixed"),
        );
        let dimension = match document.at(parent)["Properties"]["flex-direction"].as_str() {
            Some("row") => "width",
            _ => "height",
        };
        document.set_property(&child.path, &format!("flex-min-{dimension}"), json!(fixed));
        document.set_property(&child.path, &format!("flex-max-{dimension}"), json!(fixed));
    }
    if let Some(type_id) = &node.widget {
        let widget = widgets::catalog().by_type(type_id).ok_or_else(|| {
            AuthoringError::Invalid(format!(
                "template {template}: unknown widget type {type_id}"
            ))
        })?;
        let number = document.allocate();
        document.push_child(&child.path, mashup_edit::new_widget(widget, number, true));
    }
    for grandchild in &node.children {
        add_template_node(document, &child.path, grandchild, template)?;
    }
    Ok(())
}

fn validate_node(
    node: &mashup_templates::LayoutNode,
    template: &str,
) -> Result<(), AuthoringError> {
    if !node.divider && node.slot.as_deref().is_none_or(str::is_empty) {
        return Err(AuthoringError::Invalid(format!(
            "template {template}: a node needs slot"
        )));
    }
    if node.widget.is_some() && !node.children.is_empty() {
        return Err(AuthoringError::Invalid(format!(
            "template {template}: a node cannot have both widget and children"
        )));
    }
    for key in node.properties.keys() {
        if [
            "Id",
            "Type",
            "DisplayName",
            "ResponsiveLayout",
            "LastContainer",
        ]
        .contains(&key.as_str())
        {
            return Err(AuthoringError::Invalid(format!(
                "template {template}: {key} is not a layout property"
            )));
        }
    }
    if let Some(type_id) = &node.widget {
        if widgets::catalog().by_type(type_id).is_none() {
            return Err(AuthoringError::Invalid(format!(
                "template {template}: unknown widget type {type_id}"
            )));
        }
    }
    Ok(())
}

/// Validate an embedded template before it becomes a Composer document.
pub fn validate_template(
    template: &mashup_templates::LayoutTemplate,
) -> Result<(), AuthoringError> {
    for key in template.page.keys() {
        if [
            "Id",
            "Type",
            "DisplayName",
            "ResponsiveLayout",
            "LastContainer",
        ]
        .contains(&key.as_str())
        {
            return Err(AuthoringError::Invalid(format!(
                "template {}: {key} is not a layout property",
                template.name
            )));
        }
    }
    let mut slots = std::collections::BTreeSet::new();
    fn walk(
        nodes: &[mashup_templates::LayoutNode],
        template: &str,
        slots: &mut std::collections::BTreeSet<String>,
    ) -> Result<(), AuthoringError> {
        for node in nodes {
            validate_node(node, template)?;
            if let Some(slot) = &node.slot {
                if !slots.insert(slot.clone()) {
                    return Err(AuthoringError::Invalid(format!(
                        "template {template}: slot {slot} appears more than once"
                    )));
                }
            }
            walk(&node.children, template, slots)?;
        }
        Ok(())
    }
    walk(&template.children, &template.name, &mut slots)?;
    if !template.children.is_empty()
        && !template
            .master_slot
            .as_ref()
            .is_some_and(|slot| slots.contains(slot))
    {
        return Err(AuthoringError::Invalid(format!(
            "template {}: master_slot must name a slot",
            template.name
        )));
    }
    if template.children.is_empty() && template.master_slot.as_deref() != Some("center") {
        return Err(AuthoringError::Invalid(format!(
            "template {}: blank master_slot must be center",
            template.name
        )));
    }
    let document = new_document(template, false, DEFAULT_THEME)?;
    if !document.problems().is_empty() {
        return Err(AuthoringError::Invalid(format!(
            "template {}: generated document has problems",
            template.name
        )));
    }
    Ok(())
}

/// Write the planned files, all or none.
pub fn apply_new(
    solution: &Solution,
    plan: &NewPlan,
    lock: &WorkspaceLock,
) -> Result<(), AuthoringError> {
    let mut transaction = Transaction::new(&solution.root, "mashup new");
    for (path, bytes) in [
        (&plan.entity_path, plan.entity.clone()),
        (&plan.content_path, plan.assets.content.clone().into_bytes()),
        (&plan.css_path, plan.assets.css.clone().into_bytes()),
    ] {
        transaction
            .create_file(path, bytes)
            .map_err(AuthoringError::Write)?;
    }
    transaction.apply(lock).map_err(AuthoringError::Write)?;
    Ok(())
}

/// The plan as `--json` and MCP report it.
pub fn new_plan_json(solution: &Solution, plan: &NewPlan, applied: bool) -> Value {
    let relative = |path: &Path| {
        path.strip_prefix(&solution.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    json!({
        "applied": applied,
        "mashup": plan.name,
        "project": plan.project,
        "template": plan.template.name,
        "layout": plan.template.name,
        "slots": template_slots(&plan.template),
        "master": plan.master,
        "theme": plan.theme,
        "visibility": plan.visibility.iter().map(|v| format!("{}/{}", v.kind, v.name)).collect::<Vec<_>>(),
        "files": [relative(&plan.entity_path), relative(&plan.content_path), relative(&plan.css_path)],
        "widgets": plan.widgets.iter().map(|w| json!({
            "id": w.id,
            "type": w.type_id,
            "display_name": w.display_name,
        })).collect::<Vec<_>>(),
        "menus": plan.menus.iter().map(|(service, property)| json!({
            "service": service,
            "property": property,
        })).collect::<Vec<_>>(),
    })
}

/// Slot names in the order a person sees them in the generated document.
pub fn template_slots(template: &mashup_templates::LayoutTemplate) -> Vec<String> {
    fn collect(nodes: &[mashup_templates::LayoutNode], out: &mut Vec<String>) {
        for node in nodes {
            if let Some(slot) = &node.slot {
                out.push(slot.clone());
            }
            collect(&node.children, out);
        }
    }
    let mut slots = Vec::new();
    collect(&template.children, &mut slots);
    if slots.is_empty() {
        slots.push("center".into());
    }
    slots
}

/// Plan adding one catalogue widget to an existing mashup. Nothing is written.
pub fn plan_add(solution: &Solution, request: &AddRequest) -> Result<AddPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let widget = widgets::catalog()
        .resolve(&request.widget)
        .map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    let placement = match &request.placement {
        Some(placement) => placement.clone(),
        None if document.find("center").is_ok() => Placement::In("center".to_string()),
        None => {
            return Err(AuthoringError::Invalid(
                "say where: --in <container>, or --right-of/--left-of/--above/--below <widget>"
                    .to_string(),
            ))
        }
    };
    let placed = document
        .place(widget, &placement)
        .map_err(AuthoringError::Edit)?;
    if let Some(label) = &request.label {
        let property = ["LabelText", "Label", "Text", "Title"]
            .into_iter()
            .find(|name| widget.defaults.contains_key(*name))
            .ok_or_else(|| {
                AuthoringError::Invalid(format!(
                    "{} has no label property; use --prop",
                    widget.type_id
                ))
            })?;
        let path = document
            .find(&placed.id)
            .map_err(AuthoringError::Edit)?
            .path;
        document.set_property(&path, property, Value::String(label.clone()));
    }
    for (name, text) in &request.properties {
        if [
            "Id",
            "Type",
            "DisplayName",
            "__TypeDisplayName",
            "LastContainer",
            "ResponsiveLayout",
        ]
        .contains(&name.as_str())
        {
            return Err(AuthoringError::Invalid(format!("twaco sets {name}")));
        }
        let default = widget.defaults.get(name).ok_or_else(|| {
            AuthoringError::Invalid(format!("{} has no property {name}", widget.type_id))
        })?;
        let value = property_value(name, text, default)?;
        if let Some(contract) = widget.property(name) {
            if !contract.options.is_empty() && !contract.options.contains(&value) {
                return Err(AuthoringError::Invalid(format!(
                    "{name} for {} must be one of {}",
                    widget.type_id,
                    contract
                        .options
                        .iter()
                        .map(Value::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        let path = document
            .find(&placed.id)
            .map_err(AuthoringError::Edit)?
            .path;
        document.set_property(&path, name, value);
    }
    Ok(AddPlan {
        edit: finish_edit(loaded, document, "mashup add")?,
        placed,
        widget: widget.type_id.clone(),
    })
}

/// Plan adding a child mashup as Composer's contained-mashup builder does.  The child is read
/// from the repository, never from a server, so the cache is deterministic and reviewable.
pub fn plan_add_contained(
    solution: &Solution,
    request: &AddContainedRequest,
) -> Result<AddContainedPlan, AuthoringError> {
    if request.mashup == request.child {
        return Err(AuthoringError::Refused(
            "a mashup cannot contain itself".into(),
        ));
    }
    let loaded = load_existing(solution, &request.mashup)?;
    let child = load_existing(solution, &request.child)?;
    let child_document = parse_loaded(&child)?;
    if child_document.value()["mashupType"] == "targetmashup" {
        return Err(AuthoringError::Refused(
            "a master mashup is shown by a page container, not contained".into(),
        ));
    }
    let definitions = contained_parameters(&child, &child_document)?;
    let placement = match &request.placement {
        Some(placement) => placement.clone(),
        None if parse_loaded(&loaded)?.find("center").is_ok() => Placement::In("center".into()),
        None => {
            return Err(AuthoringError::Invalid(
                "say where: --in <container>, or --right-of/--left-of/--above/--below <widget>"
                    .into(),
            ))
        }
    };
    let mut document = parse_loaded(&loaded)?;
    let widget = widgets::catalog()
        .by_type("mashupcontainer")
        .expect("the embedded catalogue has mashupcontainer");
    let placed = document
        .place(widget, &placement)
        .map_err(AuthoringError::Edit)?;
    let path = document
        .find(&placed.id)
        .map_err(AuthoringError::Edit)?
        .path;
    document.set_property(&path, "Name", json!(request.child));
    document.set_property(
        &path,
        "MashupParameters",
        Value::Array(
            definitions
                .iter()
                .map(|def| cache_entry(def, def.default.clone()))
                .collect(),
        ),
    );
    let visible = definitions
        .iter()
        .filter(|definition| definition.visible && definition.direction != "OUT")
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    let mut settings = Vec::new();
    for (name, text) in &request.settings {
        let definition = definitions
            .iter()
            .find(|definition| definition.name == *name)
            .ok_or_else(|| {
                AuthoringError::Invalid(format!(
                    "{} has no settable parameter {name}; settable parameters are {}",
                    request.child,
                    visible.join(", ")
                ))
            })?;
        if !definition.visible || definition.direction == "OUT" {
            return Err(AuthoringError::Invalid(format!(
                "{}.{name} is not a settable parameter; settable parameters are {}",
                request.child,
                visible.join(", ")
            )));
        }
        let value = default_value(&definition.base_type, text)?;
        document.set_property(&path, name, value.clone());
        let properties = document.at_mut(&path)["Properties"]
            .as_object_mut()
            .expect("a placed widget has properties");
        if let Some(entry) = properties["MashupParameters"]
            .as_array_mut()
            .and_then(|entries| {
                entries
                    .iter_mut()
                    .find(|entry| entry["ParameterName"] == *name)
            })
        {
            mashup_edit::insert_sorted(
                entry.as_object_mut().expect("cache entries are objects"),
                "DefaultValue",
                value,
            );
        }
        settings.push(name.clone());
    }
    Ok(AddContainedPlan {
        edit: finish_edit(loaded, document, "mashup add-contained")?,
        placed,
        child: request.child.clone(),
        parameters: definitions
            .iter()
            .map(|definition| ContainedParameter {
                name: definition.name.clone(),
                base_type: definition.base_type.clone(),
                direction: definition.direction.clone(),
            })
            .collect(),
        settings,
    })
}

/// Content metadata is Composer's current definition; an older/unmanaged mashup can have only
/// XML definitions, which remain a supported fallback.
fn contained_parameters(
    child: &ExistingMashup,
    document: &MashupDocument,
) -> Result<Vec<ParameterDefinition>, AuthoringError> {
    let mut definitions = document_parameters(document);
    if definitions.is_empty() {
        definitions = mashup_parameters(&child.entity_before)
            .map_err(AuthoringError::Invalid)?
            .into_iter()
            .map(|parameter| ParameterDefinition {
                visible: !parameter.name.starts_with("ConfigForm_"),
                name: parameter.name,
                base_type: parameter.base_type,
                direction: if parameter.direction.is_empty() {
                    "BOTH".into()
                } else {
                    parameter.direction
                },
                description: String::new(),
                default: None,
                data_shape: None,
            })
            .collect();
    }
    if matches!(
        document.value()["mashupType"].as_str(),
        Some("thingtemplatemashup" | "thingshapemashup")
    ) {
        definitions.push(ParameterDefinition {
            name: "Entity".into(),
            base_type: "THINGNAME".into(),
            direction: "BOTH".into(),
            description: String::new(),
            default: None,
            data_shape: None,
            visible: true,
        });
    }
    Ok(definitions)
}

/// Load one existing mashup, refusing a managed sidecar that is not its entity's current content.
fn load_existing(solution: &Solution, mashup_name: &str) -> Result<ExistingMashup, AuthoringError> {
    let entity = workspace::entities(solution)
        .into_iter()
        .find(|entity| entity.info.collection == "Mashups" && entity.info.name == mashup_name)
        .ok_or_else(|| {
            AuthoringError::Unknown(format!("no mashup {mashup_name} in the solution"))
        })?;
    let entity_before = std::fs::read(&entity.path).map_err(|error| {
        AuthoringError::Unreadable(format!("{}: {error}", entity.path.display()))
    })?;
    let directory = workspace::mashup_dir(solution, &entity);
    let sidecar_assets = workspace::read_mashup(&directory)
        .map_err(|error| AuthoringError::Unreadable(error.to_string()))?;
    let (assets, sidecars) = match sidecar_assets {
        Some(assets) => {
            let (_, changes) =
                mashup::sync(&entity_before, &assets).map_err(AuthoringError::Mashup)?;
            if !changes.is_empty() {
                return Err(AuthoringError::Refused(format!(
                    "{mashup_name}: content.json and the entity file disagree; run `twaco sync` (keeps the sidecars) or `twaco extract` (keeps the entity file) first"
                )));
            }
            let content_path = directory.join("content.json");
            let content_before = std::fs::read(&content_path).map_err(|error| {
                AuthoringError::Unreadable(format!("{}: {error}", content_path.display()))
            })?;
            (
                assets,
                Some((content_path, content_before, directory.join("custom.css"))),
            )
        }
        None => (
            mashup::extract(&entity_before).map_err(AuthoringError::Mashup)?,
            None,
        ),
    };
    Ok(ExistingMashup {
        mashup: mashup_name.to_string(),
        entity_path: entity.path,
        entity_before,
        assets,
        sidecars,
    })
}

/// The loaded mashup's content as an editable document; a content twaco cannot edit names its mashup.
fn parse_loaded(loaded: &ExistingMashup) -> Result<MashupDocument, AuthoringError> {
    mashup_edit::parse(&loaded.assets.content).map_err(|error| match error {
        EditError::Invalid(message) => {
            AuthoringError::Edit(EditError::Invalid(format!("{}: {message}", loaded.mashup)))
        }
        other => AuthoringError::Edit(other),
    })
}

/// Put the changed content back through sync and prove that the entity holds precisely that content.
fn finish_edit(
    loaded: ExistingMashup,
    document: MashupDocument,
    command: &'static str,
) -> Result<EditPlan, AuthoringError> {
    let content = document.to_content();
    let assets_after = mashup::Assets {
        content: content.clone(),
        css: loaded.assets.css.clone(),
    };
    let (entity_after, _) =
        mashup::sync(&loaded.entity_before, &assets_after).map_err(AuthoringError::Mashup)?;
    if mashup::extract(&entity_after)
        .map_err(AuthoringError::Mashup)?
        .content
        != content
    {
        return Err(AuthoringError::Invalid(format!(
            "{}: mashup content did not round-trip through the entity file",
            loaded.mashup
        )));
    }
    let sidecars = loaded
        .sidecars
        .map(|(content_path, content_before, css_path)| SidecarEdit {
            content_path,
            content_before,
            content_after: content.into_bytes(),
            css_path,
            css: loaded.assets.css,
        });
    Ok(EditPlan {
        command,
        mashup: loaded.mashup,
        entity_path: loaded.entity_path,
        entity_before: loaded.entity_before,
        entity_after,
        sidecars,
    })
}

#[derive(Clone)]
enum End {
    Ui {
        id: String,
        property: String,
        base: String,
    },
    Mashup {
        parameter: String,
        base: String,
    },
    Data {
        entity: String,
        collection: String,
        service: String,
        requested_alias: Option<String>,
        selected_rows: bool,
        member: Option<String>,
        base: String,
        property_type: String,
    },
}

/// Plan one Composer-format data binding.  Calls and sections are created only when an address
/// needs one; the entity content and managed sidecar then travel through the usual edit path.
pub fn plan_bind(solution: &Solution, request: &BindRequest) -> Result<BindPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let parameters =
        mashup_parameters(&loaded.entity_before).map_err(AuthoringError::Unreadable)?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let context = AuthoringContext {
        parameters: &parameters,
        model: &model,
        platform: platform.as_ref(),
    };
    let result = bind_in(
        &mut document,
        Some(solution),
        &context,
        &request.from,
        &request.to,
    )?;
    let edit = finish_edit(loaded, document, "mashup bind")?;
    Ok(BindPlan {
        edit,
        from: request.from.clone(),
        to: request.to.clone(),
        new_source: result.new_source,
        warnings: result.warnings,
        refreshed: result.refreshed,
    })
}

struct AuthoringContext<'a> {
    parameters: &'a [crate::core::mashup_describe::MashupParameter],
    model: &'a types::Model,
    platform: Option<&'a types::Platform>,
}

struct BindIn {
    new_source: Option<String>,
    warnings: Vec<String>,
    refreshed: Vec<String>,
}

/// Add one checked binding to an already loaded document.
fn bind_in(
    document: &mut MashupDocument,
    solution: Option<&Solution>,
    context: &AuthoringContext<'_>,
    from_raw: &str,
    to_raw: &str,
) -> Result<BindIn, AuthoringError> {
    let from_address =
        parse_address(from_raw).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let to_address = parse_address(to_raw).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let mut refreshed = Vec::new();
    let from = resolve_end(
        document,
        solution,
        context,
        from_address,
        true,
        &mut refreshed,
    )?;
    let to = resolve_end(
        document,
        solution,
        context,
        to_address,
        false,
        &mut refreshed,
    )?;
    if !allowed(&from, &to) {
        return Err(AuthoringError::Invalid(format!(
            "{} cannot bind to {}",
            from_raw, to_raw
        )));
    }
    let from_base = base(&from);
    let to_base = base(&to);
    if from_base != to_base && to_base != "STRING" {
        return Err(AuthoringError::Refused(format!(
            "{} is {} but {} is {}",
            from_raw, from_base, to_raw, to_base
        )));
    }
    let (source_section, source_id, source_added) = data_call(document, &from)?;
    let (target_section, target_id, target_added) = data_call(document, &to)?;
    if binding_exists(
        document,
        &from,
        &to,
        &source_section,
        &source_id,
        &target_section,
        &target_id,
    ) {
        return Err(AuthoringError::Exists(format!(
            "{} is already bound to {}",
            from_raw, to_raw
        )));
    }
    let warnings = if has_source(document, &to, &target_section, &target_id) {
        vec![format!(
            "{} already has another source; ThingWorx applies whichever value arrives last",
            to_raw
        )]
    } else {
        Vec::new()
    };
    let mut binding = Map::new();
    insert(
        &mut binding,
        "Id",
        Value::String(Uuid::new_v4().to_string()),
    );
    let mut map = Map::new();
    insert(
        &mut map,
        "SourceProperty",
        Value::String(property(&from).to_string()),
    );
    insert(
        &mut map,
        "SourcePropertyBaseType",
        Value::String(from_base.to_string()),
    );
    insert(
        &mut map,
        "SourcePropertyType",
        Value::String(source_property_type(&from).to_string()),
    );
    insert(
        &mut map,
        "TargetProperty",
        Value::String(property(&to).to_string()),
    );
    insert(
        &mut map,
        "TargetPropertyBaseType",
        Value::String(to_base.to_string()),
    );
    insert(
        &mut map,
        "TargetPropertyType",
        Value::String(target_property_type(&to).to_string()),
    );
    insert(
        &mut binding,
        "PropertyMaps",
        Value::Array(vec![Value::Object(map)]),
    );
    insert(
        &mut binding,
        "SourceArea",
        Value::String(area(&from).into()),
    );
    insert(
        &mut binding,
        "SourceDetails",
        Value::String(details(&from).into()),
    );
    insert(&mut binding, "SourceId", Value::String(source_id));
    insert(&mut binding, "SourceSection", Value::String(source_section));
    insert(&mut binding, "TargetArea", Value::String(area(&to).into()));
    insert(&mut binding, "TargetId", Value::String(target_id));
    insert(&mut binding, "TargetSection", Value::String(target_section));
    let root = document
        .value_mut()
        .as_object_mut()
        .expect("checked: root object");
    root.entry("DataBindings")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("checked: bindings array")
        .push(Value::Object(binding));
    let new_source = source_added.or(target_added);
    Ok(BindIn {
        new_source,
        warnings,
        refreshed,
    })
}

/// Plan one event handler, adding a Thing service call for a data handler when needed.
pub fn plan_on(solution: &Solution, request: &OnRequest) -> Result<OnPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let parameters = Vec::new();
    let context = AuthoringContext {
        parameters: &parameters,
        model: &model,
        platform: platform.as_ref(),
    };
    let new_source = on_in(&mut document, &context, &request.event, &request.invoke)?;
    let edit = finish_edit(loaded, document, "mashup on")?;
    Ok(OnPlan {
        edit,
        event: request.event.clone(),
        invoke: request.invoke.clone(),
        new_source,
    })
}

/// Add one checked event handler to an already loaded document.
fn on_in(
    document: &mut MashupDocument,
    context: &AuthoringContext<'_>,
    event_raw: &str,
    invoke_raw: &str,
) -> Result<Option<String>, AuthoringError> {
    let trigger = resolve_trigger(document, context.model, context.platform, event_raw)?;
    let handler = resolve_handler(document, context.model, context.platform, invoke_raw)?;
    let (handler_section, handler_id, new_source) = data_call(document, &handler)?;
    // A data handler is addressed by its section and the call's alias; a widget handler by the
    // widget's id and the widget's own service.
    let (handler_section, handler_service) = if matches!(handler, End::Data { .. }) {
        (handler_section, handler_id.clone())
    } else {
        (handler_id.clone(), handler_service(&handler).to_string())
    };
    if event_exists(
        document,
        &trigger,
        area(&handler),
        &handler_section,
        &handler_service,
    ) {
        return Err(AuthoringError::Exists(format!(
            "{} already invokes {}",
            event_raw, invoke_raw
        )));
    }
    let mut event = Map::new();
    insert(
        &mut event,
        "EventHandlerArea",
        Value::String(area(&handler).into()),
    );
    insert(&mut event, "EventHandlerId", Value::String(handler_section));
    insert(
        &mut event,
        "EventHandlerService",
        Value::String(handler_service),
    );
    insert(
        &mut event,
        "EventHandlerServiceMethod",
        Value::String(String::new()),
    );
    insert(
        &mut event,
        "EventTriggerArea",
        Value::String(trigger.area.into()),
    );
    insert(
        &mut event,
        "EventTriggerEvent",
        Value::String(trigger.event),
    );
    insert(&mut event, "EventTriggerId", Value::String(trigger.id));
    insert(
        &mut event,
        "EventTriggerSection",
        Value::String(trigger.section),
    );
    insert(&mut event, "Id", Value::String(Uuid::new_v4().to_string()));
    let root = document
        .value_mut()
        .as_object_mut()
        .expect("checked: root object");
    root.entry("Events")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("checked: events array")
        .push(Value::Object(event));
    Ok(new_source)
}

struct Trigger {
    area: &'static str,
    section: String,
    id: String,
    event: String,
}

fn resolve_trigger(
    document: &MashupDocument,
    model: &types::Model,
    platform: Option<&types::Platform>,
    raw: &str,
) -> Result<Trigger, AuthoringError> {
    if raw == "mashup:Loaded" {
        return Ok(Trigger {
            area: "Mashup",
            section: String::new(),
            id: mashup_edit::ROOT_ID.into(),
            event: "Loaded".into(),
        });
    }
    if raw.starts_with("mashup:") {
        return Err(AuthoringError::Invalid("only mashup:Loaded".into()));
    }
    if let Some(rest) = raw.strip_prefix("ui:") {
        let (widget, event) = rest
            .rsplit_once('.')
            .ok_or_else(|| AuthoringError::Invalid(format!("invalid mashup address `{raw}`")))?;
        let found = document.find_exact(widget).map_err(AuthoringError::Edit)?;
        if found.type_id == "mashupcontainer" {
            return Err(AuthoringError::Invalid(
                "a contained mashup has no events; see twaco mashup event (planned)".into(),
            ));
        }
        let catalog = event_contract(&found.id, &found.type_id)?;
        if !catalog.has_event(event) {
            return Err(AuthoringError::Invalid(format!(
                "widget `{}` has no event {}; its events are {}",
                found.id,
                event,
                catalog
                    .events
                    .as_ref()
                    .map_or(String::new(), |events| events.join(", "))
            )));
        }
        return Ok(Trigger {
            area: "UI",
            section: String::new(),
            id: found.id,
            event: event.into(),
        });
    }
    let rest = raw
        .strip_prefix("data:")
        .ok_or_else(|| AuthoringError::Invalid(format!("invalid mashup address `{raw}`")))?;
    let (call, event) = rest
        .rsplit_once('.')
        .ok_or_else(|| AuthoringError::Invalid(format!("invalid mashup address `{raw}`")))?;
    if !matches!(
        event,
        "ServiceInvokeCompleted" | "AllDataChanged" | "SelectedRowsChanged"
    ) {
        return Err(AuthoringError::Invalid(format!(
            "invalid mashup event `{raw}`"
        )));
    }
    let address = parse_address(&format!("data:{call}"))
        .map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let end = resolve_data_call(model, platform, address)?;
    let (section, id) = existing_data_call(document, &end)?
        .ok_or_else(|| AuthoringError::Invalid(format!("{call} is not a call this mashup has")))?;
    Ok(Trigger {
        area: "Data",
        section,
        id,
        event: event.into(),
    })
}

fn resolve_handler(
    document: &MashupDocument,
    model: &types::Model,
    platform: Option<&types::Platform>,
    raw: &str,
) -> Result<End, AuthoringError> {
    if let Some(rest) = raw.strip_prefix("ui:") {
        let (widget, service) = rest
            .rsplit_once('.')
            .ok_or_else(|| AuthoringError::Invalid(format!("invalid mashup address `{raw}`")))?;
        let found = document.find_exact(widget).map_err(AuthoringError::Edit)?;
        if found.type_id == "mashupcontainer" {
            if service != "Refresh" {
                return Err(AuthoringError::Invalid(format!(
                    "a contained mashup has no service {service}; only Refresh is supported"
                )));
            }
            return Ok(End::Ui {
                id: found.id,
                property: service.into(),
                base: String::new(),
            });
        }
        let catalog = event_contract(&found.id, &found.type_id)?;
        if !catalog
            .services
            .as_ref()
            .is_some_and(|services| services.iter().any(|known| known == service))
        {
            return Err(AuthoringError::Invalid(format!(
                "widget `{}` has no service {service}",
                found.id
            )));
        }
        return Ok(End::Ui {
            id: found.id,
            property: service.into(),
            base: String::new(),
        });
    }
    let address = parse_address(raw).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    resolve_data_call(model, platform, address)
}

fn event_contract<'a>(id: &str, type_id: &str) -> Result<&'a widgets::Widget, AuthoringError> {
    widgets::catalog().by_type(type_id).filter(|widget| widget.has_contract()).ok_or_else(|| AuthoringError::Refused(format!(
        "widget `{id}` has type `{type_id}`, whose contract is not in the embedded ThingWorx 10.2 catalogue; twaco will not create an event for it"
    )))
}

fn resolve_data_call(
    model: &types::Model,
    platform: Option<&types::Platform>,
    address: Address,
) -> Result<End, AuthoringError> {
    let Address::Data {
        entity_service,
        alias,
        selected_rows,
        member,
    } = address
    else {
        return Err(AuthoringError::Invalid(
            "event data addresses name a service call".into(),
        ));
    };
    if selected_rows || member.is_some() {
        return Err(AuthoringError::Invalid(
            "event data addresses name a service call".into(),
        ));
    }
    let (entity, service) = resolve_service(model, platform, &entity_service)?;
    Ok(End::Data {
        entity: entity.name.clone(),
        collection: entity.collection.clone(),
        service,
        requested_alias: alias,
        selected_rows: false,
        member: None,
        base: String::new(),
        property_type: String::new(),
    })
}

fn handler_service(end: &End) -> &str {
    match end {
        End::Data {
            requested_alias,
            service,
            ..
        } => requested_alias.as_deref().unwrap_or(service),
        End::Ui { property, .. } => property,
        End::Mashup { .. } => unreachable!("mashup is not an event handler"),
    }
}

fn event_exists(
    document: &MashupDocument,
    trigger: &Trigger,
    handler_area: &str,
    handler_id: &str,
    handler_service: &str,
) -> bool {
    document.value()["Events"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|event| {
            event["EventTriggerArea"] == trigger.area
                && event["EventTriggerSection"] == trigger.section
                && event["EventTriggerId"] == trigger.id
                && event["EventTriggerEvent"] == trigger.event
                && event["EventHandlerArea"] == handler_area
                && event["EventHandlerId"] == handler_id
                && event["EventHandlerService"] == handler_service
        })
}

fn insert(map: &mut Map<String, Value>, key: &str, value: Value) {
    mashup_edit::insert_sorted(map, key, value);
}

fn resolve_end(
    document: &mut MashupDocument,
    solution: Option<&Solution>,
    context: &AuthoringContext<'_>,
    address: Address,
    source: bool,
    refreshed: &mut Vec<String>,
) -> Result<End, AuthoringError> {
    let (parameters, model, platform) = (context.parameters, context.model, context.platform);
    match address {
        Address::Ui { widget, property } => {
            let found = document.find_exact(&widget).map_err(AuthoringError::Edit)?;
            if found.type_id == "mashupcontainer" {
                return resolve_container_end(
                    document, solution, found, property, source, refreshed,
                );
            }
            let catalog = widgets::catalog().by_type(&found.type_id).filter(|w| w.has_contract()).ok_or_else(|| AuthoringError::Refused(format!("widget `{}` has type `{}`, whose contract is not in the embedded ThingWorx 10.2 catalogue; twaco will not create a binding for it", found.id, found.type_id)))?;
            let contract = catalog.property(&property).ok_or_else(|| {
                AuthoringError::Unknown(format!("{} has no property {property}", found.id))
            })?;
            let valid = if source {
                contract.source
            } else {
                contract.target
            };
            if !valid {
                return Err(AuthoringError::Refused(format!(
                    "ui:{}.{property} is not a {} property",
                    found.id,
                    if source { "source" } else { "target" }
                )));
            }
            Ok(End::Ui {
                id: found.id,
                property,
                base: contract.base_type.clone().unwrap_or_default(),
            })
        }
        Address::Mashup { parameter } => {
            let found = parameters
                .iter()
                .find(|p| p.name == parameter)
                .ok_or_else(|| {
                    AuthoringError::Unknown(format!("mashup has no parameter {parameter}"))
                })?;
            Ok(End::Mashup {
                parameter,
                base: found.base_type.clone(),
            })
        }
        Address::Data {
            entity_service,
            alias,
            selected_rows,
            member,
        } => {
            if !source && selected_rows {
                return Err(AuthoringError::Invalid(
                    "SelectedRows is a data source, not a parameter target".into(),
                ));
            }
            let (entity, service) = resolve_service(model, platform, &entity_service)?;
            let found = catalog::lookup_service(entity, &model.entities, platform, &service)
                .service
                .expect("resolved above");
            let member = member.or_else(|| {
                entity_service
                    .strip_prefix(&format!("{}.{}.", entity.name, service))
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
            });
            if source {
                let (base, property_type) = match member.as_deref() {
                    None => (found.result.base_type.clone(), "InfoTable".to_string()),
                    Some(name) => {
                        // A result field may share a parameter's name; the field is the source.
                        let shape = found.result.data_shape.as_deref().map(|shape_name| {
                            (
                                shape_name,
                                model
                                    .data_shapes
                                    .iter()
                                    .find(|shape| shape.name == shape_name),
                            )
                        });
                        let field = shape
                            .and_then(|(_, shape)| shape)
                            .and_then(|shape| shape.fields.iter().find(|field| field.name == name));
                        match (field, shape) {
                            (Some(field), _) => (field.base_type.clone(), "Field".to_string()),
                            _ if found.parameters.iter().any(|p| p.name == name) => {
                                return Err(AuthoringError::Invalid(format!(
                                    "data:{entity_service}.{name} is a parameter, not a data source"
                                )));
                            }
                            (None, None) => {
                                return Err(AuthoringError::Refused(format!(
                                    "{service} has no result DataShape"
                                )));
                            }
                            (None, Some((shape_name, None))) => {
                                return Err(AuthoringError::Refused(format!(
                                    "result DataShape {shape_name} is not in the repository"
                                )));
                            }
                            (None, Some((shape_name, Some(_)))) => {
                                return Err(AuthoringError::Refused(format!(
                                    "result DataShape {shape_name} has no field {name}"
                                )));
                            }
                        }
                    }
                };
                Ok(End::Data {
                    entity: entity.name.clone(),
                    collection: entity.collection.clone(),
                    service,
                    requested_alias: alias,
                    selected_rows,
                    member,
                    base,
                    property_type,
                })
            } else {
                let name = member.ok_or_else(|| {
                    AuthoringError::Invalid(format!(
                        "data:{entity_service} needs a parameter target"
                    ))
                })?;
                let parameter = found
                    .parameters
                    .iter()
                    .find(|p| p.name == name)
                    .ok_or_else(|| {
                        AuthoringError::Unknown(format!(
                            "{}.{} has no parameter {}",
                            entity.name, service, name
                        ))
                    })?;
                Ok(End::Data {
                    entity: entity.name.clone(),
                    collection: entity.collection.clone(),
                    service,
                    requested_alias: alias,
                    selected_rows,
                    member: Some(name),
                    base: parameter.value.base_type.clone(),
                    property_type: "Parameter".into(),
                })
            }
        }
    }
}

fn resolve_container_end(
    document: &mut MashupDocument,
    solution: Option<&Solution>,
    found: mashup_edit::WidgetRef,
    property: String,
    source: bool,
    refreshed: &mut Vec<String>,
) -> Result<End, AuthoringError> {
    let child = document.at(&found.path)["Properties"]["Name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            AuthoringError::Refused(format!(
                "contained mashup `{}` has no child mashup",
                found.id
            ))
        })?
        .to_string();
    let solution = solution.ok_or_else(|| {
        AuthoringError::Refused(format!(
            "contained mashup `{}` cannot be resolved without the repository",
            found.id
        ))
    })?;
    let loaded = load_existing(solution, &child).map_err(|error| match error {
        AuthoringError::Unknown(_) => AuthoringError::Refused(format!(
            "contained mashup `{}` shows {child}, which is not in the repository",
            found.id
        )),
        other => other,
    })?;
    let child_document = parse_loaded(&loaded)?;
    let definitions = contained_parameters(&loaded, &child_document)?;
    if property == "Name" {
        if source {
            return Err(AuthoringError::Refused(format!(
                "ui:{}.Name is a target-only MASHUPNAME property",
                found.id
            )));
        }
        return Ok(End::Ui {
            id: found.id,
            property,
            base: "MASHUPNAME".into(),
        });
    }
    let definition = definitions.iter().find(|definition| definition.name == property && definition.visible)
        .ok_or_else(|| AuthoringError::Unknown(format!(
            "contained mashup `{}` has no bindable parameter {property}; bindable parameters are {}",
            found.id,
            definitions.iter().filter(|definition| definition.visible).map(|definition| definition.name.as_str()).collect::<Vec<_>>().join(", ")
        )))?;
    if source && definition.direction == "IN" {
        return Err(AuthoringError::Refused(format!(
            "{property} is an input of {child}"
        )));
    }
    if !source && definition.direction == "OUT" {
        return Err(AuthoringError::Refused(format!(
            "{property} is an output of {child}"
        )));
    }
    let cached = document.at(&found.path)["Properties"]["MashupParameters"]
        .as_array()
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["ParameterName"] == property)
        });
    if !cached {
        let mut entries = Vec::new();
        refresh_containers(document.value_mut(), &child, &definitions, &mut entries);
        for (id, _) in entries {
            if !refreshed.contains(&id) {
                refreshed.push(id);
            }
        }
    }
    Ok(End::Ui {
        id: found.id,
        property,
        base: definition.base_type.clone(),
    })
}

fn resolve_service<'a>(
    model: &'a types::Model,
    platform: Option<&types::Platform>,
    raw: &str,
) -> Result<(&'a types::Entity, String), AuthoringError> {
    let mut matches = Vec::new();
    for entity in &model.entities {
        if !matches!(
            entity.collection.as_str(),
            "Things" | "ThingTemplates" | "ThingShapes"
        ) {
            continue;
        }
        let Some(rest) = raw.strip_prefix(&(entity.name.clone() + ".")) else {
            continue;
        };
        let mut ends = vec![rest];
        ends.extend(rest.match_indices('.').map(|(at, _)| &rest[..at]));
        for service in ends {
            if catalog::lookup_service(entity, &model.entities, platform, service)
                .service
                .is_some()
            {
                matches.push((entity, service.to_string()));
            }
        }
    }
    matches.sort_by(|a, b| (&a.0.name, &a.1).cmp(&(&b.0.name, &b.1)));
    matches.dedup_by(|a, b| a.0.name == b.0.name && a.1 == b.1);
    match matches.as_slice() {
        [] => Err(AuthoringError::Unknown(format!(
            "no entity and service in the solution match data:{raw}"
        ))),
        [only] => Ok((only.0, only.1.clone())),
        _ => Err(AuthoringError::Ambiguous(format!(
            "data:{raw} is ambiguous; name an entity and service that resolve once"
        ))),
    }
}

fn base(end: &End) -> &str {
    match end {
        End::Ui { base, .. } | End::Mashup { base, .. } | End::Data { base, .. } => base,
    }
}

/// Sidebar menus receive a complete service result, never a parameter or selected rows.
fn menu_service(
    document: &mut MashupDocument,
    context: &AuthoringContext<'_>,
    raw: &str,
) -> Result<(), AuthoringError> {
    let address = parse_address(raw).map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    let Address::Data {
        selected_rows,
        member,
        ..
    } = &address
    else {
        return Err(AuthoringError::Invalid(
            "a menu address names a data service result".into(),
        ));
    };
    if *selected_rows || member.is_some() {
        return Err(AuthoringError::Invalid(
            "a menu address names a service result, not a parameter or SelectedRows".into(),
        ));
    }
    let end = resolve_end(document, None, context, address, true, &mut Vec::new())?;
    if base(&end) != "INFOTABLE" {
        return Err(AuthoringError::Refused(
            "the menu service must return an INFOTABLE".into(),
        ));
    }
    Ok(())
}
fn property(end: &End) -> &str {
    match end {
        End::Ui { property, .. } => property,
        End::Mashup { parameter, .. } => parameter,
        End::Data { member, .. } => member.as_deref().unwrap_or(""),
    }
}
fn area(end: &End) -> &'static str {
    match end {
        End::Ui { .. } => "UI",
        End::Mashup { .. } => "Mashup",
        End::Data { .. } => "Data",
    }
}
fn details(end: &End) -> &'static str {
    match end {
        End::Data {
            selected_rows: true,
            ..
        } => "SelectedRows",
        End::Data { .. } => "AllData",
        _ => "",
    }
}
fn source_property_type(end: &End) -> &str {
    match end {
        End::Ui { .. } | End::Mashup { .. } => "Property",
        End::Data { property_type, .. } => property_type,
    }
}
fn target_property_type(end: &End) -> &str {
    match end {
        End::Ui { .. } | End::Mashup { .. } => "property",
        End::Data { .. } => "Parameter",
    }
}
fn allowed(from: &End, to: &End) -> bool {
    matches!(
        (from, to),
        (
            End::Ui { .. },
            End::Ui { .. } | End::Mashup { .. } | End::Data { .. }
        ) | (End::Mashup { .. }, End::Ui { .. } | End::Data { .. })
            | (End::Data { .. }, End::Ui { .. } | End::Data { .. })
    )
}

fn data_call(
    document: &mut MashupDocument,
    end: &End,
) -> Result<(String, String, Option<String>), AuthoringError> {
    let End::Data {
        entity,
        collection,
        service,
        requested_alias,
        ..
    } = end
    else {
        return Ok((
            String::new(),
            match end {
                End::Ui { id, .. } => id.clone(),
                End::Mashup { .. } => mashup_edit::ROOT_ID.into(),
                End::Data { .. } => unreachable!(),
            },
            None,
        ));
    };
    if let Some((section, call)) = existing_data_call(document, end)? {
        return Ok((section, call, None));
    }
    if collection != "Things" {
        return Err(AuthoringError::Refused(format!("{entity} is a template/shape: its data source needs the entity it runs on bound first (twaco does not create that yet); add the call in Composer once")));
    }
    let key = format!("Things_{entity}");
    let root = document
        .value_mut()
        .as_object_mut()
        .expect("checked: root object");
    if !root.contains_key("Data") {
        insert(root, "Data", Value::Object(Map::new()));
    }
    let data = root["Data"].as_object_mut().expect("checked: Data object");
    if !data.contains_key(&key) {
        let mut value = Map::new();
        insert(&mut value, "DataName", Value::String(key.clone()));
        insert(&mut value, "EntityName", Value::String(entity.clone()));
        insert(&mut value, "EntityType", Value::String("Things".into()));
        insert(&mut value, "Id", Value::String(Uuid::new_v4().to_string()));
        insert(&mut value, "RefreshInterval", Value::from(0));
        insert(&mut value, "Services", Value::Array(Vec::new()));
        insert(data, &key, Value::Object(value));
    }
    let section = &mut data[&key];
    let services = section["Services"]
        .as_array_mut()
        .ok_or_else(|| AuthoringError::Refused(format!("{key}.Services is not a list")))?;
    if let Some(alias) = requested_alias {
        if let Some(other) = services.iter().find(|call| call["Name"] == *alias) {
            return Err(AuthoringError::Exists(format!(
                "{entity} already has a call named {alias}, for {}",
                other["Target"].as_str().unwrap_or("another service")
            )));
        }
    }
    let alias = requested_alias.clone().unwrap_or_else(|| {
        let mut n = 1;
        loop {
            let name = if n == 1 {
                service.clone()
            } else {
                format!("{service}_{n}")
            };
            if !services.iter().any(|call| call["Name"] == name) {
                break name;
            }
            n += 1;
        }
    });
    let mut call = Map::new();
    insert(&mut call, "APIMethod", Value::String("post".into()));
    insert(
        &mut call,
        "Characteristic",
        Value::String("Services".into()),
    );
    insert(&mut call, "Id", Value::String(Uuid::new_v4().to_string()));
    insert(&mut call, "Name", Value::String(alias.clone()));
    insert(&mut call, "Parameters", Value::Object(Map::new()));
    insert(&mut call, "RefreshInterval", Value::from(0));
    insert(&mut call, "Target", Value::String(service.clone()));
    services.push(Value::Object(call));
    Ok((key, alias, Some(format!("{entity}.{service}"))))
}

/// The mashup's call of `end`'s service, as (section, alias). Several calls of the service and no
/// `[as alias]` to choose between them is ambiguous.
fn existing_data_call(
    document: &MashupDocument,
    end: &End,
) -> Result<Option<(String, String)>, AuthoringError> {
    let End::Data {
        entity,
        service,
        requested_alias,
        ..
    } = end
    else {
        return Ok(None);
    };
    let calls: Vec<(String, String)> = bindings::data_sections(document.value())
        .into_iter()
        .filter(|(_, details)| details.entity == *entity)
        .flat_map(|(section, details)| {
            details
                .services
                .iter()
                .filter(|call| {
                    call.target == *service
                        && requested_alias
                            .as_ref()
                            .is_none_or(|alias| call.instance == *alias)
                })
                .map(|call| (section.clone(), call.instance.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    match calls.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only.clone())),
        many => Err(AuthoringError::Ambiguous(format!(
            "{entity}.{service} has several calls in this mashup ({}); name one with [as <alias>]",
            many.iter()
                .map(|(_, alias)| alias.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn binding_exists(
    document: &MashupDocument,
    from: &End,
    to: &End,
    source_section: &str,
    source_id: &str,
    target_section: &str,
    target_id: &str,
) -> bool {
    document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|binding| {
            binding["SourceArea"] == area(from)
                && binding["SourceSection"] == source_section
                && binding["SourceId"] == source_id
                && binding["SourceDetails"] == details(from)
                && binding["TargetArea"] == area(to)
                && binding["TargetSection"] == target_section
                && binding["TargetId"] == target_id
                && binding["PropertyMaps"].as_array().is_some_and(|maps| {
                    maps.iter().any(|map| {
                        map["SourceProperty"] == property(from)
                            && map["TargetProperty"] == property(to)
                    })
                })
        })
}

/// Whether a binding already sets `to`.
fn has_source(document: &MashupDocument, to: &End, target_section: &str, target_id: &str) -> bool {
    document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|binding| {
            binding["TargetArea"] == area(to)
                && binding["TargetSection"] == target_section
                && binding["TargetId"] == target_id
                && binding["PropertyMaps"].as_array().is_some_and(|maps| {
                    maps.iter().any(|map| map["TargetProperty"] == property(to))
                })
        })
}

fn property_value(name: &str, text: &str, default: &Value) -> Result<Value, AuthoringError> {
    match default {
        Value::Bool(_) => match text {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(AuthoringError::Invalid(format!(
                "{name} must be true or false"
            ))),
        },
        Value::Number(_) => {
            if let Ok(number) = text.parse::<i64>() {
                Ok(Value::from(number))
            } else if let Ok(number) = text.parse::<f64>() {
                serde_json::Number::from_f64(number)
                    .map(Value::Number)
                    .ok_or_else(|| AuthoringError::Invalid(format!("{name} must be a JSON number")))
            } else {
                Err(AuthoringError::Invalid(format!(
                    "{name} must be a JSON number"
                )))
            }
        }
        Value::String(_) => Ok(Value::String(text.to_string())),
        Value::Object(_) | Value::Array(_) | Value::Null => Err(AuthoringError::Invalid(format!(
            "{name} is structured; set it in Composer"
        ))),
    }
}

/// Apply an existing-mashup edit as one transaction.
pub fn apply_edit(
    solution: &Solution,
    plan: &EditPlan,
    lock: &WorkspaceLock,
) -> Result<(), AuthoringError> {
    let mut transaction = Transaction::new(&solution.root, plan.command);
    transaction
        .replace_file(
            &plan.entity_path,
            &plan.entity_before,
            plan.entity_after.clone(),
        )
        .map_err(AuthoringError::Write)?;
    if let Some(sidecars) = &plan.sidecars {
        transaction
            .replace_file(
                &sidecars.content_path,
                &sidecars.content_before,
                sidecars.content_after.clone(),
            )
            .map_err(AuthoringError::Write)?;
    }
    transaction.apply(lock).map_err(AuthoringError::Write)?;
    Ok(())
}

/// The add plan as `--json` and MCP report it.
pub fn add_plan_json(solution: &Solution, plan: &AddPlan, applied: bool) -> Value {
    let relative = |path: &Path| {
        path.strip_prefix(&solution.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut files = vec![relative(&plan.edit.entity_path)];
    if let Some(sidecars) = &plan.edit.sidecars {
        files.push(relative(&sidecars.content_path));
    }
    json!({
        "applied": applied,
        "mashup": plan.edit.mashup,
        "widget": plan.widget,
        "id": plan.placed.id,
        "containers": plan.placed.containers,
        "flipped": plan.placed.flipped,
        "wrapped": plan.placed.wrapped,
        "files": files,
    })
}

/// The contained-mashup plan as `--json` and MCP report it.
pub fn add_contained_plan_json(
    solution: &Solution,
    plan: &AddContainedPlan,
    applied: bool,
) -> Value {
    let relative = |path: &Path| {
        path.strip_prefix(&solution.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut files = vec![relative(&plan.edit.entity_path)];
    if let Some(sidecars) = &plan.edit.sidecars {
        files.push(relative(&sidecars.content_path));
    }
    json!({
        "applied": applied, "mashup": plan.edit.mashup, "child": plan.child,
        "widget": "mashupcontainer", "id": plan.placed.id,
        "containers": plan.placed.containers, "flipped": plan.placed.flipped,
        "wrapped": plan.placed.wrapped, "parameters": plan.parameters.iter().map(|parameter| json!({
            "name": parameter.name, "base_type": parameter.base_type,
            "direction": parameter.direction, "set": plan.settings.contains(&parameter.name),
        })).collect::<Vec<_>>(), "files": files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn with_config(config: &str) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "twaco.toml", config);
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn one_project() -> (tempfile::TempDir, Solution) {
        with_config("[[project]]\nname = \"Acme\"\nroot = \"acme\"\n")
    }

    fn request(name: &str) -> NewRequest {
        NewRequest {
            name: name.to_string(),
            project: None,
            template: "blank".into(),
            primary_menu: None,
            secondary_menu: None,
            theme: None,
            master: false,
            visibility: Vec::new(),
            params: Vec::new(),
        }
    }

    fn lock(solution: &Solution) -> WorkspaceLock {
        crate::core::lock::acquire_for(solution, "test").unwrap()
    }

    #[test]
    fn a_blank_mashup_is_three_files_that_sync_agrees_with() {
        let (_dir, solution) = one_project();
        let plan = plan_new(&solution, &request("Acme.Orders_MU")).unwrap();
        assert_eq!(plan.project, "Acme");
        assert!(plan
            .entity_path
            .ends_with("acme/Mashups/Acme.Orders_MU.xml"));
        assert!(plan
            .content_path
            .ends_with("src/Acme.Orders_MU/mashup/content.json"));
        apply_new(&solution, &plan, &lock(&solution)).unwrap();

        let written = std::fs::read(&plan.entity_path).unwrap();
        let assets = workspace::read_mashup(plan.content_path.parent().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(assets, plan.assets);
        // A sync of what is on disk is the identity: the new mashup is already in sync.
        let (synced, changed) = mashup::sync(&written, &assets).unwrap();
        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!(synced, written);
        let text = String::from_utf8(written).unwrap();
        assert!(text.contains("name=\"Acme.Orders_MU\""));
        assert!(text.contains("projectName=\"Acme\""));
        assert!(text.contains("<Visibility></Visibility>"));
        let document = mashup_edit::parse(&assets.content).unwrap();
        assert_eq!(
            document.value()["UI"]["Properties"]["StyleTheme"],
            DEFAULT_THEME
        );
        assert_eq!(document.find("center").unwrap().id, "flexcontainer-2");
        // The entity is one the workspace now finds.
        assert!(workspace::entities(&solution)
            .iter()
            .any(|e| e.info.collection == "Mashups" && e.info.name == "Acme.Orders_MU"));
    }

    #[test]
    fn parameters_are_written_to_the_entity_and_content_together() {
        let (_dir, solution) = one_project();
        let mut request = request("Acme.Parameter_MU");
        request.params = vec!["id:LONG:IN".into(), "title:STRING".into()];
        let plan = plan_new(&solution, &request).unwrap();
        let xml = String::from_utf8(plan.entity.clone()).unwrap();
        assert!(xml.contains(r#"<FieldDefinition aspect.bindingDirection="IN" aspect.isMandatory="false" baseType="LONG" description="" name="id" ordinal="0"/>"#));
        let value: Value = serde_json::from_str(&plan.assets.content).unwrap();
        assert_eq!(
            value["UI"]["Properties"]["_currentParameterDefs"][0]["ParameterName"],
            "id"
        );
        assert_eq!(mashup_parameters(&plan.entity).unwrap().len(), 2);
        apply_new(&solution, &plan, &lock(&solution)).unwrap();
        let parameter = ParameterRequest {
            mashup: "Acme.Parameter_MU".into(),
            action: ParameterAction::Add,
            name: "header".into(),
            base_type: Some("STRING".into()),
            direction: None,
            description: None,
            default: ParameterDefault::Set("Orders".into()),
            data_shape: None,
            force: false,
        };
        let update = plan_parameter(&solution, &parameter).unwrap();
        apply_parameter(&solution, &update, &lock(&solution)).unwrap();
        let content = workspace::read_mashup(&solution.src_root().join("Acme.Parameter_MU/mashup"))
            .unwrap()
            .unwrap()
            .content;
        let value: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(value["UI"]["Properties"]["header"], "Orders");
        assert_eq!(
            value["UI"]["Properties"]["_currentParameterDefs"][2]["DefaultValue"],
            "Orders"
        );
    }

    #[test]
    fn a_master_mashup_has_a_master_root_and_a_page_container_filling_the_center() {
        let (_dir, solution) = one_project();
        let plan = plan_new(
            &solution,
            &NewRequest {
                master: true,
                ..request("Acme.Main_MM")
            },
        )
        .unwrap();
        let text = String::from_utf8(plan.entity.clone()).unwrap();
        assert!(text.contains("aspect.mashupType=\"target\""));
        let document = mashup_edit::parse(&plan.assets.content).unwrap();
        let root = &document.value()["UI"]["Properties"];
        assert_eq!(root["Type"], "targetmashup");
        assert_eq!(root["id_index"], 3);
        assert_eq!(document.value()["mashupType"], "targetmashup");
        let page = document.find("Page Mashup").unwrap();
        assert_eq!(page.id, "pagemashupcontainer-3");
        assert_eq!(
            document.at(&page.path)["Properties"]["ResponsiveLayout"],
            true
        );
        assert_eq!(page.path, [0, 0], "inside center");
    }

    #[test]
    fn layouts_have_the_composer_tree_and_masters_put_pages_in_center() {
        for (layout, expected) in [
            (
                "sidebar",
                vec![
                    ("flexcontainer-2", "flexcontainer", "page", "row"),
                    ("flexcontainer-3", "flexcontainer", "sidebar", "row"),
                    ("ptcsmenubar-4", "ptcsmenubar", "ptcs-menubar-4", ""),
                    ("flexcontainer-5", "flexcontainer", "center", "column"),
                ],
            ),
            (
                "header-footer",
                vec![
                    ("flexcontainer-2", "flexcontainer", "page", "column"),
                    ("flexcontainer-3", "flexcontainer", "header", "row"),
                    ("flexcontainer-4", "flexcontainer", "center", "column"),
                    ("flexcontainer-5", "flexcontainer", "footer", "row"),
                ],
            ),
        ] {
            for master in [false, true] {
                let template = mashup_templates::layout(layout).unwrap().unwrap();
                let document = new_document(&template, master, DEFAULT_THEME).unwrap();
                let tree: Vec<(String, String, String, String)> = document
                    .widgets()
                    .into_iter()
                    .skip(1)
                    .map(|widget| {
                        let properties = &document.at(&widget.path)["Properties"];
                        (
                            widget.id,
                            widget.type_id,
                            properties["DisplayName"].as_str().unwrap().into(),
                            properties["flex-direction"].as_str().unwrap_or("").into(),
                        )
                    })
                    .collect();
                let mut expected: Vec<(String, String, String, String)> = expected
                    .iter()
                    .map(|(id, type_id, name, direction)| {
                        (
                            (*id).into(),
                            (*type_id).into(),
                            (*name).into(),
                            (*direction).into(),
                        )
                    })
                    .collect();
                if master {
                    let at = if layout == "sidebar" { 4 } else { 3 };
                    expected.insert(
                        at,
                        (
                            "pagemashupcontainer-6".into(),
                            "pagemashupcontainer".into(),
                            "Page Mashup".into(),
                            "".into(),
                        ),
                    );
                    let page = document.find("Page Mashup").unwrap();
                    assert_eq!(page.path[..page.path.len() - 1], [0, 1]);
                }
                assert_eq!(tree, expected);
            }
        }
    }

    #[test]
    fn every_embedded_template_validates_and_its_fixed_nodes_follow_the_parent_axis() {
        for template in mashup_templates::layouts().unwrap() {
            validate_template(&template).unwrap();
        }
        for (name, expected) in [
            (
                "header-sidebar",
                vec![("flexcontainer-3", "header", "height")],
            ),
            (
                "form-dialog",
                vec![
                    ("flexcontainer-3", "title", "height"),
                    ("flexcontainer-5", "actions", "height"),
                ],
            ),
            (
                "card",
                vec![
                    ("flexcontainer-3", "header", "height"),
                    ("flexcontainer-5", "actions", "width"),
                ],
            ),
            (
                "master-detail",
                vec![("flexcontainer-4", "divider-4", "width")],
            ),
            ("kpi-tile", vec![("flexcontainer-4", "divider-4", "height")]),
        ] {
            let template = mashup_templates::layout(name).unwrap().unwrap();
            let document = new_document(&template, false, DEFAULT_THEME).unwrap();
            let expected_tree = match name {
                "header-sidebar" => vec![
                    ("flexcontainer-2", "flexcontainer", "page"),
                    ("flexcontainer-3", "flexcontainer", "header"),
                    ("flexcontainer-4", "flexcontainer", "body"),
                    ("flexcontainer-5", "flexcontainer", "sidebar"),
                    ("ptcsmenubar-6", "ptcsmenubar", "ptcs-menubar-6"),
                    ("flexcontainer-7", "flexcontainer", "center"),
                ],
                "form-dialog" => vec![
                    ("flexcontainer-2", "flexcontainer", "page"),
                    ("flexcontainer-3", "flexcontainer", "title"),
                    ("flexcontainer-4", "flexcontainer", "fields"),
                    ("flexcontainer-5", "flexcontainer", "actions"),
                ],
                "card" => vec![
                    ("flexcontainer-2", "flexcontainer", "page"),
                    ("flexcontainer-3", "flexcontainer", "header"),
                    ("flexcontainer-4", "flexcontainer", "title"),
                    ("flexcontainer-5", "flexcontainer", "actions"),
                    ("flexcontainer-6", "flexcontainer", "body"),
                ],
                "master-detail" => vec![
                    ("flexcontainer-2", "flexcontainer", "page"),
                    ("flexcontainer-3", "flexcontainer", "list"),
                    ("flexcontainer-4", "flexcontainer", "divider-4"),
                    ("flexcontainer-5", "flexcontainer", "detail"),
                ],
                "kpi-tile" => vec![
                    ("flexcontainer-2", "flexcontainer", "page"),
                    ("flexcontainer-3", "flexcontainer", "title"),
                    ("flexcontainer-4", "flexcontainer", "divider-4"),
                    ("flexcontainer-5", "flexcontainer", "value"),
                ],
                _ => unreachable!("only requested templates"),
            };
            assert_eq!(
                document
                    .widgets()
                    .into_iter()
                    .skip(1)
                    .map(|widget| (widget.id, widget.type_id, widget.display_name))
                    .collect::<Vec<_>>(),
                expected_tree
                    .into_iter()
                    .map(|(id, type_id, display_name)| {
                        (id.into(), type_id.into(), display_name.into())
                    })
                    .collect::<Vec<_>>(),
                "{name}"
            );
            for (id, slot, axis) in expected {
                let widget = document.find_exact(id).unwrap();
                let properties = &document.at(&widget.path)["Properties"];
                assert_eq!(properties["DisplayName"], slot, "{name}: {id}");
                assert_eq!(properties["flex-size"], "fixed", "{name}: {id}");
                assert!(
                    properties[format!("flex-min-{axis}")].is_string(),
                    "{name}: {id}"
                );
                assert!(
                    properties[format!("flex-max-{axis}")].is_string(),
                    "{name}: {id}"
                );
            }
            assert!(document.problems().is_empty(), "{name}");
        }
    }

    #[test]
    fn masters_use_each_template_master_slot() {
        for (name, slot) in [("form-dialog", "fields"), ("header-sidebar", "center")] {
            let template = mashup_templates::layout(name).unwrap().unwrap();
            let document = new_document(&template, true, DEFAULT_THEME).unwrap();
            let page = document.find("Page Mashup").unwrap();
            let container = document.find(slot).unwrap();
            assert_eq!(page.path[..page.path.len() - 1], container.path, "{name}");
        }
    }

    #[test]
    fn add_in_a_template_slot_places_the_widget_in_that_container() {
        let (_dir, solution) = one_project();
        let new = plan_new(
            &solution,
            &NewRequest {
                template: "form-dialog".into(),
                ..request("Acme.Edit_MU")
            },
        )
        .unwrap();
        apply_new(&solution, &new, &lock(&solution)).unwrap();
        let added = plan_add(
            &solution,
            &AddRequest {
                mashup: "Acme.Edit_MU".into(),
                widget: "button".into(),
                placement: Some(Placement::In("actions".into())),
                label: None,
                properties: Vec::new(),
            },
        )
        .unwrap();
        let content = String::from_utf8(added.edit.sidecars.unwrap().content_after).unwrap();
        let document = mashup_edit::parse(&content).unwrap();
        let button = document.find_exact(&added.placed.id).unwrap();
        assert_eq!(
            button.path[..button.path.len() - 1],
            document.find("actions").unwrap().path
        );
    }

    #[test]
    fn sidebar_menus_are_service_calls_bound_and_loaded_like_bind_and_on() {
        let (dir, solution) = one_project();
        write(
            dir.path(),
            "acme/Things/Acme.Nav.Manager.xml",
            r#"<Entities><Things><Thing name="Acme.Nav.Manager" projectName="Acme"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetPrimaryMenu"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Nav.Menu_DS"/></ServiceDefinition>
            <ServiceDefinition name="GetSecondaryMenu"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Nav.Menu_DS"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
        );
        write(
            dir.path(),
            "acme/DataShapes/Acme.Nav.Menu_DS.xml",
            r#"<Entities><DataShapes><DataShape name="Acme.Nav.Menu_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="title" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
        );
        let plan = plan_new(
            &solution,
            &NewRequest {
                template: "sidebar".into(),
                primary_menu: Some("data:Acme.Nav.Manager.GetPrimaryMenu".into()),
                secondary_menu: Some("data:Acme.Nav.Manager.GetSecondaryMenu".into()),
                ..request("Acme.Nav_MU")
            },
        )
        .unwrap();
        assert_eq!(
            plan.menus,
            [
                (
                    "data:Acme.Nav.Manager.GetPrimaryMenu".into(),
                    "PrimaryNavData".into()
                ),
                (
                    "data:Acme.Nav.Manager.GetSecondaryMenu".into(),
                    "SecondaryNavData".into()
                ),
            ]
        );
        apply_new(&solution, &plan, &lock(&solution)).unwrap();
        let content: Value = serde_json::from_str(&plan.assets.content).unwrap();
        assert_eq!(
            content["Data"]["Things_Acme.Nav.Manager"]["Services"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let bindings: Vec<&str> = content["DataBindings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|binding| {
                binding["PropertyMaps"][0]["TargetProperty"]
                    .as_str()
                    .unwrap()
            })
            .collect();
        assert_eq!(bindings, ["PrimaryNavData", "SecondaryNavData"]);
        assert_eq!(content["Events"].as_array().unwrap().len(), 2);
        let report = crate::core::check::run(&solution);
        let failing: Vec<_> = report
            .gates
            .iter()
            // The fixture declares service definitions only, which the project gate reports.
            .filter(|gate| !gate.ok() && gate.name != "project")
            .collect();
        assert!(failing.is_empty(), "{failing:?}");
        let description = crate::core::mashup_describe::describe(&solution, "Acme.Nav_MU").unwrap();
        assert_eq!(
            description
                .bindings
                .iter()
                .map(|binding| (binding.from.as_str(), binding.to.as_str()))
                .collect::<Vec<_>>(),
            [
                (
                    "data:Acme.Nav.Manager.GetPrimaryMenu",
                    "ui:ptcsmenubar-4.PrimaryNavData",
                ),
                (
                    "data:Acme.Nav.Manager.GetSecondaryMenu",
                    "ui:ptcsmenubar-4.SecondaryNavData",
                ),
            ]
        );
        assert_eq!(
            description
                .events
                .iter()
                .map(|event| (event.trigger.as_str(), event.handler.as_str()))
                .collect::<Vec<_>>(),
            [
                ("mashup:Loaded", "data:Acme.Nav.Manager.GetPrimaryMenu"),
                ("mashup:Loaded", "data:Acme.Nav.Manager.GetSecondaryMenu"),
            ]
        );
    }

    #[test]
    fn sidebar_menu_requests_refuse_missing_wrong_or_non_sidebar_menus() {
        let (dir, solution) = one_project();
        write(
            dir.path(),
            "acme/Things/Acme.Nav.Manager.xml",
            r#"<Entities><Things><Thing name="Acme.Nav.Manager" projectName="Acme"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetMenu"><ResultType baseType="STRING"/></ServiceDefinition>
            <ServiceDefinition name="GetTable"><ParameterDefinitions><FieldDefinition name="filter" baseType="STRING"/></ParameterDefinitions><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Nav.Menu_DS"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
        );
        write(
            dir.path(),
            "acme/DataShapes/Acme.Nav.Menu_DS.xml",
            r#"<Entities><DataShapes><DataShape name="Acme.Nav.Menu_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="title" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
        );
        let error = plan_new(
            &solution,
            &NewRequest {
                template: "sidebar".into(),
                ..request("Acme.Nav_MU")
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        let invalid = |primary_menu: &str| {
            plan_new(
                &solution,
                &NewRequest {
                    template: "sidebar".into(),
                    primary_menu: Some(primary_menu.into()),
                    ..request("Acme.Nav_MU")
                },
            )
            .unwrap_err()
        };
        assert!(invalid("data:Acme.Nav.Manager.GetMenu")
            .to_string()
            .contains("menu service must return an INFOTABLE"));
        assert_eq!(
            invalid("data:Acme.Nav.Manager.GetTable.filter").code(),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            invalid("data:Acme.Nav.Manager.GetTable[SelectedRows]").code(),
            ErrorCode::InvalidArguments
        );
        let error = plan_new(
            &solution,
            &NewRequest {
                template: "header-footer".into(),
                primary_menu: Some("data:Acme.Nav.Manager.GetTable".into()),
                ..request("Acme.Nav_MU")
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        assert!(mashup_templates::layout("stacked").unwrap().is_none());
    }

    #[test]
    fn a_theme_must_be_the_default_or_in_the_solution() {
        let (dir, solution) = one_project();
        let error = plan_new(
            &solution,
            &NewRequest {
                theme: Some("Acme.Theme".into()),
                ..request("Acme.Orders_MU")
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnknownEntity);
        write(
            dir.path(),
            "acme/StyleThemes/Acme.Theme.xml",
            "<Entities><StyleThemes><StyleTheme name=\"Acme.Theme\" projectName=\"Acme\"></StyleTheme></StyleThemes></Entities>",
        );
        let plan = plan_new(
            &solution,
            &NewRequest {
                theme: Some("Acme.Theme".into()),
                ..request("Acme.Orders_MU")
            },
        )
        .unwrap();
        assert!(plan
            .assets
            .content
            .contains("\"StyleTheme\": \"Acme.Theme\""));
    }

    #[test]
    fn visibility_is_only_what_is_asked_and_must_be_well_formed() {
        assert_eq!(
            parse_visibility("OrganizationalUnit/Acme.Org:Acme.Users").unwrap(),
            Visibility {
                kind: "OrganizationalUnit".into(),
                name: "Acme.Org:Acme.Users".into()
            }
        );
        for bad in [
            "Acme.Org",
            "User/Administrator",
            "Organization/",
            "Organization/Acme:X",
            "OrganizationalUnit/Acme.Org",
            "OrganizationalUnit/:x",
        ] {
            assert!(parse_visibility(bad).is_err(), "{bad}");
        }
        let (_dir, solution) = one_project();
        let plan = plan_new(
            &solution,
            &NewRequest {
                visibility: vec!["Organization/Acme.Org".into()],
                ..request("Acme.Orders_MU")
            },
        )
        .unwrap();
        let text = String::from_utf8(plan.entity).unwrap();
        assert!(text.contains("name=\"Acme.Org\"\n                     type=\"Organization\""));
    }

    #[test]
    fn an_existing_mashup_or_file_and_an_unclear_project_are_refused() {
        let (dir, solution) = one_project();
        write(
            dir.path(),
            "acme/Mashups/Acme.Orders_MU.xml",
            "<Entities><Mashups><Mashup name=\"Acme.Orders_MU\" projectName=\"Acme\"><mashupContent><![CDATA[{}]]></mashupContent></Mashup></Mashups></Entities>",
        );
        let error = plan_new(&solution, &request("Acme.Orders_MU")).unwrap_err();
        assert_eq!(error.code(), ErrorCode::AlreadyExists, "{error}");
        // A leftover sidecar folder is not overwritten either.
        write(dir.path(), "src/Acme.Other_MU/mashup/content.json", "{}");
        let error = plan_new(&solution, &request("Acme.Other_MU")).unwrap_err();
        assert_eq!(error.code(), ErrorCode::AlreadyExists, "{error}");

        let (_two, two) = with_config(
            "[[project]]\nname = \"Acme\"\nroot = \"acme\"\n\n[[project]]\nname = \"Acme.UI\"\nroot = \"ui\"\n",
        );
        let error = plan_new(&two, &request("Acme.Orders_MU")).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        assert!(error.to_string().contains("--project"), "{error}");
        let error = plan_new(
            &two,
            &NewRequest {
                project: Some("Nope".into()),
                ..request("Acme.Orders_MU")
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnknownEntity);
        let plan = plan_new(
            &two,
            &NewRequest {
                project: Some("Acme.UI".into()),
                ..request("Acme.Orders_MU")
            },
        )
        .unwrap();
        assert!(plan.entity_path.ends_with("ui/Mashups/Acme.Orders_MU.xml"));
        let error = plan_new(&two, &request("Acme/Orders")).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
    }

    #[test]
    fn a_name_that_cannot_be_a_file_name_everywhere_is_refused_when_planning() {
        let (_dir, solution) = one_project();
        for name in [
            "Acme?Orders_MU",
            "Acme*Orders",
            "CON.Orders_MU",
            "Acme|X",
            "Acme.Orders.",
        ] {
            let error = plan_new(&solution, &request(name)).unwrap_err();
            assert_eq!(error.code(), ErrorCode::InvalidArguments, "{name}: {error}");
        }
    }

    #[test]
    fn a_file_where_a_folder_goes_is_refused_when_planning() {
        let (dir, solution) = one_project();
        write(dir.path(), "src/Acme.Orders_MU", "not a folder");
        let error = plan_new(&solution, &request("Acme.Orders_MU")).unwrap_err();
        assert_eq!(error.code(), ErrorCode::AlreadyExists, "{error}");
        assert!(error.to_string().contains("needs a folder"), "{error}");
    }

    #[test]
    fn a_new_mashup_passes_every_gate() {
        let (_dir, solution) = one_project();
        for request in [
            request("Acme.Orders_MU"),
            NewRequest {
                master: true,
                ..request("Acme.Main_MM")
            },
        ] {
            let plan = plan_new(&solution, &request).unwrap();
            apply_new(&solution, &plan, &lock(&solution)).unwrap();
        }
        let report = crate::core::check::run(&solution);
        let failing: Vec<_> = report
            .gates
            .iter()
            .filter(|gate| !gate.ok())
            .map(|gate| format!("{}: {:?} {:?}", gate.name, gate.findings, gate.broken))
            .collect();
        assert!(failing.is_empty(), "{failing:?}");
    }

    #[test]
    fn a_file_appearing_after_the_plan_stops_the_write_and_nothing_is_left() {
        let (dir, solution) = one_project();
        let plan = plan_new(&solution, &request("Acme.Orders_MU")).unwrap();
        write(
            dir.path(),
            "src/Acme.Orders_MU/mashup/custom.css",
            "body {}\n",
        );
        let error = apply_new(&solution, &plan, &lock(&solution)).unwrap_err();
        assert!(matches!(error, AuthoringError::Write(_)), "{error}");
        assert!(!plan.entity_path.exists());
        assert!(!plan.content_path.exists());
    }

    fn add_request(mashup: &str, widget: &str) -> AddRequest {
        AddRequest {
            mashup: mashup.to_string(),
            widget: widget.to_string(),
            placement: None,
            label: None,
            properties: Vec::new(),
        }
    }

    #[test]
    fn adding_widgets_keeps_sidecars_and_the_rest_of_the_entity_identical() {
        let (_dir, solution) = one_project();
        let name = "Acme.Orders_MU";
        let new = plan_new(&solution, &request(name)).unwrap();
        apply_new(&solution, &new, &lock(&solution)).unwrap();
        let grid = plan_add(&solution, &add_request(name, "grid")).unwrap();
        assert_eq!(grid.placed.id, "ptcsgrid-3");
        apply_edit(&solution, &grid.edit, &lock(&solution)).unwrap();
        let before_button = std::fs::read(&grid.edit.entity_path).unwrap();
        let mut button_request = add_request(name, "button");
        button_request.placement = Some(Placement::Beside {
            anchor: grid.placed.id.clone(),
            side: mashup_edit::Side::Below,
        });
        button_request.label = Some("Save".into());
        button_request.properties = vec![("ButtonType".into(), "secondary".into())];
        let button = plan_add(&solution, &button_request).unwrap();
        apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
        let entity = std::fs::read(&button.edit.entity_path).unwrap();
        let assets = workspace::read_mashup(new.content_path.parent().unwrap())
            .unwrap()
            .unwrap();
        assert!(mashup::sync(&entity, &assets).unwrap().1.is_empty());
        let document = mashup_edit::parse(&assets.content).unwrap();
        let properties =
            &document.at(&document.find(&button.placed.id).unwrap().path)["Properties"];
        assert_eq!(properties["Label"], "Save");
        assert_eq!(properties["ButtonType"], "secondary");
        let content = |bytes: &[u8]| {
            let start = bytes
                .windows(b"<mashupContent".len())
                .position(|part| part == b"<mashupContent")
                .unwrap();
            let end = bytes
                .windows(b"</mashupContent>".len())
                .position(|part| part == b"</mashupContent>")
                .unwrap()
                + b"</mashupContent>".len();
            (bytes[..start].to_vec(), bytes[end..].to_vec())
        };
        assert_eq!(content(&before_button), content(&entity));
        assert!(crate::core::check::run(&solution).ok());
    }

    #[test]
    fn adding_refuses_drift_and_edits_an_unmanaged_mashup_only_in_its_entity() {
        let (dir, solution) = one_project();
        let name = "Acme.Orders_MU";
        let new = plan_new(&solution, &request(name)).unwrap();
        apply_new(&solution, &new, &lock(&solution)).unwrap();
        write(dir.path(), "src/Acme.Orders_MU/mashup/content.json", "{}\n");
        let error = plan_add(&solution, &add_request(name, "grid")).unwrap_err();
        assert!(matches!(error, AuthoringError::Refused(_)));
        let assets = mashup::extract(&std::fs::read(&new.entity_path).unwrap()).unwrap();
        std::fs::write(&new.content_path, assets.content).unwrap();
        std::fs::remove_dir_all(new.content_path.parent().unwrap()).unwrap();
        let plan = plan_add(&solution, &add_request(name, "grid")).unwrap();
        assert!(plan.edit.sidecars.is_none());
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        assert!(!new.content_path.exists());
    }

    #[test]
    fn add_validates_widget_properties_and_labels() {
        let (_dir, solution) = one_project();
        let name = "Acme.Orders_MU";
        let new = plan_new(&solution, &request(name)).unwrap();
        apply_new(&solution, &new, &lock(&solution)).unwrap();
        for (widget, property) in [
            ("grid", ("Nope", "x")),
            ("grid", ("Visible", "TRUE")),
            ("button", ("ButtonType", "wrong")),
            ("button", ("Id", "x")),
            ("toolbar", ("CustomBaseTypesMapping", "x")),
        ] {
            let mut request = add_request(name, widget);
            request.properties = vec![(property.0.into(), property.1.into())];
            assert!(matches!(
                plan_add(&solution, &request),
                Err(AuthoringError::Invalid(_))
            ));
        }
        let mut label = add_request(name, "flexcontainer");
        label.label = Some("No label".into());
        assert!(matches!(
            plan_add(&solution, &label),
            Err(AuthoringError::Invalid(_))
        ));
        assert!(matches!(
            plan_add(&solution, &add_request(name, "nope")),
            Err(AuthoringError::Invalid(_))
        ));
        assert!(matches!(
            plan_add(&solution, &add_request("Acme.Nope_MU", "grid")),
            Err(AuthoringError::Unknown(_))
        ));
    }

    #[test]
    fn binding_a_service_result_creates_a_thing_call_and_composer_binding() {
        let (dir, solution) = one_project();
        write(
            dir.path(),
            "acme/Things/Acme.Orders.Manager.xml",
            r#"<Entities><Things><Thing name="Acme.Orders.Manager" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
        );
        write(
            dir.path(),
            "acme/DataShapes/Acme.Orders.Order_DS.xml",
            r#"<Entities><DataShapes><DataShape name="Acme.Orders.Order_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="id" baseType="LONG"/><FieldDefinition name="note" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
        );
        let name = "Acme.Orders_MU";
        let new = plan_new(&solution, &request(name)).unwrap();
        apply_new(&solution, &new, &lock(&solution)).unwrap();
        let grid = plan_add(&solution, &add_request(name, "grid")).unwrap();
        apply_edit(&solution, &grid.edit, &lock(&solution)).unwrap();
        let plan = plan_bind(
            &solution,
            &BindRequest {
                mashup: name.into(),
                from: "data:Acme.Orders.Manager.GetOrders".into(),
                to: "ui:ptcsgrid-3.Data".into(),
            },
        )
        .unwrap();
        assert_eq!(
            plan.new_source.as_deref(),
            Some("Acme.Orders.Manager.GetOrders")
        );
        let content = plan.edit.sidecars.as_ref().unwrap().content_after.clone();
        let value: Value = serde_json::from_slice(&content).unwrap();
        let call = &value["Data"]["Things_Acme.Orders.Manager"]["Services"][0];
        assert_eq!(call["Name"], "GetOrders");
        let binding = &value["DataBindings"][0];
        assert_eq!(binding["SourceArea"], "Data");
        assert_eq!(binding["SourceDetails"], "AllData");
        assert_eq!(
            binding["PropertyMaps"][0]["SourcePropertyType"],
            "InfoTable"
        );
        assert_eq!(
            binding["PropertyMaps"][0]["SourcePropertyBaseType"],
            "INFOTABLE"
        );
    }

    /// A Thing with GetOrders and GetOrder(id) (results of Order_DS: id, note) and
    /// SaveOrder(id, note), a shape with GetLog,
    /// that DataShape, and a blank mashup with a grid, a text field and a label in it.
    fn bind_fixture() -> (tempfile::TempDir, Solution) {
        let (dir, solution) = one_project();
        write(
            dir.path(),
            "acme/Things/Acme.Orders.Manager.xml",
            r#"<Entities><Things><Thing name="Acme.Orders.Manager" projectName="Acme"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="GetOrders"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            <ServiceDefinition name="SaveOrder"><ParameterDefinitions><FieldDefinition name="id" baseType="LONG"/><FieldDefinition name="note" baseType="STRING"/></ParameterDefinitions><ResultType baseType="NOTHING"/></ServiceDefinition>
            <ServiceDefinition name="GetOrder"><ParameterDefinitions><FieldDefinition name="id" baseType="LONG"/></ParameterDefinitions><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            </ServiceDefinitions><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
        );
        write(
            dir.path(),
            "acme/ThingShapes/Acme.Orders.Audit_TS.xml",
            r#"<Entities><ThingShapes><ThingShape name="Acme.Orders.Audit_TS" projectName="Acme"><ServiceDefinitions>
            <ServiceDefinition name="GetLog"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Orders.Order_DS"/></ServiceDefinition>
            </ServiceDefinitions></ThingShape></ThingShapes></Entities>"#,
        );
        write(
            dir.path(),
            "acme/DataShapes/Acme.Orders.Order_DS.xml",
            r#"<Entities><DataShapes><DataShape name="Acme.Orders.Order_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="id" baseType="LONG"/><FieldDefinition name="note" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
        );
        let name = "Acme.Orders_MU";
        let new = plan_new(&solution, &request(name)).unwrap();
        apply_new(&solution, &new, &lock(&solution)).unwrap();
        // The grid last, so it shares center and does not fill it.
        for widget in ["text-field", "label", "grid"] {
            let add = add_request(name, widget);
            let plan = plan_add(&solution, &add).unwrap();
            apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        }
        (dir, solution)
    }

    fn bind(solution: &Solution, from: &str, to: &str) -> Result<BindPlan, AuthoringError> {
        plan_bind(
            solution,
            &BindRequest {
                mashup: "Acme.Orders_MU".into(),
                from: from.into(),
                to: to.into(),
            },
        )
    }

    fn sidecar_content(solution: &Solution) -> String {
        workspace::read_mashup(&solution.src_root().join("Acme.Orders_MU/mashup"))
            .unwrap()
            .unwrap()
            .content
    }

    #[test]
    fn bindings_to_parameters_and_from_selected_rows_reuse_calls_and_pass_the_gates() {
        let (_dir, solution) = bind_fixture();
        let ids: Vec<String> = mashup_edit::parse(&sidecar_content(&solution))
            .unwrap()
            .widgets()
            .into_iter()
            .map(|w| w.id)
            .collect();
        assert!(ids.contains(&"ptcstextfield-3".to_string()), "{ids:?}");
        assert!(ids.contains(&"ptcslabel-4".to_string()), "{ids:?}");
        let rows = [
            ("data:Acme.Orders.Manager.GetOrders", "ui:ptcsgrid-5.Data"),
            (
                "ui:ptcstextfield-3.Text",
                "data:Acme.Orders.Manager.SaveOrder.note",
            ),
            (
                "data:Acme.Orders.Manager.GetOrders[SelectedRows].note",
                "ui:ptcslabel-4.LabelText",
            ),
        ];
        for (from, to) in rows {
            let plan = bind(&solution, from, to).unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
            apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        }
        let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
        let calls: Vec<&str> = value["Data"]["Things_Acme.Orders.Manager"]["Services"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| call["Name"].as_str().unwrap())
            .collect();
        assert_eq!(calls, ["GetOrders", "SaveOrder"], "GetOrders is reused");
        let maps: Vec<(&str, &str, &str)> = value["DataBindings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                let map = &b["PropertyMaps"][0];
                (
                    map["SourcePropertyType"].as_str().unwrap(),
                    map["TargetPropertyType"].as_str().unwrap(),
                    map["SourcePropertyBaseType"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            maps,
            [
                ("InfoTable", "property", "INFOTABLE"),
                ("Property", "Parameter", "STRING"),
                ("Field", "property", "STRING"),
            ]
        );
        assert_eq!(value["DataBindings"][2]["SourceDetails"], "SelectedRows");
        let report = crate::core::check::run(&solution);
        let failing: Vec<_> = report
            .gates
            .iter()
            // The fixture's services have no implementations, which only the project gate minds.
            .filter(|gate| !gate.ok() && gate.name != "project")
            .map(|gate| format!("{}: {:?} {:?}", gate.name, gate.findings, gate.broken))
            .collect();
        assert!(failing.is_empty(), "{failing:?}");
        let described: Vec<(String, String)> =
            crate::core::mashup_describe::describe(&solution, "Acme.Orders_MU")
                .unwrap()
                .bindings
                .into_iter()
                .map(|b| (b.from, b.to))
                .collect();
        assert_eq!(described, rows.map(|(a, b)| (a.to_string(), b.to_string())));
    }

    #[test]
    fn a_binding_twaco_cannot_stand_behind_is_refused() {
        let (dir, solution) = bind_fixture();
        let code = |from: &str, to: &str| bind(&solution, from, to).unwrap_err().code();
        assert_eq!(
            code("ui:nope-9.Text", "ui:ptcsgrid-5.Data"),
            ErrorCode::UnknownEntity
        );
        // STRING into a LONG parameter.
        assert_eq!(
            code(
                "ui:ptcstextfield-3.Text",
                "data:Acme.Orders.Manager.SaveOrder.id"
            ),
            ErrorCode::GuardRefused
        );
        // A grid's Data is an input only.
        assert_eq!(
            code("ui:ptcsgrid-5.Data", "ui:ptcslabel-4.LabelText"),
            ErrorCode::GuardRefused
        );
        // A service as a target needs a parameter.
        assert_eq!(
            code(
                "ui:ptcstextfield-3.Text",
                "data:Acme.Orders.Manager.SaveOrder"
            ),
            ErrorCode::InvalidArguments
        );
        // A shape's service with no call in the mashup yet.
        assert_eq!(
            code("data:Acme.Orders.Audit_TS.GetLog", "ui:ptcsgrid-5.Data"),
            ErrorCode::GuardRefused
        );
        assert_eq!(
            code("bogus", "ui:ptcsgrid-5.Data"),
            ErrorCode::InvalidArguments
        );
        // The same binding twice.
        let plan = bind(
            &solution,
            "data:Acme.Orders.Manager.GetOrders",
            "ui:ptcsgrid-5.Data",
        )
        .unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        assert_eq!(
            code("data:Acme.Orders.Manager.GetOrders", "ui:ptcsgrid-5.Data"),
            ErrorCode::AlreadyExists
        );
        // A widget the catalogue has no contract for: the label becomes an extension's widget, in
        // both the sidecar and the entity file so they agree.
        let content = sidecar_content(&solution).replace("\"ptcslabel\"", "\"acmewidget\"");
        std::fs::write(
            dir.path().join("src/Acme.Orders_MU/mashup/content.json"),
            &content,
        )
        .unwrap();
        let entity_path = dir.path().join("acme/Mashups/Acme.Orders_MU.xml");
        let css = std::fs::read_to_string(dir.path().join("src/Acme.Orders_MU/mashup/custom.css"))
            .unwrap();
        let (entity, _) = mashup::sync(
            &std::fs::read(&entity_path).unwrap(),
            &mashup::Assets { content, css },
        )
        .unwrap();
        std::fs::write(&entity_path, entity).unwrap();
        let error = bind(&solution, "ui:ptcstextfield-3.Text", "ui:ptcslabel-4.Text").unwrap_err();
        assert_eq!(error.code(), ErrorCode::GuardRefused, "{error}");
        assert!(
            error
                .to_string()
                .contains("whose contract is not in the embedded ThingWorx 10.2 catalogue"),
            "{error}"
        );
    }

    fn on(solution: &Solution, event: &str, invoke: &str) -> Result<OnPlan, AuthoringError> {
        plan_on(
            solution,
            &OnRequest {
                mashup: "Acme.Orders_MU".into(),
                event: event.into(),
                invoke: invoke.into(),
            },
        )
    }

    #[test]
    fn events_use_composer_keys_and_describe_their_wiring() {
        let (_dir, solution) = bind_fixture();
        let button = plan_add(&solution, &add_request("Acme.Orders_MU", "button")).unwrap();
        assert_eq!(button.placed.id, "ptcsbutton-6");
        apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
        let rows = [
            (
                "ui:ptcsbutton-6.Clicked",
                "data:Acme.Orders.Manager.SaveOrder",
            ),
            ("mashup:Loaded", "data:Acme.Orders.Manager.GetOrders"),
            (
                "data:Acme.Orders.Manager.SaveOrder.ServiceInvokeCompleted",
                "data:Acme.Orders.Manager.GetOrders",
            ),
        ];
        for (event, invoke) in rows {
            let plan = on(&solution, event, invoke)
                .unwrap_or_else(|error| panic!("{event} -> {invoke}: {error}"));
            apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        }
        let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
        let first = value["Events"][0].as_object().unwrap();
        assert_eq!(
            first.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "EventHandlerArea",
                "EventHandlerId",
                "EventHandlerService",
                "EventHandlerServiceMethod",
                "EventTriggerArea",
                "EventTriggerEvent",
                "EventTriggerId",
                "EventTriggerSection",
                "Id"
            ]
        );
        assert_eq!(first["EventHandlerArea"], "Data");
        assert_eq!(first["EventHandlerId"], "Things_Acme.Orders.Manager");
        assert_eq!(first["EventHandlerService"], "SaveOrder");
        assert_eq!(first["EventHandlerServiceMethod"], "");
        assert_eq!(first["EventTriggerArea"], "UI");
        assert_eq!(first["EventTriggerEvent"], "Clicked");
        assert_eq!(first["EventTriggerId"], "ptcsbutton-6");
        assert_eq!(first["EventTriggerSection"], "");
        assert!(first["Id"]
            .as_str()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()));
        let described: Vec<(String, String)> =
            crate::core::mashup_describe::describe(&solution, "Acme.Orders_MU")
                .unwrap()
                .events
                .into_iter()
                .map(|event| (event.trigger, event.handler))
                .collect();
        assert_eq!(
            described,
            rows.map(|(event, invoke)| (event.into(), invoke.into()))
        );
        let failing: Vec<_> = crate::core::check::run(&solution)
            .gates
            .into_iter()
            .filter(|gate| !gate.ok() && gate.name != "project")
            .collect();
        assert!(failing.is_empty(), "{failing:?}");
    }

    #[test]
    fn event_authoring_refuses_unverified_wiring() {
        let (dir, solution) = bind_fixture();
        let button = plan_add(&solution, &add_request("Acme.Orders_MU", "button")).unwrap();
        apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
        let code = |event: &str, invoke: &str| on(&solution, event, invoke).unwrap_err().code();
        assert_eq!(
            code("ui:ptcsbutton-6.Nope", "data:Acme.Orders.Manager.SaveOrder"),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            code("mashup:Unloaded", "data:Acme.Orders.Manager.SaveOrder"),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            code(
                "data:Acme.Orders.Manager.SaveOrder.ServiceInvokeCompleted",
                "data:Acme.Orders.Manager.GetOrders"
            ),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            code("ui:ptcsbutton-6.Clicked", "ui:ptcsbutton-6.Nope"),
            ErrorCode::InvalidArguments
        );
        let plan = on(
            &solution,
            "ui:ptcsbutton-6.Clicked",
            "data:Acme.Orders.Manager.SaveOrder",
        )
        .unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        assert_eq!(
            code(
                "ui:ptcsbutton-6.Clicked",
                "data:Acme.Orders.Manager.SaveOrder"
            ),
            ErrorCode::AlreadyExists
        );
        let content = sidecar_content(&solution).replace("\"ptcsbutton\"", "\"acmewidget\"");
        std::fs::write(
            dir.path().join("src/Acme.Orders_MU/mashup/content.json"),
            &content,
        )
        .unwrap();
        let entity_path = dir.path().join("acme/Mashups/Acme.Orders_MU.xml");
        let css = std::fs::read_to_string(dir.path().join("src/Acme.Orders_MU/mashup/custom.css"))
            .unwrap();
        let (entity, _) = mashup::sync(
            &std::fs::read(&entity_path).unwrap(),
            &mashup::Assets { content, css },
        )
        .unwrap();
        std::fs::write(entity_path, entity).unwrap();
        assert_eq!(
            code(
                "ui:ptcsbutton-6.Clicked",
                "data:Acme.Orders.Manager.SaveOrder"
            ),
            ErrorCode::GuardRefused
        );
    }

    #[test]
    fn a_widget_service_handler_is_addressed_by_the_widget() {
        let (_dir, solution) = bind_fixture();
        let button = plan_add(&solution, &add_request("Acme.Orders_MU", "button")).unwrap();
        apply_edit(&solution, &button.edit, &lock(&solution)).unwrap();
        let plan = on(&solution, "ui:ptcsbutton-6.Clicked", "ui:ptcsgrid-5.Reset").unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
        let event = &value["Events"][0];
        assert_eq!(event["EventHandlerArea"], "UI");
        assert_eq!(event["EventHandlerId"], "ptcsgrid-5");
        assert_eq!(event["EventHandlerService"], "Reset");
        let described =
            crate::core::mashup_describe::describe(&solution, "Acme.Orders_MU").unwrap();
        assert_eq!(described.events[0].handler, "ui:ptcsgrid-5.Reset");
    }

    /// Change the mashup's content in both its sidecar and its entity file, so they agree.
    fn rewrite_content(dir: &Path, edit: impl FnOnce(&mut Value)) {
        let content_path = dir.join("src/Acme.Orders_MU/mashup/content.json");
        let mut value: Value =
            serde_json::from_str(&std::fs::read_to_string(&content_path).unwrap()).unwrap();
        edit(&mut value);
        let content = mashup_edit::parse(&serde_json::to_string(&value).unwrap())
            .unwrap()
            .to_content();
        std::fs::write(&content_path, &content).unwrap();
        let entity_path = dir.join("acme/Mashups/Acme.Orders_MU.xml");
        let css =
            std::fs::read_to_string(dir.join("src/Acme.Orders_MU/mashup/custom.css")).unwrap();
        let (entity, _) = mashup::sync(
            &std::fs::read(&entity_path).unwrap(),
            &mashup::Assets { content, css },
        )
        .unwrap();
        std::fs::write(&entity_path, entity).unwrap();
    }

    #[test]
    fn named_calls_are_bound_and_invoked_by_name_and_several_calls_need_one() {
        let (_dir, solution) = bind_fixture();
        let plan = bind(
            &solution,
            "data:Acme.Orders.Manager.GetOrders[as orders]",
            "ui:ptcsgrid-5.Data",
        )
        .unwrap();
        assert_eq!(
            plan.new_source.as_deref(),
            Some("Acme.Orders.Manager.GetOrders")
        );
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        // One call of the service: an address without a name finds it.
        let plan = bind(
            &solution,
            "data:Acme.Orders.Manager.GetOrders[SelectedRows].note",
            "ui:ptcslabel-4.LabelText",
        )
        .unwrap();
        assert_eq!(plan.new_source, None);
        // A name another service's call has is refused, not given to a second call.
        let error = bind(
            &solution,
            "ui:ptcstextfield-3.Text",
            "data:Acme.Orders.Manager.SaveOrder[as orders].note",
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::AlreadyExists, "{error}");
        assert!(error.to_string().contains("for GetOrders"), "{error}");
        let plan = bind(
            &solution,
            "data:Acme.Orders.Manager.GetOrders[as second][SelectedRows].note",
            "ui:ptcslabel-4.LabelText",
        )
        .unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        let plan = on(
            &solution,
            "mashup:Loaded",
            "data:Acme.Orders.Manager.GetOrders[as second]",
        )
        .unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
        let calls: Vec<(&str, &str)> = value["Data"]["Things_Acme.Orders.Manager"]["Services"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| {
                (
                    call["Name"].as_str().unwrap(),
                    call["Target"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(calls, [("orders", "GetOrders"), ("second", "GetOrders")]);
        assert_eq!(value["Events"][0]["EventHandlerService"], "second");
        // Two calls of GetOrders: an address must say which.
        let errors = [
            bind(
                &solution,
                "data:Acme.Orders.Manager.GetOrders",
                "ui:ptcsgrid-5.Data",
            )
            .unwrap_err(),
            on(
                &solution,
                "mashup:Loaded",
                "data:Acme.Orders.Manager.GetOrders",
            )
            .unwrap_err(),
            on(
                &solution,
                "data:Acme.Orders.Manager.GetOrders.ServiceInvokeCompleted",
                "data:Acme.Orders.Manager.SaveOrder",
            )
            .unwrap_err(),
        ];
        for error in errors {
            assert_eq!(error.code(), ErrorCode::Ambiguous, "{error}");
            assert!(error.to_string().contains("orders, second"), "{error}");
        }
        // What describe prints, bind and on parse.
        let described =
            crate::core::mashup_describe::describe(&solution, "Acme.Orders_MU").unwrap();
        assert_eq!(
            described.bindings[1].from,
            "data:Acme.Orders.Manager.GetOrders[as second][SelectedRows].note"
        );
        assert_eq!(
            described.events[0].handler,
            "data:Acme.Orders.Manager.GetOrders[as second]"
        );
    }

    #[test]
    fn a_result_field_named_like_a_parameter_is_a_source_and_a_parameter_is_not() {
        let (_dir, solution) = bind_fixture();
        let plan = bind(
            &solution,
            "data:Acme.Orders.Manager.GetOrder[SelectedRows].id",
            "ui:ptcslabel-4.LabelText",
        )
        .unwrap();
        let value: Value =
            serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
        let map = &value["DataBindings"][0]["PropertyMaps"][0];
        assert_eq!(map["SourcePropertyType"], "Field");
        assert_eq!(map["SourcePropertyBaseType"], "LONG");
        let error = bind(
            &solution,
            "data:Acme.Orders.Manager.SaveOrder.note",
            "ui:ptcslabel-4.LabelText",
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArguments, "{error}");
        assert!(error.to_string().contains("is a parameter"), "{error}");
    }

    #[test]
    fn a_mashup_parameter_binds_to_a_service_parameter() {
        let (dir, solution) = bind_fixture();
        let entity_path = dir.path().join("acme/Mashups/Acme.Orders_MU.xml");
        let entity = std::fs::read_to_string(&entity_path).unwrap().replace(
            "<ParameterDefinitions></ParameterDefinitions>",
            r#"<ParameterDefinitions><FieldDefinition name="OrderId" baseType="LONG" aspect.bindingDirection="IN"/></ParameterDefinitions>"#,
        );
        std::fs::write(&entity_path, entity).unwrap();
        let plan = bind(
            &solution,
            "mashup:OrderId",
            "data:Acme.Orders.Manager.SaveOrder.id",
        )
        .unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        let value: Value = serde_json::from_str(&sidecar_content(&solution)).unwrap();
        let binding = &value["DataBindings"][0];
        assert_eq!(binding["SourceArea"], "Mashup");
        assert_eq!(binding["SourceId"], mashup_edit::ROOT_ID);
        assert_eq!(binding["PropertyMaps"][0]["SourceProperty"], "OrderId");
        assert_eq!(binding["PropertyMaps"][0]["TargetProperty"], "id");
        assert_eq!(
            crate::core::mashup_describe::describe(&solution, "Acme.Orders_MU")
                .unwrap()
                .bindings[0]
                .from,
            "mashup:OrderId"
        );
        assert_eq!(
            bind(
                &solution,
                "mashup:Nope",
                "data:Acme.Orders.Manager.SaveOrder.id"
            )
            .unwrap_err()
            .code(),
            ErrorCode::UnknownEntity
        );
    }

    #[test]
    fn a_second_source_for_one_target_is_bound_with_a_warning() {
        let (_dir, solution) = bind_fixture();
        let first = bind(
            &solution,
            "ui:ptcstextfield-3.Text",
            "ui:ptcslabel-4.LabelText",
        )
        .unwrap();
        assert!(first.warnings.is_empty());
        apply_edit(&solution, &first.edit, &lock(&solution)).unwrap();
        let second = bind(
            &solution,
            "data:Acme.Orders.Manager.GetOrders[SelectedRows].note",
            "ui:ptcslabel-4.LabelText",
        )
        .unwrap();
        assert_eq!(second.warnings.len(), 1, "{:?}", second.warnings);
        assert!(second.warnings[0].contains("ui:ptcslabel-4.LabelText already has another source"));
    }

    #[test]
    fn wiring_names_widgets_exactly_and_does_not_guess_by_catalogue_search() {
        let (_dir, solution) = bind_fixture();
        let document = mashup_edit::parse(&sidecar_content(&solution)).unwrap();
        // `add` would take "gri" for the grid; bind and on do not.
        assert_eq!(document.find("gri").unwrap().id, "ptcsgrid-5");
        let error = bind(
            &solution,
            "data:Acme.Orders.Manager.GetOrders",
            "ui:gri.Data",
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnknownEntity, "{error}");
        let error = on(
            &solution,
            "ui:gri.DoubleClicked",
            "data:Acme.Orders.Manager.SaveOrder",
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnknownEntity, "{error}");
        // A word the catalogue resolves still names the only widget of its type.
        bind(
            &solution,
            "data:Acme.Orders.Manager.GetOrders",
            "ui:grid.Data",
        )
        .unwrap();
    }

    #[test]
    fn a_shape_service_already_called_in_the_mashup_is_reused() {
        let (dir, solution) = bind_fixture();
        rewrite_content(dir.path(), |value| {
            value["Data"] = json!({
                "ThingShapes_Acme.Orders.Audit_TS": {
                    "DataName": "ThingShapes_Acme.Orders.Audit_TS",
                    "EntityName": "Acme.Orders.Audit_TS",
                    "EntityType": "ThingShapes",
                    "Id": "a1",
                    "RefreshInterval": 0,
                    "Services": [{"Name": "GetLog", "Target": "GetLog", "Parameters": {}}]
                }
            });
        });
        let plan = bind(
            &solution,
            "data:Acme.Orders.Audit_TS.GetLog",
            "ui:ptcsgrid-5.Data",
        )
        .unwrap();
        assert_eq!(plan.new_source, None);
        let value: Value =
            serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
        assert_eq!(
            value["DataBindings"][0]["SourceSection"],
            "ThingShapes_Acme.Orders.Audit_TS"
        );
        assert_eq!(value["DataBindings"][0]["SourceId"], "GetLog");
    }

    #[test]
    fn planning_add_bind_and_on_writes_nothing() {
        use crate::core::commands::{self, Mode, Notices};
        fn snapshot(dir: &Path, files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    snapshot(&path, files);
                } else {
                    files.insert(path.clone(), std::fs::read(&path).unwrap());
                }
            }
        }
        let (dir, solution) = bind_fixture();
        let mut before = std::collections::BTreeMap::new();
        snapshot(dir.path(), &mut before);
        commands::mashup::add(
            &solution,
            &commands::mashup::MashupAddRequest {
                request: add_request("Acme.Orders_MU", "button"),
                mode: Mode::Plan,
                lock_label: "test",
            },
            &mut Notices::default(),
        )
        .unwrap();
        commands::mashup::bind(
            &solution,
            &commands::mashup::MashupBindRequest {
                request: BindRequest {
                    mashup: "Acme.Orders_MU".into(),
                    from: "data:Acme.Orders.Manager.GetOrders".into(),
                    to: "ui:ptcsgrid-5.Data".into(),
                },
                mode: Mode::Plan,
                lock_label: "test",
            },
            &mut Notices::default(),
        )
        .unwrap();
        commands::mashup::on(
            &solution,
            &commands::mashup::MashupOnRequest {
                request: OnRequest {
                    mashup: "Acme.Orders_MU".into(),
                    event: "mashup:Loaded".into(),
                    invoke: "data:Acme.Orders.Manager.GetOrders".into(),
                },
                mode: Mode::Plan,
                lock_label: "test",
            },
            &mut Notices::default(),
        )
        .unwrap();
        let mut after = std::collections::BTreeMap::new();
        snapshot(dir.path(), &mut after);
        assert!(before == after, "planning changed a file");
    }

    fn param_request(mashup: &str, action: ParameterAction, name: &str) -> ParameterRequest {
        ParameterRequest {
            mashup: mashup.into(),
            action,
            name: name.into(),
            base_type: None,
            direction: None,
            description: None,
            default: ParameterDefault::Unchanged,
            data_shape: None,
            force: false,
        }
    }

    fn param(
        solution: &Solution,
        request: &ParameterRequest,
    ) -> Result<ParameterPlan, AuthoringError> {
        let plan = plan_parameter(solution, request)?;
        apply_parameter(solution, &plan, &lock(solution))?;
        Ok(plan)
    }

    fn content_of(solution: &Solution, mashup: &str) -> Value {
        let text = workspace::read_mashup(&solution.src_root().join(format!("{mashup}/mashup")))
            .unwrap()
            .unwrap()
            .content;
        serde_json::from_str(&text).unwrap()
    }

    /// Change a mashup's content in its sidecar and entity file together.
    fn rewrite_mashup(dir: &Path, mashup: &str, edit: impl FnOnce(&mut Value)) {
        let content_path = dir.join(format!("src/{mashup}/mashup/content.json"));
        let mut value: Value =
            serde_json::from_str(&std::fs::read_to_string(&content_path).unwrap()).unwrap();
        edit(&mut value);
        let content = mashup_edit::parse(&serde_json::to_string(&value).unwrap())
            .unwrap()
            .to_content();
        std::fs::write(&content_path, &content).unwrap();
        let entity_path = dir.join(format!("acme/Mashups/{mashup}.xml"));
        let css =
            std::fs::read_to_string(dir.join(format!("src/{mashup}/mashup/custom.css"))).unwrap();
        let (entity, _) = mashup::sync(
            &std::fs::read(&entity_path).unwrap(),
            &mashup::Assets { content, css },
        )
        .unwrap();
        std::fs::write(&entity_path, entity).unwrap();
    }

    /// A child `Acme.Card_MU` with `id` (LONG, IN) whose content binds `mashup:id` into a label, and a
    /// parent `Acme.Board_MU` holding it in `mashupcontainer-9` with a stale cache (no entries), a
    /// static `id` of 7, and a binding from the container's `id` into a label.
    fn parent_and_child() -> (tempfile::TempDir, Solution) {
        let (dir, solution) = one_project();
        let mut child = request("Acme.Card_MU");
        child.params = vec!["id:LONG:IN".into()];
        apply_new(
            &solution,
            &plan_new(&solution, &child).unwrap(),
            &lock(&solution),
        )
        .unwrap();
        let label = plan_add(&solution, &add_request("Acme.Card_MU", "label")).unwrap();
        apply_edit(&solution, &label.edit, &lock(&solution)).unwrap();
        rewrite_mashup(dir.path(), "Acme.Card_MU", |value| {
            value["DataBindings"] = json!([{
                "Id": "child-1",
                "PropertyMaps": [{"SourceProperty": "id", "SourcePropertyBaseType": "LONG",
                    "SourcePropertyType": "Property", "TargetProperty": "LabelText",
                    "TargetPropertyBaseType": "STRING", "TargetPropertyType": "property"}],
                "SourceArea": "Mashup", "SourceDetails": "", "SourceId": "mashup-root", "SourceSection": "",
                "TargetArea": "UI", "TargetId": "ptcslabel-3", "TargetSection": ""
            }]);
        });
        apply_new(
            &solution,
            &plan_new(&solution, &request("Acme.Board_MU")).unwrap(),
            &lock(&solution),
        )
        .unwrap();
        let label = plan_add(&solution, &add_request("Acme.Board_MU", "label")).unwrap();
        apply_edit(&solution, &label.edit, &lock(&solution)).unwrap();
        rewrite_mashup(dir.path(), "Acme.Board_MU", |value| {
            value["UI"]["Widgets"][0]["Widgets"].as_array_mut().unwrap().push(json!({
                "Properties": {"Id": "mashupcontainer-9", "Type": "mashupcontainer",
                    "DisplayName": "mashupcontainer-9", "Name": "Acme.Card_MU", "ResponsiveLayout": true,
                    "MashupParameters": [{"BaseType": "LONG", "DefaultValue": 7, "Description": "",
                        "ParameterName": "id", "ParmDef": {"aspects": {"bindingDirection": "IN",
                        "isMandatory": "false"}, "baseType": "LONG", "description": "", "name": "id",
                        "ordinal": 0}}],
                    "id": 7},
                "Widgets": []
            }));
            value["DataBindings"] = json!([{
                "Id": "parent-1",
                "PropertyMaps": [{"SourceProperty": "id", "SourcePropertyBaseType": "LONG",
                    "SourcePropertyType": "property", "TargetProperty": "LabelText",
                    "TargetPropertyBaseType": "STRING", "TargetPropertyType": "property"}],
                "SourceArea": "UI", "SourceDetails": "", "SourceId": "mashupcontainer-9", "SourceSection": "",
                "TargetArea": "UI", "TargetId": "ptcslabel-3", "TargetSection": ""
            }]);
        });
        (dir, solution)
    }

    fn container(value: &Value) -> &Value {
        value["UI"]["Widgets"][0]["Widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["Properties"]["Type"] == "mashupcontainer")
            .map(|w| &w["Properties"])
            .unwrap()
    }

    fn contained_fixture() -> (tempfile::TempDir, Solution) {
        let (dir, solution) = one_project();
        let mut child = request("Acme.Card_MU");
        child.params = vec![
            "id:LONG:IN".into(),
            "title:STRING:BOTH".into(),
            "done:NUMBER:OUT".into(),
        ];
        apply_new(
            &solution,
            &plan_new(&solution, &child).unwrap(),
            &lock(&solution),
        )
        .unwrap();
        apply_new(
            &solution,
            &plan_new(&solution, &request("Acme.Board_MU")).unwrap(),
            &lock(&solution),
        )
        .unwrap();
        (dir, solution)
    }

    #[test]
    fn add_contained_in_center_writes_composer_cache_and_passes_the_project_gate() {
        let (_dir, solution) = contained_fixture();
        let plan = plan_add_contained(
            &solution,
            &AddContainedRequest {
                mashup: "Acme.Board_MU".into(),
                child: "Acme.Card_MU".into(),
                placement: None,
                settings: vec![("id".into(), "7".into())],
            },
        )
        .unwrap();
        assert_eq!(plan.placed.id, "mashupcontainer-3");
        let value: Value =
            serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
        let properties = &value["UI"]["Widgets"][0]["Widgets"][0]["Properties"];
        assert_eq!(properties["Name"], "Acme.Card_MU");
        assert_eq!(properties["ResponsiveLayout"], true);
        assert_eq!(properties["Margin"], "5");
        assert_eq!(properties["id"], 7);
        assert_eq!(properties["MashupParameters"][0]["DefaultValue"], 7);
        assert_eq!(
            properties["MashupParameters"][0],
            cache_entry(
                &ParameterDefinition {
                    name: "id".into(),
                    base_type: "LONG".into(),
                    direction: "IN".into(),
                    description: String::new(),
                    default: None,
                    data_shape: None,
                    visible: true
                },
                Some(json!(7))
            )
        );
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        let failures: Vec<_> = crate::core::check::run(&solution)
            .gates
            .into_iter()
            .filter(|gate| gate.name == "project" && !gate.ok())
            .collect();
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn add_contained_refuses_invalid_children_and_settings() {
        let (_dir, solution) = contained_fixture();
        let add = |child: &str, settings: Vec<(String, String)>| {
            plan_add_contained(
                &solution,
                &AddContainedRequest {
                    mashup: "Acme.Board_MU".into(),
                    child: child.into(),
                    placement: None,
                    settings,
                },
            )
        };
        assert_eq!(
            add("Acme.Nope_MU", vec![]).unwrap_err().code(),
            ErrorCode::UnknownEntity
        );
        assert_eq!(
            plan_add_contained(
                &solution,
                &AddContainedRequest {
                    mashup: "Acme.Board_MU".into(),
                    child: "Acme.Board_MU".into(),
                    placement: None,
                    settings: vec![]
                }
            )
            .unwrap_err()
            .code(),
            ErrorCode::GuardRefused
        );
        assert_eq!(
            add("Acme.Card_MU", vec![("done".into(), "1".into())])
                .unwrap_err()
                .code(),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            add("Acme.Card_MU", vec![("nope".into(), "1".into())])
                .unwrap_err()
                .code(),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            add("Acme.Card_MU", vec![("id".into(), "nope".into())])
                .unwrap_err()
                .code(),
            ErrorCode::InvalidArguments
        );
        let master = plan_new(
            &solution,
            &NewRequest {
                master: true,
                ..request("Acme.Master_MM")
            },
        )
        .unwrap();
        apply_new(&solution, &master, &lock(&solution)).unwrap();
        assert_eq!(
            add("Acme.Master_MM", vec![]).unwrap_err().code(),
            ErrorCode::GuardRefused
        );
    }

    #[test]
    fn add_contained_beside_a_label_uses_the_normal_placement_rules() {
        let (_dir, solution) = contained_fixture();
        let label = plan_add(&solution, &add_request("Acme.Board_MU", "label")).unwrap();
        apply_edit(&solution, &label.edit, &lock(&solution)).unwrap();
        let plan = plan_add_contained(
            &solution,
            &AddContainedRequest {
                mashup: "Acme.Board_MU".into(),
                child: "Acme.Card_MU".into(),
                placement: Some(Placement::Beside {
                    anchor: "ptcslabel-3".into(),
                    side: mashup_edit::Side::Right,
                }),
                settings: vec![],
            },
        )
        .unwrap();
        let value: Value =
            serde_json::from_slice(&plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
        let ids = value["UI"]["Widgets"][0]["Widgets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|widget| widget["Properties"]["Id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["ptcslabel-3", "mashupcontainer-4"]);
    }

    #[test]
    fn contained_parameters_bind_refresh_and_events_follow_the_container_contract() {
        let (_dir, solution) = contained_fixture();
        let add = plan_add_contained(
            &solution,
            &AddContainedRequest {
                mashup: "Acme.Board_MU".into(),
                child: "Acme.Card_MU".into(),
                placement: None,
                settings: vec![],
            },
        )
        .unwrap();
        apply_edit(&solution, &add.edit, &lock(&solution)).unwrap();
        let add_sibling = |widget: &str, anchor: &str| {
            let mut request = add_request("Acme.Board_MU", widget);
            request.placement = Some(Placement::Beside {
                anchor: anchor.into(),
                side: mashup_edit::Side::Right,
            });
            let plan = plan_add(&solution, &request).unwrap();
            let id = plan.placed.id.clone();
            apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
            id
        };
        let text = add_sibling("text-field", "mashupcontainer-3");
        let label = add_sibling("label", &text);
        let button = add_sibling("button", &label);
        rewrite_mashup(_dir.path(), "Acme.Board_MU", |value| {
            fn clear(widget: &mut Value) {
                if widget["Properties"]["Id"] == "mashupcontainer-3" {
                    widget["Properties"]["MashupParameters"] = json!([]);
                }
                if let Some(children) = widget["Widgets"].as_array_mut() {
                    for child in children {
                        clear(child);
                    }
                }
            }
            clear(&mut value["UI"]);
        });
        let bind_board = |from: String, to: String| {
            plan_bind(
                &solution,
                &BindRequest {
                    mashup: "Acme.Board_MU".into(),
                    from,
                    to,
                },
            )
        };
        let title = bind_board(
            format!("ui:{text}.Text"),
            "ui:mashupcontainer-3.title".into(),
        )
        .unwrap();
        let value: Value =
            serde_json::from_slice(&title.edit.sidecars.as_ref().unwrap().content_after).unwrap();
        assert_eq!(value["DataBindings"][0]["TargetArea"], "UI");
        assert_eq!(value["DataBindings"][0]["TargetId"], "mashupcontainer-3");
        assert_eq!(
            value["DataBindings"][0]["PropertyMaps"][0]["TargetPropertyType"],
            "property"
        );
        assert_eq!(title.refreshed, ["mashupcontainer-3"]);
        assert_eq!(
            bind_board(
                "ui:mashupcontainer-3.done".into(),
                format!("ui:{label}.LabelText")
            )
            .unwrap()
            .edit
            .mashup,
            "Acme.Board_MU"
        );
        assert_eq!(
            bind_board(
                format!("ui:{text}.Text"),
                "ui:mashupcontainer-3.done".into()
            )
            .unwrap_err()
            .code(),
            ErrorCode::GuardRefused
        );
        assert_eq!(
            bind_board(
                "ui:mashupcontainer-3.id".into(),
                format!("ui:{label}.LabelText")
            )
            .unwrap_err()
            .code(),
            ErrorCode::GuardRefused
        );
        let unknown = bind_board(
            format!("ui:{text}.Text"),
            "ui:mashupcontainer-3.nope".into(),
        )
        .unwrap_err();
        assert_eq!(unknown.code(), ErrorCode::UnknownEntity);
        assert!(unknown.to_string().contains("id, title, done"));
        let on_plan = plan_on(
            &solution,
            &OnRequest {
                mashup: "Acme.Board_MU".into(),
                event: format!("ui:{button}.Clicked"),
                invoke: "ui:mashupcontainer-3.Refresh".into(),
            },
        )
        .unwrap();
        let event: Value =
            serde_json::from_slice(&on_plan.edit.sidecars.as_ref().unwrap().content_after).unwrap();
        assert_eq!(event["Events"][0]["EventHandlerArea"], "UI");
        assert_eq!(event["Events"][0]["EventHandlerId"], "mashupcontainer-3");
        assert_eq!(event["Events"][0]["EventHandlerService"], "Refresh");
        assert_eq!(
            plan_on(
                &solution,
                &OnRequest {
                    mashup: "Acme.Board_MU".into(),
                    event: "ui:mashupcontainer-3.Anything".into(),
                    invoke: "ui:mashupcontainer-3.Refresh".into()
                }
            )
            .unwrap_err()
            .code(),
            ErrorCode::InvalidArguments
        );
    }

    #[test]
    fn hidden_and_config_form_child_parameters_are_not_offered_or_bindable() {
        let (dir, solution) = contained_fixture();
        rewrite_mashup(dir.path(), "Acme.Card_MU", |value| {
            value["UI"]["Properties"]["_currentParameterDefs"].as_array_mut().unwrap().push(json!({
                "ParameterName": "ConfigForm_mode", "BaseType": "STRING", "Description": "", "Aspects": {"bindingDirection": "BOTH"}
            }));
            value["UI"]["Properties"]["_currentParameterDefs"][1]["Aspects"]["isVisible"] =
                json!(false);
        });
        let plan = plan_add_contained(
            &solution,
            &AddContainedRequest {
                mashup: "Acme.Board_MU".into(),
                child: "Acme.Card_MU".into(),
                placement: None,
                settings: vec![],
            },
        )
        .unwrap();
        apply_edit(&solution, &plan.edit, &lock(&solution)).unwrap();
        let text = plan_add(
            &solution,
            &AddRequest {
                mashup: "Acme.Board_MU".into(),
                widget: "text-field".into(),
                placement: Some(Placement::Beside {
                    anchor: "mashupcontainer-3".into(),
                    side: mashup_edit::Side::Right,
                }),
                label: None,
                properties: vec![],
            },
        )
        .unwrap();
        apply_edit(&solution, &text.edit, &lock(&solution)).unwrap();
        let error = plan_bind(
            &solution,
            &BindRequest {
                mashup: "Acme.Board_MU".into(),
                from: format!("ui:{}.Text", text.placed.id),
                to: "ui:mashupcontainer-3.title".into(),
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnknownEntity);
        assert!(!error.to_string().contains("title, "), "{error}");
        let error = plan_bind(
            &solution,
            &BindRequest {
                mashup: "Acme.Board_MU".into(),
                from: format!("ui:{}.Text", text.placed.id),
                to: "ui:mashupcontainer-3.ConfigForm_mode".into(),
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnknownEntity);
    }

    #[test]
    fn adding_a_parameter_refreshes_every_container_of_the_mashup_and_keeps_static_values() {
        let (_dir, solution) = parent_and_child();
        let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
        add.base_type = Some("STRING".into());
        let plan = param(&solution, &add).unwrap();
        assert_eq!(
            plan.refreshed,
            [("mashupcontainer-9".to_string(), "Acme.Card_MU".to_string())]
        );
        let parent = content_of(&solution, "Acme.Board_MU");
        let cache = &container(&parent)["MashupParameters"];
        assert_eq!(
            cache[1],
            json!({"BaseType": "STRING", "Description": "", "ParameterName": "title",
                "ParmDef": {"aspects": {"bindingDirection": "BOTH", "isMandatory": "false"},
                "baseType": "STRING", "description": "", "name": "title", "ordinal": 0}})
        );
        assert_eq!(
            serde_json::to_string(&cache[1]).unwrap(),
            r#"{"BaseType":"STRING","Description":"","ParameterName":"title","ParmDef":{"aspects":{"bindingDirection":"BOTH","isMandatory":"false"},"baseType":"STRING","description":"","name":"title","ordinal":0}}"#,
            "key order as Composer writes it"
        );
        // The unrelated `id` keeps the static value the container passes.
        assert_eq!(cache[0]["DefaultValue"], 7);
        assert_eq!(container(&parent)["id"], 7);
        // Both mashups changed in one plan, and the result passes the gates.
        assert_eq!(plan.edits.len(), 2);
        let failing: Vec<_> = crate::core::check::run(&solution)
            .gates
            .into_iter()
            .filter(|gate| !gate.ok() && gate.name != "project")
            .collect();
        assert!(failing.is_empty(), "{failing:?}");
        let described = crate::core::mashup_describe::describe(&solution, "Acme.Board_MU").unwrap();
        assert_eq!(described.containers[0].mashup, "Acme.Card_MU");
        assert!(described.containers[0]
            .parameters
            .iter()
            .all(|parameter| parameter.status == "ok"));
    }

    #[test]
    fn describe_reports_a_container_cache_that_misses_a_parameter() {
        let (dir, solution) = parent_and_child();
        rewrite_mashup(dir.path(), "Acme.Board_MU", |value| {
            value["UI"]["Widgets"][0]["Widgets"][1]["Properties"]["MashupParameters"] = json!([]);
        });
        let described = crate::core::mashup_describe::describe(&solution, "Acme.Board_MU").unwrap();
        assert_eq!(
            described.containers[0].parameters[0].status,
            "missing in cache"
        );
    }

    #[test]
    fn a_type_change_retypes_bindings_in_both_mashups_and_drops_the_stale_static_value() {
        let (_dir, solution) = parent_and_child();
        let mut set = param_request("Acme.Card_MU", ParameterAction::Set, "id");
        set.base_type = Some("STRING".into());
        let plan = param(&solution, &set).unwrap();
        assert_eq!(plan.retyped, ["child-1", "parent-1"]);
        let child = content_of(&solution, "Acme.Card_MU");
        assert_eq!(
            child["DataBindings"][0]["PropertyMaps"][0]["SourcePropertyBaseType"],
            "STRING"
        );
        assert_eq!(
            child["UI"]["Properties"]["_currentParameterDefs"][0]["BaseType"],
            "STRING"
        );
        let entity =
            std::fs::read_to_string(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
        assert!(entity.contains(
            r#"<FieldDefinition aspect.bindingDirection="IN" aspect.isMandatory="false" baseType="STRING" description="" name="id" ordinal="0"/>"#
        ));
        let parent = content_of(&solution, "Acme.Board_MU");
        assert_eq!(
            parent["DataBindings"][0]["PropertyMaps"][0]["SourcePropertyBaseType"],
            "STRING"
        );
        let properties = container(&parent);
        assert!(
            properties.get("id").is_none(),
            "a LONG static value is not a STRING one"
        );
        assert_eq!(properties["MashupParameters"][0]["BaseType"], "STRING");
        assert!(properties["MashupParameters"][0]
            .get("DefaultValue")
            .is_none());
    }

    #[test]
    fn a_type_change_that_breaks_a_binding_is_refused_and_writes_nothing() {
        let (dir, solution) = parent_and_child();
        rewrite_mashup(dir.path(), "Acme.Card_MU", |value| {
            let map = &mut value["DataBindings"][0]["PropertyMaps"][0];
            map["TargetPropertyBaseType"] = json!("LONG");
        });
        let before = std::fs::read(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
        let mut set = param_request("Acme.Card_MU", ParameterAction::Set, "id");
        set.base_type = Some("BOOLEAN".into());
        let error = plan_parameter(&solution, &set).unwrap_err();
        assert_eq!(error.code(), ErrorCode::GuardRefused, "{error}");
        assert!(error.to_string().contains("child-1"), "{error}");
        assert_eq!(
            std::fs::read(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap(),
            before
        );
    }

    #[test]
    fn removing_a_bound_parameter_needs_force_which_removes_the_bindings_and_cache_entries() {
        let (_dir, solution) = parent_and_child();
        let remove = param_request("Acme.Card_MU", ParameterAction::Remove, "id");
        let error = plan_parameter(&solution, &remove).unwrap_err();
        assert_eq!(error.code(), ErrorCode::GuardRefused, "{error}");
        assert!(error.to_string().contains("child-1"), "{error}");
        let plan = param(
            &solution,
            &ParameterRequest {
                force: true,
                ..remove
            },
        )
        .unwrap();
        assert_eq!(plan.removed, ["child-1", "parent-1"]);
        let child = content_of(&solution, "Acme.Card_MU");
        assert_eq!(child["DataBindings"], json!([]));
        assert_eq!(
            child["UI"]["Properties"]["_currentParameterDefs"],
            json!([])
        );
        let entity = std::fs::read(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
        assert!(mashup_parameters(&entity).unwrap().is_empty());
        let parent = content_of(&solution, "Acme.Board_MU");
        assert_eq!(parent["DataBindings"], json!([]));
        let properties = container(&parent);
        assert_eq!(properties["MashupParameters"], json!([]));
        assert!(properties.get("id").is_none());
    }

    #[test]
    fn removing_keeps_the_order_of_the_other_root_properties() {
        let (_dir, solution) = parent_and_child();
        let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
        add.base_type = Some("STRING".into());
        add.default = ParameterDefault::Set("Orders".into());
        param(&solution, &add).unwrap();
        let keys = |value: &Value| -> Vec<String> {
            value["UI"]["Properties"]
                .as_object()
                .unwrap()
                .keys()
                .filter(|key| *key != "title")
                .cloned()
                .collect()
        };
        let before = keys(&content_of(&solution, "Acme.Card_MU"));
        param(
            &solution,
            &param_request("Acme.Card_MU", ParameterAction::Remove, "title"),
        )
        .unwrap();
        assert_eq!(keys(&content_of(&solution, "Acme.Card_MU")), before);
    }

    #[test]
    fn a_parent_changed_after_planning_stops_the_whole_write() {
        let (_dir, solution) = parent_and_child();
        let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
        add.base_type = Some("STRING".into());
        let plan = plan_parameter(&solution, &add).unwrap();
        let child_path = solution.root.join("acme/Mashups/Acme.Card_MU.xml");
        let child_before = std::fs::read(&child_path).unwrap();
        let parent_path = solution.root.join("acme/Mashups/Acme.Board_MU.xml");
        let mut parent = std::fs::read(&parent_path).unwrap();
        parent.extend_from_slice(b"\n");
        std::fs::write(&parent_path, &parent).unwrap();
        assert!(apply_parameter(&solution, &plan, &lock(&solution)).is_err());
        assert_eq!(std::fs::read(&child_path).unwrap(), child_before);
        assert_eq!(std::fs::read(&parent_path).unwrap(), parent);
    }

    #[test]
    fn an_infotable_parameter_needs_a_data_shape_from_the_solution() {
        let (dir, solution) = parent_and_child();
        let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "rows");
        add.base_type = Some("INFOTABLE".into());
        assert_eq!(
            plan_parameter(&solution, &add).unwrap_err().code(),
            ErrorCode::InvalidArguments
        );
        add.data_shape = Some("Acme.Orders.Row_DS".into());
        assert_eq!(
            plan_parameter(&solution, &add).unwrap_err().code(),
            ErrorCode::UnknownEntity
        );
        write(
            dir.path(),
            "acme/DataShapes/Acme.Orders.Row_DS.xml",
            r#"<Entities><DataShapes><DataShape name="Acme.Orders.Row_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="id" baseType="LONG"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
        );
        param(&solution, &add).unwrap();
        let entity =
            std::fs::read_to_string(dir.path().join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
        assert!(entity.contains(
            r#"<FieldDefinition aspect.bindingDirection="BOTH" aspect.dataShape="Acme.Orders.Row_DS" aspect.isMandatory="false" baseType="INFOTABLE" description="" name="rows" ordinal="0"/>"#
        ));
        let child = content_of(&solution, "Acme.Card_MU");
        assert_eq!(
            child["UI"]["Properties"]["_currentParameterDefs"][1]["Aspects"],
            json!({"bindingDirection": "BOTH", "dataShape": "Acme.Orders.Row_DS", "isMandatory": false})
        );
        let parent = content_of(&solution, "Acme.Board_MU");
        assert_eq!(
            container(&parent)["MashupParameters"][1]["ParmDef"]["aspects"]["dataShape"],
            "Acme.Orders.Row_DS"
        );
    }

    #[test]
    fn parameter_requests_twaco_cannot_stand_behind_are_refused() {
        let (_dir, solution) = parent_and_child();
        let code = |name: &str, base_type: Option<&str>, action: ParameterAction| {
            let mut request = param_request("Acme.Card_MU", action, name);
            request.base_type = base_type.map(str::to_string);
            plan_parameter(&solution, &request).unwrap_err().code()
        };
        assert_eq!(
            code("id", Some("LONG"), ParameterAction::Add),
            ErrorCode::AlreadyExists
        );
        assert_eq!(
            code("1x", Some("STRING"), ParameterAction::Add),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            code("Entity", Some("STRING"), ParameterAction::Add),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            code("x", Some("WORD"), ParameterAction::Add),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            code("x", None, ParameterAction::Add),
            ErrorCode::InvalidArguments
        );
        assert_eq!(
            code("nope", None, ParameterAction::Remove),
            ErrorCode::UnknownEntity
        );
        assert_eq!(
            code("nope", Some("STRING"), ParameterAction::Set),
            ErrorCode::UnknownEntity
        );
        let mut bad_default = param_request("Acme.Card_MU", ParameterAction::Add, "count");
        bad_default.base_type = Some("LONG".into());
        bad_default.default = ParameterDefault::Set("many".into());
        assert_eq!(
            plan_parameter(&solution, &bad_default).unwrap_err().code(),
            ErrorCode::InvalidArguments
        );
    }

    #[test]
    fn a_mashup_without_sidecars_still_refreshes_its_containers() {
        let (dir, solution) = parent_and_child();
        std::fs::remove_dir_all(dir.path().join("src/Acme.Card_MU")).unwrap();
        let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
        add.base_type = Some("STRING".into());
        let plan = param(&solution, &add).unwrap();
        assert!(plan.edits[0].sidecars.is_none());
        let names: Vec<_> = container(&content_of(&solution, "Acme.Board_MU"))["MashupParameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["ParameterName"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["id", "title"]);
    }

    #[test]
    fn template_containers_have_no_margin_and_their_widgets_keep_composers() {
        // A 5px margin, the 10.2 default nearly no real container keeps, shrinks every fixed band
        // by 10px and hides a 1px divider; Composer clears it on a container it drops.
        for template in mashup_templates::layouts().unwrap() {
            for master in [false, true] {
                let document = new_document(&template, master, DEFAULT_THEME).unwrap();
                for widget in document.widgets().into_iter().skip(1) {
                    let margin = &document.at(&widget.path)["Properties"]["Margin"];
                    if widget.type_id == "flexcontainer" {
                        assert_eq!(margin, "", "{} {}", template.name, widget.id);
                    } else {
                        assert_eq!(margin, "5", "{} {}", template.name, widget.id);
                    }
                }
            }
        }
    }

    #[test]
    fn a_menu_bar_sits_in_a_row_so_it_fills_the_sidebar_height() {
        for name in ["sidebar", "header-sidebar"] {
            let template = mashup_templates::layout(name).unwrap().unwrap();
            let document = new_document(&template, false, DEFAULT_THEME).unwrap();
            let sidebar = document.find("sidebar").unwrap();
            assert_eq!(
                document.at(&sidebar.path)["Properties"]["flex-direction"],
                "row",
                "{name}"
            );
        }
    }

    #[test]
    fn parameter_definitions_go_where_the_server_keeps_them_sorted_by_name_ignoring_case() {
        // A live 10.1 server read back `done, orderId, title` for definitions sent as
        // `orderId, title, done`; exports sort `cardWidth, Manager, mashupName` the same way.
        let (_dir, solution) = parent_and_child();
        for (name, base_type) in [("title", "STRING"), ("Alpha", "STRING"), ("done", "NUMBER")] {
            let mut add = param_request("Acme.Card_MU", ParameterAction::Add, name);
            add.base_type = Some(base_type.into());
            param(&solution, &add).unwrap();
        }
        let entity = std::fs::read(solution.root.join("acme/Mashups/Acme.Card_MU.xml")).unwrap();
        let names: Vec<String> = mashup_parameters(&entity)
            .unwrap()
            .into_iter()
            .map(|parameter| parameter.name)
            .collect();
        assert_eq!(names, ["Alpha", "done", "id", "title"]);
        // The content keeps the order they were made in, as the server does.
        let content: Vec<String> = content_of(&solution, "Acme.Card_MU")["UI"]["Properties"]
            ["_currentParameterDefs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["ParameterName"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(content, ["id", "title", "Alpha", "done"]);
    }

    #[test]
    fn parameter_definitions_join_the_root_properties_in_the_order_the_server_keeps() {
        // A live server read the content back with `_currentParameterDefs` before `id_index`.
        for master in [false, true] {
            let (_dir, solution) = one_project();
            let mut new = request("Acme.Sorted_MU");
            new.master = master;
            new.params = vec!["id:LONG".into()];
            let plan = plan_new(&solution, &new).unwrap();
            let value: Value = serde_json::from_str(&plan.assets.content).unwrap();
            let keys: Vec<&String> = value["UI"]["Properties"]
                .as_object()
                .unwrap()
                .keys()
                .collect();
            let mut sorted = keys.clone();
            sorted.sort();
            assert_eq!(keys, sorted, "master: {master}");
        }
    }

    #[test]
    fn describe_shows_each_container_with_its_child_parameters_and_their_bindings() {
        let (_dir, solution) = parent_and_child();
        let mut add = param_request("Acme.Card_MU", ParameterAction::Add, "title");
        add.base_type = Some("STRING".into());
        param(&solution, &add).unwrap();
        let description =
            crate::core::mashup_describe::describe(&solution, "Acme.Board_MU").unwrap();
        let text = crate::core::mashup_describe::render(&description);
        assert!(
            text.contains("mashupcontainer-9 shows Acme.Card_MU (id IN, title BOTH)"),
            "{text}"
        );
        assert!(
            text.contains("ui:mashupcontainer-9.id -> ui:ptcslabel-3.LabelText"),
            "{text}"
        );
    }
}
