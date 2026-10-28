//! `mashup new`: a mashup built from a layout template, with its entity file and sidecars.

use super::{
    bind_in, cache_entry, contained_parameters, load_existing, menu_service, on_in, parse_loaded,
    parse_new_parameter, set_content_parameter, splice_parameter_definition, AuthoringContext,
    AuthoringError, ParameterDefinition,
};
use crate::core::config::Solution;
use crate::core::entity::key::EntityKey;
use crate::core::lock::WorkspaceLock;
use crate::core::mashup::edit::MashupDocument;
use crate::core::mashup::templates::{MashupKind, Visibility};
use crate::core::transaction::Transaction;
use crate::core::{mashup, types, widgets, workspace};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The theme a new mashup uses unless asked for another: 10.2's default.
pub(crate) const DEFAULT_THEME: &str = "PTC Convergence Theme";

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
    /// `Label=Mashup` (or `Label` for an empty tab), one per tab, for a template with a tab set.
    pub tabs: Vec<String>,
}

#[derive(Debug)]
pub struct NewPlan {
    pub name: String,
    pub project: String,
    pub template: mashup::templates::LayoutTemplate,
    pub master: bool,
    pub theme: String,
    pub visibility: Vec<Visibility>,
    pub(crate) entity_path: PathBuf,
    pub(crate) content_path: PathBuf,
    pub(crate) css_path: PathBuf,
    pub entity: Vec<u8>,
    pub(crate) assets: mashup::Assets,
    /// The widgets the new mashup holds, root first.
    pub widgets: Vec<mashup::edit::WidgetRef>,
    /// Menu service calls wired while creating a sidebar.
    pub menus: Vec<(String, String)>,
    /// Each tab's label and the mashup it shows, for a template with a tab set.
    pub tabs: Vec<(String, Option<String>)>,
}

/// `Organization/<name>` or `OrganizationalUnit/<organization>:<unit>`.
pub(crate) fn parse_visibility(text: &str) -> Result<Visibility, AuthoringError> {
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
pub(crate) fn plan_new(
    solution: &Solution,
    request: &NewRequest,
) -> Result<NewPlan, AuthoringError> {
    let template = mashup::templates::layout(&request.template)
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
        .find(|e| e.info.collection == "Mashups" && e.info.name.eq_ignore_ascii_case(&request.name))
    {
        // ThingWorx refuses a name that differs from an existing one only in case.
        let how = if existing.info.name == request.name {
            String::new()
        } else {
            format!(
                " as {}, which ThingWorx takes for the same name",
                existing.info.name
            )
        };
        return Err(AuthoringError::Exists(format!(
            "mashup {} already exists{how}: {}",
            request.name,
            solution.relative(&existing.path)
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

    if request.master && template.master_slot.is_none() && !template.children.is_empty() {
        return Err(AuthoringError::Invalid(format!(
            "template {} has no place for a page container, so it cannot be a master",
            template.name
        )));
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
    let tabs = build_tabs(solution, &request.name, &mut document, &request.tabs)?;
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
        mashup::templates::entity_xml(&request.name, &project.name, kind, &visibility)
            .map_err(|e| AuthoringError::Invalid(e.to_string()))?;
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
        tabs,
    })
}

/// The content of a new mashup.
pub(super) fn new_document(
    template: &mashup::templates::LayoutTemplate,
    master: bool,
    theme: &str,
) -> Result<MashupDocument, AuthoringError> {
    let mut document =
        mashup::edit::parse(mashup::templates::BLANK).map_err(AuthoringError::Edit)?;
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
        let Value::Object(mut root) = mashup::templates::master_root() else {
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
        document.push_child(&center.path, mashup::edit::new_widget(page, number, true));
    }
    mashup::edit::insert_sorted(document.root_properties_mut(), "StyleTheme", json!(theme));
    Ok(document)
}

fn unknown_template(word: &str) -> String {
    let names = mashup::templates::layouts()
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
    node: &mashup::templates::LayoutNode,
    template: &str,
) -> Result<(), AuthoringError> {
    validate_node(node, template)?;
    let container = widgets::catalog()
        .by_type("flexcontainer")
        .expect("the catalogue has the flex container; a test checks it");
    let number = document.allocate();
    document.push_child(parent, mashup::edit::new_widget(container, number, true));
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
        document.push_child(&child.path, mashup::edit::new_widget(widget, number, true));
    }
    for grandchild in &node.children {
        add_template_node(document, &child.path, grandchild, template)?;
    }
    Ok(())
}

fn validate_node(
    node: &mashup::templates::LayoutNode,
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
pub(crate) fn validate_template(
    template: &mashup::templates::LayoutTemplate,
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
        nodes: &[mashup::templates::LayoutNode],
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
    // A template with no master slot cannot be a master mashup; one it names must exist.
    if template
        .master_slot
        .as_ref()
        .is_some_and(|slot| !template.children.is_empty() && !slots.contains(slot))
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
pub(crate) fn apply_new(
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
    let relative = |path: &Path| solution.relative(path);
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
pub fn template_slots(template: &mashup::templates::LayoutTemplate) -> Vec<String> {
    fn collect(nodes: &[mashup::templates::LayoutNode], out: &mut Vec<String>) {
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

/// Fill a template's tab set: one container per tab, holding the mashup the tab shows in a
/// contained mashup with its parameters cached, and the tab set's labels and `Items` as Composer
/// keeps them. No `--tab` leaves the two empty tabs a dropped tab set has.
fn build_tabs(
    solution: &Solution,
    mashup: &str,
    document: &mut MashupDocument,
    requested: &[String],
) -> Result<Vec<(String, Option<String>)>, AuthoringError> {
    let Some(tabset) = document
        .widgets()
        .into_iter()
        .find(|widget| widget.type_id == "ptcstabset")
    else {
        if requested.is_empty() {
            return Ok(Vec::new());
        }
        return Err(AuthoringError::Invalid(
            "--tab is for a template with a tab set (tabs)".into(),
        ));
    };
    let mut tabs = Vec::new();
    for text in requested {
        let (label, child) = match text.split_once('=') {
            Some((label, child)) => (label.trim(), Some(child.trim())),
            None => (text.trim(), None),
        };
        if label.is_empty() || child.is_some_and(str::is_empty) {
            return Err(AuthoringError::Invalid(format!(
                "--tab {text:?} must be Label=Mashup or Label"
            )));
        }
        tabs.push((label.to_string(), child.map(str::to_string)));
    }
    if tabs.is_empty() {
        tabs = (1..=2).map(|n| (format!("Tab Name {n}"), None)).collect();
    }
    if tabs.len() > 16 {
        return Err(AuthoringError::Invalid(
            "a tab set has at most 16 tabs".into(),
        ));
    }
    let catalog = widgets::catalog();
    let container = catalog
        .by_type("flexcontainer")
        .expect("the catalogue has the flex container; a test checks it");
    let contained = catalog
        .by_type("mashupcontainer")
        .expect("the catalogue has the contained mashup; a test checks it");
    let tabset_name = document.at(&tabset.path)["Properties"]["DisplayName"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for (index, (_, child)) in tabs.iter().enumerate() {
        let number = document.allocate();
        let mut tab = mashup::edit::new_widget(container, number, true);
        tab["Properties"]["DisplayName"] = json!(format!("{tabset_name} #{}", index + 1));
        mashup::edit::insert_sorted(
            mashup::edit::properties_mut(&mut tab),
            "userCannotRemove",
            json!(true),
        );
        if let Some(child) = child {
            if child == mashup {
                return Err(AuthoringError::Refused(
                    "a mashup cannot contain itself".into(),
                ));
            }
            let loaded = load_existing(solution, child)?;
            let child_document = parse_loaded(&loaded)?;
            if child_document.value()["mashupType"] == "targetmashup" {
                return Err(AuthoringError::Refused(format!(
                    "{child} is a master mashup, shown by a page container, not contained"
                )));
            }
            let definitions = contained_parameters(&loaded, &child_document)?;
            let mut widget = mashup::edit::new_widget(contained, document.allocate(), true);
            let properties = mashup::edit::properties_mut(&mut widget);
            mashup::edit::insert_sorted(properties, "Name", json!(child));
            mashup::edit::insert_sorted(
                properties,
                "MashupParameters",
                Value::Array(
                    definitions
                        .iter()
                        .map(|def| cache_entry(def, def.default.clone()))
                        .collect(),
                ),
            );
            tab["Widgets"] = json!([widget]);
        }
        document.push_child(&tabset.path, tab);
    }
    document.set_property(&tabset.path, "NumberOfTabs", json!(tabs.len()));
    let mut items = Vec::new();
    for (index, (label, _)) in tabs.iter().enumerate() {
        let n = index + 1;
        document.set_property(&tabset.path, &format!("Tab{n}Name"), json!(label));
        document.set_property(
            &tabset.path,
            &format!("Tab{n}Value"),
            json!(format!("TabValue{n}")),
        );
        items.push(json!({"disabled": false, "name": label, "value": format!("TabValue{n}"), "visible": true}));
    }
    document.set_property(&tabset.path, "Items", Value::Array(items));
    Ok(tabs)
}
