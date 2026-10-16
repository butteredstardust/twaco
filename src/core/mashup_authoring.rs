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
use crate::core::{bindings, catalog, types, widgets, workspace};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The theme a new mashup uses unless asked for another: 10.2's default.
pub const DEFAULT_THEME: &str = "PTC Convergence Theme";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One `center` container.
    Blank,
    /// A menu sidebar beside a center container.
    Sidebar,
    /// Fixed header and footer around a center container.
    HeaderFooter,
}

impl Layout {
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "blank" => Some(Self::Blank),
            "sidebar" => Some(Self::Sidebar),
            "header-footer" => Some(Self::HeaderFooter),
            _ => None,
        }
    }

    pub const fn word(self) -> &'static str {
        match self {
            Self::Blank => "blank",
            Self::Sidebar => "sidebar",
            Self::HeaderFooter => "header-footer",
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewRequest {
    pub name: String,
    /// The project it belongs to; may be left out when the solution has one project.
    pub project: Option<String>,
    pub layout: Layout,
    /// The required primary menu service for a sidebar layout.
    pub primary_menu: Option<String>,
    /// The optional secondary menu service for a sidebar layout.
    pub secondary_menu: Option<String>,
    /// A StyleTheme in the solution, or 10.2's default.
    pub theme: Option<String>,
    /// A master mashup: pages open inside it.
    pub master: bool,
    /// `Organization/<name>` or `OrganizationalUnit/<organization>:<unit>`; none unless asked.
    pub visibility: Vec<String>,
}

#[derive(Debug)]
pub struct NewPlan {
    pub name: String,
    pub project: String,
    pub layout: Layout,
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
    match request.layout {
        Layout::Sidebar if request.primary_menu.is_none() => {
            return Err(AuthoringError::Invalid(
                "--primary-menu is required for --layout sidebar".into(),
            ));
        }
        Layout::Sidebar => {}
        _ if request.primary_menu.is_some() || request.secondary_menu.is_some() => {
            return Err(AuthoringError::Invalid(
                "menus are for --layout sidebar".into(),
            ));
        }
        _ => {}
    }
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

    let mut document = new_document(request.layout, request.master, &theme)?;
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
            menu_service(&document, &context, service)?;
            bind_in(
                &mut document,
                &context,
                service,
                &format!("ui:ptcsmenubar-4.{property}"),
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
    let template = mashup_templates::entity_xml(&request.name, &project.name, kind, &visibility);
    let (entity, _) = mashup::sync(template.as_bytes(), &assets).map_err(AuthoringError::Mashup)?;
    Ok(NewPlan {
        name: request.name.clone(),
        project: project.name.clone(),
        layout: request.layout,
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
    layout: Layout,
    master: bool,
    theme: &str,
) -> Result<MashupDocument, AuthoringError> {
    let mut document = mashup_edit::parse(mashup_templates::BLANK).map_err(AuthoringError::Edit)?;
    let root = document.find("center").map_err(AuthoringError::Edit)?;
    match layout {
        Layout::Blank => {}
        Layout::Sidebar => {
            document.set_property(&root.path, "DisplayName", json!("page"));
            document.set_property(&root.path, "flex-direction", json!("row"));
            document.set_property(&root.path, "align-items", json!("stretch"));
            let container = widgets::catalog()
                .by_type("flexcontainer")
                .expect("the catalogue has the flex container; a test checks it");
            let sidebar_number = document.allocate();
            let mut sidebar = mashup_edit::new_widget(container, sidebar_number, true);
            let menu = widgets::catalog()
                .by_type("ptcsmenubar")
                .expect("the catalogue has the menu bar; a test checks it");
            let menu_number = document.allocate();
            sidebar["Widgets"] = json!([mashup_edit::new_widget(menu, menu_number, true)]);
            document.push_child(&root.path, sidebar);
            let sidebar = document
                .find(&format!("flexcontainer-{sidebar_number}"))
                .map_err(AuthoringError::Edit)?;
            document.set_property(&sidebar.path, "DisplayName", json!("sidebar"));
            document.set_property(&sidebar.path, "flex-direction", json!("column"));
            document.set_property(&sidebar.path, "flex-grow", json!(0));
            document.set_property(&sidebar.path, "align-items", json!("stretch"));
            let center_number = document.allocate();
            let center = mashup_edit::new_widget(container, center_number, true);
            document.push_child(&root.path, center);
            let center = document
                .find("flexcontainer-5")
                .map_err(AuthoringError::Edit)?;
            document.set_property(&center.path, "DisplayName", json!("center"));
            document.set_property(&center.path, "flex-direction", json!("column"));
            document.set_property(&center.path, "flex-grow", json!(1));
            document.set_property(&center.path, "align-items", json!("stretch"));
        }
        Layout::HeaderFooter => {
            document.set_property(&root.path, "DisplayName", json!("page"));
            document.set_property(&root.path, "flex-direction", json!("column"));
            document.set_property(&root.path, "align-items", json!("stretch"));
            let container = widgets::catalog()
                .by_type("flexcontainer")
                .expect("the catalogue has the flex container; a test checks it");
            for (name, height) in [("header", "50px"), ("center", ""), ("footer", "40px")] {
                let number = document.allocate();
                let child = mashup_edit::new_widget(container, number, true);
                document.push_child(&root.path, child);
                let child = document
                    .find(&format!("flexcontainer-{number}"))
                    .map_err(AuthoringError::Edit)?;
                document.set_property(&child.path, "DisplayName", json!(name));
                document.set_property(
                    &child.path,
                    "flex-direction",
                    json!(if name == "center" { "column" } else { "row" }),
                );
                document.set_property(
                    &child.path,
                    "align-items",
                    json!(if name == "center" {
                        "stretch"
                    } else {
                        "center"
                    }),
                );
                if name == "center" {
                    document.set_property(&child.path, "flex-grow", json!(1));
                } else {
                    document.set_property(&child.path, "flex-size", json!("fixed"));
                    document.set_property(
                        &child.path,
                        "iconClass",
                        json!("widgets-flexcontainer-fixed"),
                    );
                    document.set_property(&child.path, "flex-min-height", json!(height));
                    document.set_property(&child.path, "flex-max-height", json!(height));
                }
            }
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
        // Pages open in a page container that fills the center.
        let center = document.find("center").map_err(AuthoringError::Edit)?;
        let number = document.allocate();
        let page = widgets::catalog()
            .by_type("pagemashupcontainer")
            .expect("the catalogue has the page container; a test checks it");
        document.push_child(&center.path, mashup_edit::new_widget(page, number, true));
    }
    mashup_edit::insert_sorted(document.root_properties_mut(), "StyleTheme", json!(theme));
    Ok(document)
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
        "layout": plan.layout.word(),
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
    let result = bind_in(&mut document, &context, &request.from, &request.to)?;
    let edit = finish_edit(loaded, document, "mashup bind")?;
    Ok(BindPlan {
        edit,
        from: request.from.clone(),
        to: request.to.clone(),
        new_source: result.new_source,
        warnings: result.warnings,
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
}

/// Add one checked binding to an already loaded document.
fn bind_in(
    document: &mut MashupDocument,
    context: &AuthoringContext<'_>,
    from_raw: &str,
    to_raw: &str,
) -> Result<BindIn, AuthoringError> {
    let from_address =
        parse_address(from_raw).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let to_address = parse_address(to_raw).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    let from = resolve_end(
        document,
        context.parameters,
        context.model,
        context.platform,
        from_address,
        true,
    )?;
    let to = resolve_end(
        document,
        context.parameters,
        context.model,
        context.platform,
        to_address,
        false,
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
    document: &MashupDocument,
    parameters: &[crate::core::mashup_describe::MashupParameter],
    model: &types::Model,
    platform: Option<&types::Platform>,
    address: Address,
    source: bool,
) -> Result<End, AuthoringError> {
    match address {
        Address::Ui { widget, property } => {
            let found = document.find_exact(&widget).map_err(AuthoringError::Edit)?;
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
    document: &MashupDocument,
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
    let end = resolve_end(
        document,
        context.parameters,
        context.model,
        context.platform,
        address,
        true,
    )?;
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
            layout: Layout::Blank,
            primary_menu: None,
            secondary_menu: None,
            theme: None,
            master: false,
            visibility: Vec::new(),
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
                Layout::Sidebar,
                vec![
                    ("flexcontainer-2", "flexcontainer", "page", "row"),
                    ("flexcontainer-3", "flexcontainer", "sidebar", "column"),
                    ("ptcsmenubar-4", "ptcsmenubar", "ptcs-menubar-4", ""),
                    ("flexcontainer-5", "flexcontainer", "center", "column"),
                ],
            ),
            (
                Layout::HeaderFooter,
                vec![
                    ("flexcontainer-2", "flexcontainer", "page", "column"),
                    ("flexcontainer-3", "flexcontainer", "header", "row"),
                    ("flexcontainer-4", "flexcontainer", "center", "column"),
                    ("flexcontainer-5", "flexcontainer", "footer", "row"),
                ],
            ),
        ] {
            for master in [false, true] {
                let document = new_document(layout, master, DEFAULT_THEME).unwrap();
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
                    let at = if layout == Layout::Sidebar { 4 } else { 3 };
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
                layout: Layout::Sidebar,
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
                layout: Layout::Sidebar,
                ..request("Acme.Nav_MU")
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        let invalid = |primary_menu: &str| {
            plan_new(
                &solution,
                &NewRequest {
                    layout: Layout::Sidebar,
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
                layout: Layout::HeaderFooter,
                primary_menu: Some("data:Acme.Nav.Manager.GetTable".into()),
                ..request("Acme.Nav_MU")
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        assert_eq!(Layout::from_word("stacked"), None);
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
}
