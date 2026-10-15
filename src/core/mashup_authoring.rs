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
use crate::core::mashup_edit::{self, EditError, MashupDocument, Placement};
use crate::core::mashup_templates::{self, MashupKind, Visibility};
use crate::core::transaction::{Transaction, TransactionError};
use crate::core::{widgets, workspace};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The theme a new mashup uses unless asked for another: 10.2's default.
pub const DEFAULT_THEME: &str = "PTC Convergence Theme";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One `center` container.
    Blank,
}

impl Layout {
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "blank" => Some(Self::Blank),
            _ => None,
        }
    }

    pub const fn word(self) -> &'static str {
        match self {
            Self::Blank => "blank",
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewRequest {
    pub name: String,
    /// The project it belongs to; may be left out when the solution has one project.
    pub project: Option<String>,
    pub layout: Layout,
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

#[derive(Debug, thiserror::Error)]
pub enum AuthoringError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unknown(String),
    #[error("{0}")]
    Exists(String),
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

    let document = new_document(request.layout, request.master, &theme)?;
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
    })
}

/// The content of a new mashup.
fn new_document(
    layout: Layout,
    master: bool,
    theme: &str,
) -> Result<MashupDocument, AuthoringError> {
    let mut document = match layout {
        Layout::Blank => {
            mashup_edit::parse(mashup_templates::BLANK).map_err(AuthoringError::Edit)?
        }
    };
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
    })
}

/// Plan adding one catalogue widget to an existing mashup. Nothing is written.
pub fn plan_add(solution: &Solution, request: &AddRequest) -> Result<AddPlan, AuthoringError> {
    let entity = workspace::entities(solution)
        .into_iter()
        .find(|entity| entity.info.collection == "Mashups" && entity.info.name == request.mashup)
        .ok_or_else(|| {
            AuthoringError::Unknown(format!("no mashup {} in the solution", request.mashup))
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
                    "{}: content.json and the entity file disagree; run `twaco sync` (keeps the sidecars) or `twaco extract` (keeps the entity file) first",
                    request.mashup
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
    let mut document = mashup_edit::parse(&assets.content).map_err(AuthoringError::Edit)?;
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
    let content = document.to_content();
    let assets_after = mashup::Assets {
        content: content.clone(),
        css: assets.css.clone(),
    };
    let (entity_after, _) =
        mashup::sync(&entity_before, &assets_after).map_err(AuthoringError::Mashup)?;
    if mashup::extract(&entity_after)
        .map_err(AuthoringError::Mashup)?
        .content
        != content
    {
        return Err(AuthoringError::Invalid(format!(
            "{}: mashup content did not round-trip through the entity file",
            request.mashup
        )));
    }
    let sidecars = sidecars.map(|(content_path, content_before, css_path)| SidecarEdit {
        content_path,
        content_before,
        content_after: content.into_bytes(),
        css_path,
        css: assets.css,
    });
    Ok(AddPlan {
        edit: EditPlan {
            command: "mashup add",
            mashup: request.mashup.clone(),
            entity_path: entity.path,
            entity_before,
            entity_after,
            sidecars,
        },
        placed,
        widget: widget.type_id.clone(),
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
}
