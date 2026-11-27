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
use crate::core::{mashup, scan, splice, types, widgets, workspace};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The theme a new mashup uses unless asked for another: 10.2's default.
pub(crate) const DEFAULT_THEME: &str = "PTC Convergence Theme";

/// Style themes that ThingWorx 10.2 supplies even though they have no entity in a solution
/// export.  Keep this list in the authoring boundary: every caller which accepts a theme goes
/// through `validate_theme`.
pub(crate) const BUILTIN_THEMES: &[&str] = &[
    "Base Theme",
    "Composer Theme",
    "Legacy Styles Theme",
    DEFAULT_THEME,
    "PTC Convergence Theme - Green Accent",
];

#[derive(Clone, Debug)]
pub struct NewRequest {
    pub name: String,
    /// The project it belongs to; may be left out when the solution has one project.
    pub project: Option<String>,
    /// An embedded layout template name or alias.
    pub template: String,
    pub rules: Option<String>,
    pub gadget: bool,
    pub layout: Option<String>,
    /// The primary menu service for a template with a menu bar; it or `menu_entity` is required.
    pub primary_menu: Option<String>,
    /// A Menu entity as the primary menu instead of a service.
    pub menu_entity: Option<String>,
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
    /// An existing repository mashup whose design is copied verbatim.
    pub from: Option<String>,
    pub replacements: Vec<String>,
}

#[derive(Debug)]
pub struct NewPlan {
    pub name: String,
    pub notes: Vec<String>,
    /// The copied mashup, if this plan came from `--from`.
    pub from: Option<String>,
    /// Counts for the concise text report of a copied mashup.
    pub copy_summary: Option<CopySummary>,
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

#[derive(Debug)]
pub struct CopySummary {
    pub widgets: usize,
    pub data_calls: usize,
    pub bindings: usize,
    pub events: usize,
    pub parameters: usize,
    pub replacements: Vec<String>,
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
    let gadget_size = if request.gadget {
        for (set, flag) in [
            (request.template != "blank", "--template"),
            (request.master, "--master"),
            (request.from.is_some(), "--from"),
            (!request.tabs.is_empty(), "--tab"),
            (request.primary_menu.is_some(), "--primary-menu"),
            (request.menu_entity.is_some(), "--menu-entity"),
            (request.secondary_menu.is_some(), "--secondary-menu"),
            (!request.replacements.is_empty(), "--replace"),
        ] {
            if set {
                return Err(AuthoringError::Invalid(format!(
                    "--gadget cannot be used with {flag}"
                )));
            }
        }
        let layout = request.layout.as_deref().unwrap_or("1x1");
        if ![
            "1x1", "2x1", "4x1", "1x2", "2x2", "4x2", "1x4", "2x4", "4x4",
        ]
        .contains(&layout)
        {
            return Err(AuthoringError::Invalid(format!("--layout {layout:?} for --gadget must be one of 1x1, 2x1, 4x1, 1x2, 2x2, 4x2, 1x4, 2x4, 4x4")));
        }
        let (columns, rows) = layout.split_once('x').expect("validated size");
        Some((
            columns.parse::<u32>().unwrap(),
            rows.parse::<u32>().unwrap(),
        ))
    } else {
        if request.layout.is_some() {
            return Err(AuthoringError::Invalid(
                "gadget layout needs --gadget; use template for an ordinary mashup".into(),
            ));
        }
        None
    };
    if request.from.is_none() && !request.replacements.is_empty() {
        return Err(AuthoringError::Invalid("--replace needs --from".into()));
    }
    if request.rules.is_some() && (request.from.is_some() || request.gadget || request.master) {
        return Err(AuthoringError::Invalid(
            "--rules needs an ordinary new mashup without --from, --gadget or --master".into(),
        ));
    }
    let default_rules = if request.gadget || request.master {
        "legacy"
    } else {
        "v1"
    };
    let rules = match request.rules.as_deref().unwrap_or(default_rules) {
        "legacy" => mashup::layout::Version::Legacy,
        "v1" => mashup::layout::Version::V1,
        _ => {
            return Err(AuthoringError::Invalid(
                "--rules must be legacy or v1".into(),
            ))
        }
    };
    if request.from.is_some() {
        return plan_copy(solution, request);
    }
    let mut template = mashup::templates::layouts_with(mashup::layout::RuleSet::embedded(rules))
        .map_err(AuthoringError::Invalid)?
        .into_iter()
        .find(|t| t.name == request.template || t.aliases.contains(&request.template))
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
    let theme = request.theme.as_deref().map_or_else(
        || Ok(DEFAULT_THEME.to_string()),
        |theme| validate_theme(solution, theme),
    )?;
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
    let mut document = if let Some((columns, rows)) = gadget_size {
        template.name = format!("gadget {columns}x{rows}");
        let mut document = mashup::edit::parse(include_str!("../templates/gadget.content.json"))
            .map_err(AuthoringError::Edit)?;
        let properties = document.root_properties_mut();
        for (key, value) in [
            ("Columns", columns),
            ("Rows", rows),
            ("Width", columns * 240 + (columns - 1) * 10),
            ("Height", rows * 240 + (rows - 1) * 10),
        ] {
            properties.insert(key.into(), json!(value));
        }
        properties.insert("StyleTheme".into(), json!(theme));
        document
    } else {
        new_document(&template, request.master, &theme)?
    };
    if rules == mashup::layout::Version::V1 {
        document.set_top("twacoPageRules", json!("v1"));
        mashup::layout::finish_page(&mut document);
        mashup::tokens::Tokens::active(solution)
            .map_err(AuthoringError::Invalid)?
            .apply_page(&mut document)?;
    }
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
    match (
        menu.as_ref(),
        request.primary_menu.as_ref(),
        request.menu_entity.as_ref(),
    ) {
        (Some(_), None, None) => {
            return Err(AuthoringError::Invalid(format!(
                "--primary-menu <service> or --menu-entity <Menu> is required for --template {}",
                template.name
            )));
        }
        (Some(_), Some(_), Some(_)) => {
            return Err(AuthoringError::Invalid(
                "give --primary-menu or --menu-entity, not both".into(),
            ));
        }
        (Some(_), None, Some(_)) if request.secondary_menu.is_some() => {
            return Err(AuthoringError::Invalid(
                "--secondary-menu is a service menu beside --primary-menu; a --menu-entity menu bar has none".into(),
            ));
        }
        (None, _, _)
            if request.primary_menu.is_some()
                || request.secondary_menu.is_some()
                || request.menu_entity.is_some() =>
        {
            return Err(AuthoringError::Invalid(
                "menus are for a template with a menu bar (sidebar, header-sidebar)".into(),
            ));
        }
        _ => {}
    }
    if let Some(menu) = &menu {
        let path = document
            .find_exact(&menu.id)
            .map_err(AuthoringError::Edit)?
            .path;
        document.set_property(&path, "PrimaryNavIcons", Value::Bool(true));
        document.set_property(&path, "DisableResizing", Value::Bool(true));
    }
    let tabs = build_tabs(solution, &request.name, &mut document, &request.tabs)?;
    let mut menus = Vec::new();
    if let Some(entity) = &request.menu_entity {
        if !workspace::entities(solution)
            .iter()
            .any(|found| found.info.collection == "Menus" && found.info.name == *entity)
        {
            return Err(AuthoringError::Unknown(format!(
                "no Menu {} in the solution; create it with twaco new menu",
                workspace::quoted_selector(entity)
            )));
        }
        // A Menu entity is the menu bar's data source itself: no service, no load event.
        let id = menu.as_ref().expect("checked menu").id.clone();
        let path = document.find_exact(&id).map_err(AuthoringError::Edit)?.path;
        document.set_property(&path, "PrimaryNavMenu", Value::String(entity.clone()));
        document.set_property(&path, "DataSource", Value::String("menuentity".into()));
        menus.push((entity.clone(), "PrimaryNavMenu".to_string()));
    }
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
    let mut assets =
        mashup::assets_from_payload(&document.to_content()).map_err(AuthoringError::Mashup)?;
    if !request.gadget {
        remove_content_css(&mut assets);
    }
    let entity_template = if let Some((columns, rows)) = gadget_size {
        mashup::templates::gadget_entity_xml(
            &request.name,
            &project.name,
            &visibility,
            columns,
            rows,
        )
    } else {
        mashup::templates::entity_xml(&request.name, &project.name, kind, &visibility)
    }
    .map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    let (mut entity, _) =
        mashup::sync(entity_template.as_bytes(), &assets).map_err(AuthoringError::Mashup)?;
    for parameter in &parameters {
        entity = splice_parameter_definition(&entity, parameter, None, false)?;
    }
    Ok(NewPlan {
        name: request.name.clone(),
        notes: page_notes(solution, request)
            .into_iter()
            .chain(tabs.iter().filter_map(|(_, child)| {
                let child = child.as_ref()?;
                let loaded = load_existing(solution, child).ok()?;
                let document = parse_loaded(&loaded).ok()?;
                super::add::contained_page_warning(child, &document)
            }))
            .collect(),
        from: None,
        copy_summary: None,
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

fn page_notes(solution: &Solution, request: &NewRequest) -> Vec<String> {
    if request.master || request.gadget || request.from.is_some() {
        return Vec::new();
    }
    workspace::entities(solution).iter()
        .filter(|entity| entity.info.collection == "Mashups")
        .filter_map(|entity| {
            let loaded = super::load_existing(solution, &entity.info.name).ok()?;
            let document = super::parse_loaded(&loaded).ok()?;
            (document.value()["mashupType"] == "targetmashup").then(|| format!(
                "attach this page to a master with twaco mashup set {} mashup-root --prop Master={}",
                request.name, entity.info.name))
        }).collect()
}

/// Refuse themes Composer cannot resolve without creating an entity.
pub(crate) fn validate_theme(solution: &Solution, theme: &str) -> Result<String, AuthoringError> {
    if BUILTIN_THEMES.contains(&theme)
        || workspace::entities(solution)
            .iter()
            .any(|entity| entity.info.collection == "StyleThemes" && entity.info.name == theme)
    {
        Ok(theme.to_string())
    } else {
        Err(AuthoringError::Unknown(format!(
            "no StyleTheme {theme} in the solution; twaco does not create one (the default is {DEFAULT_THEME}){}",
            workspace::short_name_hint(workspace::entities(solution).iter().filter(|entity| entity.info.collection == "StyleThemes").map(|entity| entity.info.name.as_str()), theme)
        )))
    }
}

/// Copy an authored mashup without ever rebuilding its entity XML.  Only identity and the
/// deliberately non-portable permission material are spliced.
fn plan_copy(solution: &Solution, request: &NewRequest) -> Result<NewPlan, AuthoringError> {
    let from = request.from.as_deref().expect("checked");
    let from = from.strip_prefix("Mashups/").unwrap_or(from);
    // A copy keeps the reference's design; a flag that would shape a new one has no meaning.
    let conflicting = [
        (request.template != "blank", "--template"),
        (request.master, "--master"),
        (!request.params.is_empty(), "--param"),
        (!request.tabs.is_empty(), "--tab"),
        (request.primary_menu.is_some(), "--primary-menu"),
        (request.menu_entity.is_some(), "--menu-entity"),
        (request.secondary_menu.is_some(), "--secondary-menu"),
        (request.theme.is_some(), "--theme"),
    ];
    if let Some((_, flag)) = conflicting.iter().find(|(set, _)| *set) {
        return Err(AuthoringError::Invalid(format!(
            "--from copies {from} as it is; {flag} cannot be combined with it"
        )));
    }
    EntityKey::new("Mashups", request.name.as_str())
        .map_err(|error| AuthoringError::Invalid(format!("not a mashup name: {error}")))?;
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
                    "name the project the mashup belongs to with --project".into(),
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
    let visibility = request
        .visibility
        .iter()
        .map(|text| parse_visibility(text))
        .collect::<Result<Vec<_>, _>>()?;
    let loaded = load_existing(solution, from)?;
    let mut entity = copied_entity(
        &loaded.entity_before,
        &request.name,
        &project.name,
        &visibility,
    )?;
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
    }
    // The source has already been checked by load_existing. Re-extracting proves the spliced
    // XML still represents exactly the sidecars we will create.
    let extracted = mashup::extract(&entity).map_err(AuthoringError::Mashup)?;
    let difference = if !mashup::same_css(&extracted.css, &loaded.assets.css) {
        Some(css_difference(&loaded.assets.css, &extracted.css))
    } else {
        super::content_difference(
            &loaded.assets.content,
            &extracted.content,
            loaded.sidecars.is_some(),
        )
    };
    if let Some((path, before, after)) = difference {
        tracing::debug!(path = %path, "mashup copy round-trip differs");
        return Err(AuthoringError::Refused(format!(
            "{from}: copying its identity changed {path}: {} -> {}; twaco will not re-serialise it",
            short(&before),
            short(&after)
        )));
    }
    let mut assets = loaded.assets;
    let mut value: Value = serde_json::from_str(&assets.content).expect("checked content");
    let mut replacements = Vec::new();
    for replacement in &request.replacements {
        let (old, new) = replacement
            .split_once('=')
            .filter(|(old, _)| !old.is_empty())
            .ok_or_else(|| {
                AuthoringError::Invalid(
                    "--replace needs <old>=<new> with a nonempty old token".into(),
                )
            })?;
        let count = replace_content_tokens(&mut value, old, new);
        if count == 0 {
            let structural = contains_token(&value, old);
            let reason = if structural {
                "old token occurs only in structural references; replace content values instead"
            } else {
                "old token occurs nowhere in copied content"
            };
            return Err(AuthoringError::Invalid(format!(
                "--replace {replacement}: {reason}"
            )));
        }
        replacements.push(format!("{replacement}: {count} replacements"));
    }
    assets.content = mashup::to_sidecar(&value);
    remove_content_css(&mut assets);
    entity = mashup::sync(&entity, &assets)
        .map_err(AuthoringError::Mashup)?
        .0;
    // The sidecars are read back normalised the way the next command reads them; settle the
    // entity against that, so the copy is in step in this run and not only after a `sync`.
    let read_back = mashup::Assets {
        content: assets.content.replace("\r\n", "\n"),
        css: assets
            .css
            .trim_start_matches('\u{feff}')
            .replace("\r\n", "\n"),
    };
    for _ in 0..2 {
        let (settled, changes) =
            mashup::sync(&entity, &read_back).map_err(AuthoringError::Mashup)?;
        if changes.is_empty() {
            break;
        }
        entity = settled;
    }
    let (_, changes) = mashup::sync(&entity, &read_back).map_err(AuthoringError::Mashup)?;
    if !changes.is_empty() {
        return Err(AuthoringError::Refused(format!(
            "{from}: the copy's entity file and sidecars would disagree on {}; nothing was written",
            changes.join(", ")
        )));
    }
    let document = mashup::edit::parse(&assets.content).map_err(AuthoringError::Edit)?;
    let content = document.value();
    let copy_summary = CopySummary {
        widgets: document.widgets().len().saturating_sub(1),
        data_calls: content["Data"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, section)| section["Services"].as_array().map_or(0, Vec::len))
            .sum(),
        bindings: content["DataBindings"].as_array().map_or(0, Vec::len),
        events: content["Events"].as_array().map_or(0, Vec::len),
        replacements,
        parameters: crate::core::mashup::describe::mashup_parameters(&entity)
            .map_err(AuthoringError::Unreadable)?
            .len(),
    };
    let mut notes = page_notes(solution, request);
    notes.extend(super::copy_visibility_notes(
        solution,
        from,
        &request.name,
        visibility.is_empty(),
    )?);
    Ok(NewPlan {
        name: request.name.clone(),
        notes,
        from: Some(from.to_string()),
        copy_summary: Some(copy_summary),
        project: project.name.clone(),
        template: mashup::templates::layout("blank")
            .expect("valid blank template")
            .expect("embedded blank"),
        master: false,
        theme: String::new(),
        visibility,
        entity_path,
        content_path,
        css_path,
        entity,
        assets,
        widgets: document.widgets(),
        menus: Vec::new(),
        tabs: Vec::new(),
    })
}

fn copied_entity(
    src: &[u8],
    name: &str,
    project: &str,
    visibility: &[Visibility],
) -> Result<Vec<u8>, AuthoringError> {
    let scanned = scan::scan(src)
        .map_err(|e| AuthoringError::Unreadable(format!("mashup XML cannot be scanned: {e}")))?;
    let tokens = &scanned.tokens;
    let mashup = tokens
        .iter()
        .find(|token| token.kind == scan::Kind::Start && token.name.of(src) == b"Mashup")
        .ok_or_else(|| AuthoringError::Refused("reference has no Mashup element".into()))?;
    let mut edits = Vec::new();
    for (attribute, value) in [
        ("name", name),
        ("projectName", project),
        ("description", ""),
    ] {
        let span = scan::attribute(src, mashup, attribute)
            .map_err(|e| AuthoringError::Unreadable(e.to_string()))?
            .ok_or_else(|| {
                AuthoringError::Refused(format!("reference Mashup has no {attribute} attribute"))
            })?;
        let escaped =
            scan::escape_attribute(value).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
        edits.push(splice::Edit::new(span, escaped.into_bytes()));
    }
    // What the reference grants goes: every Principal inside its permission blocks, and a
    // configuration table that names principals. The blocks themselves stay as Composer writes
    // them (empty `<Create></Create>`, `<Visibility></Visibility>` and so on).
    let mut inside_permissions = 0usize;
    let mut principal_start: Option<usize> = None;
    let mut table_start: Option<usize> = None;
    let mut table_names_principal = false;
    for (index, token) in tokens.iter().enumerate() {
        let tag = token.name.of(src);
        let permissions = matches!(
            tag,
            b"RunTimePermissions" | b"DesignTimePermissions" | b"VisibilityPermissions"
        );
        match token.kind {
            scan::Kind::Start if permissions => inside_permissions += 1,
            scan::Kind::End if permissions => {
                inside_permissions = inside_permissions.saturating_sub(1)
            }
            scan::Kind::Start if inside_permissions > 0 && tag == b"Principal" => {
                principal_start = Some(index)
            }
            scan::Kind::End if tag == b"Principal" => {
                if let Some(start) = principal_start.take() {
                    let span = scan::Span::new(tokens[start].span.start, token.span.end);
                    edits.push(splice::Edit::new(span, Vec::new()));
                }
            }
            scan::Kind::Empty if inside_permissions > 0 && tag == b"Principal" => {
                edits.push(splice::Edit::new(token.span, Vec::new()));
            }
            scan::Kind::Start if inside_permissions == 0 && tag == b"ConfigurationTable" => {
                table_start = Some(index);
                table_names_principal = false;
            }
            // A Principal element in a table, not text that mentions one.
            scan::Kind::Start | scan::Kind::Empty
                if table_start.is_some() && tag == b"Principal" =>
            {
                table_names_principal = true
            }
            scan::Kind::End if tag == b"ConfigurationTable" => {
                if let Some(start) = table_start.take() {
                    if table_names_principal {
                        let span = scan::Span::new(tokens[start].span.start, token.span.end);
                        edits.push(splice::Edit::new(span, Vec::new()));
                    }
                }
            }
            _ => {}
        }
    }
    if !visibility.is_empty() {
        let at = tokens
            .iter()
            .find(|token| {
                token.kind == scan::Kind::Start
                    && token.name.of(src) == b"ConfigurationTableDefinitions"
            })
            .map(|token| token.span.start)
            .ok_or_else(|| {
                AuthoringError::Refused(
                    "reference has no ConfigurationTableDefinitions insertion point for visibility"
                        .into(),
                )
            })?;
        let principals = visibility
            .iter()
            .map(|principal| {
                Ok(format!(
                    "\n                <Principal isPermitted=\"true\" name=\"{}\" type=\"{}\"></Principal>",
                    scan::escape_attribute(&principal.name)
                        .map_err(|e| AuthoringError::Invalid(e.to_string()))?,
                    scan::escape_attribute(&principal.kind)
                        .map_err(|e| AuthoringError::Invalid(e.to_string()))?,
                ))
            })
            .collect::<Result<String, AuthoringError>>()?;
        edits.push(splice::Edit::new(
            scan::Span::new(at, at),
            format!("            <VisibilityPermissions>{principals}\n            </VisibilityPermissions>\n            "),
        ));
    }
    splice::splice(src, &edits)
        .map_err(|e| AuthoringError::Refused(format!("reference XML cannot be copied safely: {e}")))
}

/// The content of a new mashup.
pub(super) fn new_document(
    template: &mashup::templates::LayoutTemplate,
    master: bool,
    theme: &str,
) -> Result<MashupDocument, AuthoringError> {
    new_document_shell(template, master, theme, true)
}

pub(super) fn new_document_shell(
    template: &mashup::templates::LayoutTemplate,
    master: bool,
    theme: &str,
    page_host: bool,
) -> Result<MashupDocument, AuthoringError> {
    let mut document =
        mashup::edit::parse(mashup::templates::BLANK).map_err(AuthoringError::Edit)?;
    let root = document.find("center").map_err(AuthoringError::Edit)?;
    for (key, value) in &template.page {
        document.set_property(&root.path, key, value.clone());
    }
    if !template.children.is_empty() {
        document.set_property(&root.path, "DisplayName", json!("page"));
        document.at_mut(&root.path)["twacoTemplateName"] = json!(template.name);
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
    }
    if master && page_host {
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

pub(super) fn add_template_node(
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
    document.at_mut(&child.path)["twacoTemplateSlot"] = json!(true);
    for (key, value) in &node.properties {
        document.set_property(&child.path, key, value.clone());
    }
    if !node.style.is_empty() {
        let mut css = serde_json::Map::new();
        let mut sorted: Vec<_> = node.style.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (property, value) in sorted {
            css.insert(property.clone(), value.clone());
        }
        let entry = json!({"parts": {"$": {"$": css}}, "widget": "flex-container"});
        super::set::copy_style_entry(
            document,
            &format!("flexcontainer-{number}"),
            entry,
            None,
            &mut Vec::new(),
        );
    }
    if let Some(fixed) = node.fixed.as_deref().or(node.thickness.as_deref()) {
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
        if !node.properties.contains_key("flex-grow") {
            document.set_property(&child.path, "flex-grow", json!(0));
        }
    }
    if let Some(type_id) = &node.widget {
        let widget = widgets::catalog().by_type(type_id).ok_or_else(|| {
            AuthoringError::Invalid(format!(
                "template {template}: unknown widget type {type_id}"
            ))
        })?;
        let number = document.allocate();
        let mut value = mashup::edit::new_widget(widget, number, true);
        if let Some(gutter) = &node.widget_gutter {
            value["Properties"]["Margin"] = json!(gutter);
        }
        for (key, prop) in &node.widget_properties {
            value["Properties"][key] = prop.clone();
        }
        document.push_child(&child.path, value);
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
    if node.divider && node.thickness.is_none() {
        return Err(AuthoringError::Invalid(format!(
            "template {template}: a divider needs a thickness from the layout rules"
        )));
    }
    for (property, value) in &node.style {
        let css_name = !property.is_empty()
            && property
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-');
        if !css_name || !value.is_string() {
            return Err(AuthoringError::Invalid(format!(
                "template {template}: style {property} must be a CSS property with a text value"
            )));
        }
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
        "notes": plan.notes,
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
        "replacements": plan.copy_summary.as_ref().map(|summary| &summary.replacements),
        "menus": plan.menus.iter().map(|(service, property)| json!({
            "service": service,
            "property": property,
        })).collect::<Vec<_>>(),
    })
}

/// The concise text report for `mashup new --from`; JSON keeps the complete plan instead.
pub fn copy_summary_lines(solution: &Solution, plan: &NewPlan, applied: bool) -> Vec<String> {
    let source = plan
        .from
        .as_deref()
        .expect("only copied mashups have a copy summary");
    let summary = plan
        .copy_summary
        .as_ref()
        .expect("only copied mashups have a copy summary");
    let mut lines = vec![format!(
        "{} mashup {} from {source}",
        if applied { "created" } else { "would create" },
        plan.name
    )];
    lines.push(format!(
        "  widgets: {}; data calls: {}; bindings: {}; events: {}; parameters: {}",
        summary.widgets, summary.data_calls, summary.bindings, summary.events, summary.parameters
    ));
    lines.extend(
        summary
            .replacements
            .iter()
            .map(|replacement| format!("  {replacement}")),
    );
    lines.extend(
        [&plan.entity_path, &plan.content_path, &plan.css_path]
            .into_iter()
            .map(|path| format!("  {}", solution.relative(path))),
    );
    lines
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
        tabs = (1..=2).map(|n| (format!("Tab {n}"), None)).collect();
    }
    if tabs.len() > 16 {
        return Err(AuthoringError::Invalid(
            "a tab set has at most 16 tabs".into(),
        ));
    }
    let contained = widgets::catalog()
        .by_type("mashupcontainer")
        .expect("the catalogue has the contained mashup; a test checks it");
    let tabset_name = document.at(&tabset.path)["Properties"]["DisplayName"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for (index, (_, child)) in tabs.iter().enumerate() {
        let mut tab = tab_container(document, &tabset_name, index + 1);
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
            mashup::edit::insert_sorted(properties, "Margin", json!(""));
            mashup::edit::insert_sorted(properties, "Width", json!(400));
            mashup::edit::insert_sorted(properties, "Height", json!(300));
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

/// A tab set's NumberOfTabs, which Composer may store as a number or as text; 2 when unset.
pub(super) fn tab_count(value: &Value) -> usize {
    value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|n| n.fract() == 0.0)
                .and_then(|n| format!("{n:.0}").parse().ok())
        })
        .unwrap_or(2)
}

/// Give a tab set exactly `count` tabs, as the `tabs` template builds them: one flex container per
/// tab (`<tab set> #<n>`, `userCannotRemove`), `Tab<n>Name` and `Tab<n>Value` for new tabs, and
/// `Items` and `NumberOfTabs` to match. Existing tabs keep their containers and names; a trailing
/// tab whose container still holds widgets is not removed. Returns the tab containers added.
pub(super) fn sync_tabs(
    document: &mut MashupDocument,
    tabset_id: &str,
    count: usize,
) -> Result<Vec<String>, AuthoringError> {
    if !(1..=16).contains(&count) {
        return Err(AuthoringError::Invalid("a tab set has 1 to 16 tabs".into()));
    }
    let path = document
        .find_exact(tabset_id)
        .map_err(AuthoringError::Edit)?
        .path;
    let existing = document.at(&path)["Widgets"].as_array().map_or(0, Vec::len);
    for index in (count..existing).rev() {
        let child = [path.as_slice(), &[index]].concat();
        let holds = document.at(&child)["Widgets"]
            .as_array()
            .is_some_and(|widgets| !widgets.is_empty());
        if holds {
            let name = document.at(&child)["Properties"]["DisplayName"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            return Err(AuthoringError::Refused(format!(
                "tab {} ({name}) still holds widgets; remove them first",
                index + 1
            )));
        }
        document
            .remove_widget(&child)
            .map_err(AuthoringError::Edit)?;
    }
    let tabset_name = document.at(&path)["Properties"]["DisplayName"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut added = Vec::new();
    for index in existing..count {
        let tab = tab_container(document, &tabset_name, index + 1);
        added.push(
            tab["Properties"]["Id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        document.push_child(&path, tab);
    }
    let mut items = Vec::new();
    for n in 1..=count {
        let properties = &document.at(&path)["Properties"];
        let name = properties[format!("Tab{n}Name")]
            .as_str()
            .filter(|name| !name.is_empty() && *name != "[[TabName]]")
            .map_or_else(|| format!("Tab {n}"), str::to_string);
        let value = properties[format!("Tab{n}Value")]
            .as_str()
            .filter(|value| !value.is_empty())
            .map_or_else(|| format!("TabValue{n}"), str::to_string);
        document.set_property(&path, &format!("Tab{n}Name"), json!(name));
        document.set_property(&path, &format!("Tab{n}Value"), json!(value));
        items.push(json!({"disabled": false, "name": name, "value": value, "visible": true}));
    }
    // A removed tab leaves no Tab<n> properties behind for a later NumberOfTabs to reuse.
    if let Some(properties) = document.at_mut(&path)["Properties"].as_object_mut() {
        properties.retain(|key, _| {
            key.strip_prefix("Tab")
                .map(|rest| {
                    rest.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                })
                .and_then(|digits| digits.parse::<usize>().ok())
                .is_none_or(|tab| tab <= count)
        });
    }
    document.set_property(&path, "Items", Value::Array(items));
    document.set_property(&path, "NumberOfTabs", json!(count));
    Ok(added)
}

fn tab_container(document: &mut MashupDocument, tabset: &str, number: usize) -> Value {
    let container = widgets::catalog()
        .by_type("flexcontainer")
        .expect("the catalogue has the flex container");
    let mut tab = mashup::edit::new_widget(container, document.allocate(), true);
    tab["Properties"]["DisplayName"] = json!(format!("{tabset} #{number}"));
    tab["Properties"]["ExpandCollapseTabLocation"] = json!("inside");
    tab["Properties"]["Margin"] = json!("");
    mashup::edit::insert_sorted(
        mashup::edit::properties_mut(&mut tab),
        "userCannotRemove",
        json!(true),
    );
    tab
}

fn remove_content_css(assets: &mut mashup::Assets) {
    let mut content: Value = serde_json::from_str(&assets.content).expect("mashup content is JSON");
    content
        .as_object_mut()
        .expect("mashup content is an object")
        .remove("CustomMashupCss");
    assets.content = mashup::to_sidecar(&content);
}

fn short(text: &str) -> String {
    let prefix: String = text.chars().take(80).collect();
    if text.chars().count() > 80 {
        format!("{prefix}\u{2026}")
    } else {
        prefix
    }
}

fn css_difference(before: &str, after: &str) -> (String, String, String) {
    let left: Vec<_> = before.lines().collect();
    let right: Vec<_> = after.lines().collect();
    let at = (0..left.len().max(right.len()))
        .find(|&at| left.get(at) != right.get(at))
        .unwrap_or(0);
    (
        format!("custom.css line {}", at + 1),
        left.get(at).unwrap_or(&"<absent>").to_string(),
        right.get(at).unwrap_or(&"<absent>").to_string(),
    )
}

fn contains_token(value: &Value, old: &str) -> bool {
    match value {
        Value::String(_) => replace_tokens(&mut value.clone(), old, "") > 0,
        Value::Array(items) => items.iter().any(|item| contains_token(item, old)),
        Value::Object(map) => map.iter().any(|(key, value)| {
            contains_token(&Value::String(key.clone()), old) || contains_token(value, old)
        }),
        _ => false,
    }
}

// Only the payloads Composer treats as values are eligible. Identities and wiring remain intact.
fn replace_content_tokens(value: &mut Value, old: &str, new: &str) -> usize {
    fn widget(value: &mut Value, old: &str, new: &str) -> usize {
        let kind = value["Properties"]["Type"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let contract = crate::core::widgets::catalog().by_type(&kind);
        let mut count = 0;
        if let Some(properties) = value.get_mut("Properties").and_then(Value::as_object_mut) {
            for (key, value) in properties {
                let reference = matches!(
                    key.as_str(),
                    "Id" | "Type" | "Name" | "EntityName" | "DataName"
                ) || contract
                    .and_then(|widget| widget.property(key))
                    .and_then(|property| property.base_type.as_deref())
                    .is_some_and(|base| base.ends_with("NAME"));
                if !reference {
                    count += replace_tokens(value, old, new);
                }
            }
        }
        if let Some(children) = value.get_mut("Widgets").and_then(Value::as_array_mut) {
            for child in children {
                count += widget(child, old, new);
            }
        }
        count
    }
    let mut count = widget(&mut value["UI"], old, new);
    if let Some(sections) = value.get_mut("Data").and_then(Value::as_object_mut) {
        for section in sections.values_mut() {
            if let Some(services) = section.get_mut("Services").and_then(Value::as_array_mut) {
                for service in services {
                    if let Some(parameters) = service.get_mut("Parameters") {
                        count += replace_tokens(parameters, old, new);
                    }
                }
            }
        }
    }
    count
}

fn replace_tokens(value: &mut Value, old: &str, new: &str) -> usize {
    match value {
        Value::String(text) => {
            let token = |ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '-');
            let mut count = 0;
            let mut out = String::new();
            let mut end = 0;
            for (at, _) in text.match_indices(old) {
                let next = at + old.len();
                if text[..at].chars().next_back().is_some_and(token)
                    || text[next..].chars().next().is_some_and(token)
                {
                    continue;
                }
                out.push_str(&text[end..at]);
                out.push_str(new);
                end = next;
                count += 1;
            }
            out.push_str(&text[end..]);
            *text = out;
            count
        }
        Value::Array(items) => items.iter_mut().map(|v| replace_tokens(v, old, new)).sum(),
        Value::Object(map) => map.values_mut().map(|v| replace_tokens(v, old, new)).sum(),
        _ => 0,
    }
}
