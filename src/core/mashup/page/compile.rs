use super::{
    ids::Ids,
    schema::{self, Node, Page, Size},
    Error,
};
use crate::core::{
    config::Solution,
    mashup::{
        authoring::{self, session::Session},
        edit::{Placement, Side},
    },
    types,
};
use serde_json::{json, Value};
use serde_saphyr::Location;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Artifacts {
    pub(crate) entity: Vec<u8>,
    pub(crate) content: String,
    pub(crate) css: String,
    pub(crate) ids: BTreeMap<String, String>,
    /// Pages the spec generates for its collections, created beside it when absent.
    pub(crate) cells: Vec<GeneratedCell>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct GeneratedCell {
    pub(crate) name: String,
    /// The page spec that built it; written as the cell's own `page.yaml`.
    pub(crate) spec: String,
    pub(crate) artifacts: Artifacts,
}

pub(crate) fn compile(solution: &Solution, project: &str, text: &str) -> Result<Artifacts, Error> {
    super::deep(|| compile_nested(solution, project, text))
}

fn compile_nested(solution: &Solution, project: &str, text: &str) -> Result<Artifacts, Error> {
    let spec = schema::parse_solution(text, solution)?;
    let page = &spec.page.value;
    if solution.project(project).is_none() {
        return Err(Error::at(
            page.name.referenced,
            format!("unknown project {project}"),
        ));
    }
    let theme = page
        .theme
        .as_ref()
        .map_or("PTC Convergence Theme", |t| t.value.as_str());
    authoring::session::validate_theme(solution, theme).map_err(|e| {
        Error::at(
            page.theme
                .as_ref()
                .map_or(page.name.referenced, |t| t.referenced),
            e,
        )
    })?;
    let mut session = Session::new(&page.name.value, project, &page.template.value, theme)
        .map_err(|e| Error::at(page.template.referenced, e))?;
    for (name, base_type) in &page.parameters {
        session
            .add_parameter(&format!("{name}:{}:IN", base_type.value))
            .map_err(|e| Error::at(base_type.referenced, e))?;
    }
    let mut allocator = Ids::new(session.document.widgets().into_iter().map(|w| w.id));
    let mut ids = BTreeMap::new();
    let mut cells = vec![];
    if let Some(title) = &page.title {
        let parent = match session.document.find("title") {
            Ok(slot) => slot.id,
            Err(_) => {
                session
                    .document
                    .find("center")
                    .map_err(|e| Error::at(title.referenced, e))?
                    .id
            }
        };
        let old = session
            .document
            .widgets()
            .into_iter()
            .map(|w| w.id)
            .collect();
        let id = session
            .add(
                solution,
                &authoring::AddRequest {
                    mashup: page.name.value.clone(),
                    widget: "ptcslabel".into(),
                    inputs: vec![],
                    placement: Some(Placement::In(parent)),
                    label: Some(title.value.clone()),
                    properties: vec![("LabelType".into(), "header".into())],
                },
            )
            .map_err(|e| Error::at(title.referenced, e))?;
        stabilize(
            &mut session,
            &mut allocator,
            &old,
            &id,
            "$page",
            title.referenced,
        )?;
    }
    for (slot, nodes) in &page.slots {
        let parent = session
            .document
            .find(slot)
            .map_err(|e| Error::at(nodes.referenced, e))?
            .id;
        let mut previous = None;
        for node in &nodes.value {
            let added = add(
                solution,
                &mut session,
                &mut allocator,
                &mut ids,
                page,
                node,
                (&parent, previous.as_deref()),
            )?;
            if !is_function(node)? {
                previous = Some(added);
            }
        }
    }
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    for (alias, source) in &page.data {
        let source = &source.value;
        if alias.is_empty() || !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::at(
                source.service.referenced,
                "data name must contain letters, digits or _",
            ));
        }
        let (entity, service) =
            authoring::session::source_target(&model, platform.as_ref(), &source.service.value)
                .map_err(|e| Error::at(source.service.referenced, e))?;
        let refresh = source
            .on
            .as_ref()
            .filter(|on| on.value.starts_with("every "))
            .map(|on| {
                on.value
                    .strip_prefix("every ")
                    .and_then(|s| s.strip_suffix('s'))
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        Error::at(on.referenced, "refresh must be every Ns with N above zero")
                    })
            })
            .transpose()?;
        let params = source
            .params
            .iter()
            .filter(|(_, v)| binding(&v.value).is_none())
            .map(|(k, v)| format!("{k}={}", scalar(&v.value)))
            .collect();
        session
            .source(
                solution,
                &authoring::SourceAddRequest {
                    mashup: page.name.value.clone(),
                    entity,
                    service,
                    dynamic: false,
                    on_load: source.on.as_ref().is_some_and(|on| on.value == "load"),
                    alias: Some(alias.clone()),
                    entity_from: None,
                    refresh,
                    cache: false,
                    cache_keys: vec![],
                    cache_strategy: None,
                    max_result_sets: None,
                    param_values: params,
                },
            )
            .map_err(|e| Error::at(source.service.referenced, e))?;
    }
    wire_nodes(
        solution,
        project,
        &mut session,
        &model,
        platform.as_ref(),
        page,
        &mut allocator,
        &mut ids,
        &mut cells,
    )?;
    for (alias, source) in &page.data {
        let source = &source.value;
        for (param, value) in &source.params {
            if let Some(from) = binding(&value.value) {
                let from = address(page, &ids, from, value.referenced)?;
                let to = format!("{}.{param}", call(page, alias));
                session
                    .bind(solution, &model, platform.as_ref(), &from, &to)
                    .map_err(|e| Error::at(value.referenced, e))?;
            }
        }
        if let Some(on) = source
            .on
            .as_ref()
            .filter(|on| on.value != "load" && !on.value.starts_with("every "))
        {
            let from = address(page, &ids, &on.value, on.referenced)?;
            session
                .on(&model, platform.as_ref(), &from, &call(page, alias))
                .map_err(|e| Error::at(on.referenced, e))?;
        }
    }
    for event in &page.events {
        let on = &event.value.on;
        let invoke = &event.value.invoke;
        let from = address(page, &ids, &on.value, on.referenced)?;
        let to = address(page, &ids, &invoke.value, invoke.referenced)?;
        session
            .on(&model, platform.as_ref(), &from, &to)
            .map_err(|e| Error::at(on.referenced, e))?;
    }
    for nodes in page.slots.values() {
        for node in &nodes.value {
            apply_size(solution, &mut session, page, &ids, node)?;
        }
    }
    super::elements::finish_layout(&mut session, page, &ids)?;
    session
        .sync()
        .map_err(|e| Error::at(spec.page.referenced, e))?;
    Ok(Artifacts {
        entity: session.entity,
        content: session.document.to_content(),
        css: session.css,
        ids,
        cells,
    })
}

pub(super) fn scalar(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

pub(super) fn binding(value: &Value) -> Option<&str> {
    value
        .as_object()
        .filter(|m| m.len() == 1)?
        .get("bind")?
        .as_str()
}

fn is_function(node: &Node) -> Result<bool, Error> {
    if matches!(node, Node::Pagination(_)) {
        return Ok(true);
    }
    Ok(crate::core::widgets::catalog()
        .resolve(node.widget()?)
        .is_ok_and(|w| w.function))
}

fn add(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    ids: &mut BTreeMap<String, String>,
    page: &Page,
    node: &Node,
    position: (&str, Option<&str>),
) -> Result<String, Error> {
    let body = node.body();
    let loc = body.id.referenced;
    if matches!(node, Node::Pagination(_)) {
        return Ok(String::new());
    }
    let mut props: Vec<_> = body
        .props
        .iter()
        .filter(|(_, v)| binding(&v.value).is_none())
        .map(|(k, v)| (k.clone(), scalar(&v.value)))
        .collect();
    let op = match node {
        Node::PageHeader(_) => Some("page-header"),
        Node::Toolbar(_) => Some("toolbar"),
        Node::KpiRow(_) => Some("page-kpi-row"),
        Node::Card(_) | Node::TrendTile(_) => Some("page-card"),
        _ => None,
    };
    match node {
        Node::Row(_) => props.push(("flex-direction".into(), "row".into())),
        Node::Column(_) => props.push(("flex-direction".into(), "column".into())),
        Node::DateRange(_) => props.push(("DateRange".into(), "true".into())),
        Node::Tabs(_) => {
            props.push(("NumberOfTabs".into(), body.tabs.len().to_string()));
            for (i, tab) in body.tabs.iter().enumerate() {
                props.push((format!("Tab{}Name", i + 1), tab.label.value.clone()));
            }
        }
        _ => {}
    }
    if let Some(hint) = &body.hint {
        props.push(("HintText".into(), hint.clone()));
    }
    if matches!(node, Node::Heading(_)) {
        props.push(("LabelType".into(), "header".into()));
    }
    if matches!(node, Node::Text(_)) {
        props.push(("LabelType".into(), "body".into()));
    }
    let (parent, previous) = position;
    let placement = if let Some(anchor) = previous {
        let parent = session
            .document
            .find_exact(parent)
            .map_err(|e| Error::at(loc, e))?;
        if session.document.at(&parent.path)["Properties"]["flex-direction"] == "row" {
            Placement::Beside {
                anchor: anchor.into(),
                side: Side::Right,
            }
        } else {
            Placement::BelowInColumn(anchor.into())
        }
    } else {
        Placement::In(parent.into())
    };
    let old = session
        .document
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    let placed = session
        .add(
            solution,
            &authoring::AddRequest {
                mashup: page.name.value.clone(),
                widget: node.widget()?.into(),
                inputs: body.inputs.clone(),
                placement: if is_function(node)? {
                    None
                } else {
                    Some(placement)
                },
                label: body.text.clone(),
                properties: props,
            },
        )
        .map_err(|e| Error::at(loc, e))?;
    if let Some(op) = op {
        let context = crate::core::mashup::layout::context::Node::op(
            op,
            crate::core::mashup::layout::Element::Container,
        );
        let path = session
            .document
            .find_exact(&placed)
            .map_err(|e| Error::at(loc, e))?
            .path;
        for (key, value) in session
            .document
            .rules()
            .evaluate(&context.context())
            .patch()
        {
            session.document.set_property(&path, key, value);
        }
    }
    match node {
        Node::Toolbar(_) if body.band.unwrap_or(true) => session.recipe(
            solution,
            authoring::Recipe::ToolbarBand {
                container: placed.clone(),
            },
        ),
        Node::Card(_) if body.align.is_some() => session.recipe(
            solution,
            authoring::Recipe::Section {
                container: placed.clone(),
                title: body.title.clone().expect("validated title"),
                center: body.align.as_ref().is_some_and(|a| a.value == "center"),
            },
        ),
        Node::Card(_) | Node::TrendTile(_) => session.recipe(
            solution,
            authoring::Recipe::Card {
                container: placed.clone(),
                caption: body.title.clone().expect("validated title"),
                value: body
                    .value
                    .as_ref()
                    .filter(|_| matches!(node, Node::Card(_)))
                    .map(|v| {
                        if binding(&v.value).is_some() {
                            String::new()
                        } else {
                            scalar(&v.value)
                        }
                    }),
                status: body.status.as_ref().map(|v| {
                    if binding(&v.value).is_some() {
                        String::new()
                    } else {
                        scalar(&v.value)
                    }
                }),
                kind: body.chart_kind.clone().unwrap_or_else(|| "info".into()),
            },
        ),
        Node::FilterPanel(_) => session
            .template_children(&placed, "filter-panel")
            .and_then(|()| filter_chevron(solution, session, &placed, body.collapsible)),
        Node::SidePanel(_) => session.recipe(
            solution,
            authoring::Recipe::SidePanel {
                container: placed.clone(),
                title: body.title.clone().expect("validated title"),
                info: body.help.clone(),
                tab: body.tab.clone().unwrap_or_else(|| "Filters".into()),
            },
        ),
        _ => Ok(()),
    }
    .map_err(|e| Error::at(loc, e))?;
    if (matches!(node, Node::Card(_)) && body.align.is_none()) || matches!(node, Node::TrendTile(_))
    {
        // Kit card titles use the regular label size, not the recipe's sub-header.
        let path = session
            .document
            .find_exact(&placed)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let caption = session
            .document
            .widgets()
            .into_iter()
            .find(|w| w.path.starts_with(&path) && w.type_id == "ptcslabel")
            .map(|w| w.path);
        if let Some(caption) = caption {
            let context = crate::core::mashup::layout::context::Node::op(
                "page-card-caption",
                crate::core::mashup::layout::Element::Container,
            );
            for (key, value) in session
                .document
                .rules()
                .evaluate(&context.context())
                .patch()
            {
                session.document.set_property(&caption, key, value);
            }
        }
    }
    let id = stabilize(session, allocator, &old, &placed, &body.id.value, loc)?;
    ids.insert(body.id.value.clone(), id.clone());
    apply_styles(solution, session, page, &id, body)?;
    if matches!(node, Node::FilterPanel(_)) {
        let root = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let slot = |name: &str, session: &Session| {
            session
                .document
                .widgets()
                .into_iter()
                .find(|w| w.path.starts_with(&root) && w.display_name == name)
                .expect("template slot")
                .id
        };
        let filters = slot("filters", session);
        let footer = slot("filter-footer", session);
        let content = slot("content", session);
        add_children(
            solution,
            session,
            allocator,
            ids,
            page,
            &body.fields,
            &filters,
        )?;
        for child in &body.children {
            let target = if body
                .footer
                .iter()
                .any(|f| child.body().id.value == format!("{}--{}", body.id.value, f.value))
            {
                &footer
            } else {
                &content
            };
            add_children(
                solution,
                session,
                allocator,
                ids,
                page,
                std::slice::from_ref(child),
                target,
            )?;
        }
    } else if matches!(node, Node::SidePanel(_)) {
        // The recipe's one tab holds the panel's content.
        let root = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let tabset = session
            .document
            .widgets()
            .into_iter()
            .find(|w| w.path.starts_with(&root) && w.type_id == "ptcstabset")
            .expect("side panel tab strip");
        let content = session.document.at(&tabset.path)["Widgets"][0]["Properties"]["Id"]
            .as_str()
            .expect("side panel tab content")
            .to_string();
        add_children(
            solution,
            session,
            allocator,
            ids,
            page,
            &body.children,
            &content,
        )?;
    } else if matches!(node, Node::Card(_)) && body.align.is_some() {
        // The section recipe's last container is the body its content goes in.
        let root = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let content = session.document.at(&root)["Widgets"]
            .as_array()
            .and_then(|items| items.last())
            .and_then(|item| item["Properties"]["Id"].as_str())
            .expect("section body")
            .to_string();
        add_children(
            solution,
            session,
            allocator,
            ids,
            page,
            &body.children,
            &content,
        )?;
    } else if matches!(node, Node::Tabs(_)) {
        let path = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        for (i, tab) in body.tabs.iter().enumerate() {
            let tab_path = [path.as_slice(), &[i]].concat();
            let parent = session.document.at(&tab_path)["Properties"]["Id"]
                .as_str()
                .expect("tab id")
                .to_string();
            let old: BTreeSet<_> = session
                .document
                .widgets()
                .into_iter()
                .map(|w| w.id)
                .collect();
            let role = format!("{}--tab-{}", body.id.value, tab.label.value);
            if let Some(mashup) = &tab.mashup {
                let held = session
                    .contained(solution, &parent, &mashup.value)
                    .map_err(|e| Error::at(mashup.referenced, e))?;
                stabilize(session, allocator, &old, &held, &role, tab.label.referenced)?;
            } else {
                session
                    .template_children(&parent, "tab-content")
                    .map_err(|e| Error::at(tab.label.referenced, e))?;
                let body_slot = session
                    .document
                    .widgets()
                    .into_iter()
                    .find(|w| w.path.starts_with(&tab_path) && w.display_name == "body")
                    .expect("tab body")
                    .id;
                let stable = stabilize(
                    session,
                    allocator,
                    &old,
                    &body_slot,
                    &role,
                    tab.label.referenced,
                )?;
                // Inline specs supply their own toolbar; do not reserve a second band.
                let toolbar = session
                    .document
                    .widgets()
                    .into_iter()
                    .find(|w| w.path.starts_with(&tab_path) && w.display_name == "toolbar")
                    .expect("tab toolbar");
                let index = *toolbar.path.last().expect("toolbar parent");
                session.document.at_mut(&tab_path)["Widgets"]
                    .as_array_mut()
                    .expect("tab children")
                    .remove(index);
                add_children(
                    solution,
                    session,
                    allocator,
                    ids,
                    page,
                    tab.content.as_deref().unwrap_or_default(),
                    &stable,
                )?;
            }
        }
    } else {
        add_children(solution, session, allocator, ids, page, &body.children, &id)?;
    }
    Ok(id)
}

/// The filter column's chevron: `collapsible: true` is the collapsible recipe with the tab inside
/// the column's top-right edge, as the kits draw it; `false` removes the template's chevron.
fn filter_chevron(
    solution: &Solution,
    session: &mut Session,
    panel: &str,
    collapsible: Option<bool>,
) -> Result<(), authoring::AuthoringError> {
    let Some(collapsible) = collapsible else {
        return Ok(());
    };
    let root = session
        .document
        .find_exact(panel)
        .map_err(authoring::AuthoringError::Edit)?
        .path;
    let column = session
        .document
        .widgets()
        .into_iter()
        .find(|w| w.path.starts_with(&root) && w.display_name == "filter-column")
        .expect("filter-panel template column");
    if collapsible {
        session.recipe(
            solution,
            authoring::Recipe::Collapsible {
                container: column.id,
                toggle: None,
                add_toggle_in: None,
                tab: "inside".into(),
                collapsed: false,
            },
        )
    } else {
        for (key, value) in [
            ("EnableExpandCollapse", json!(false)),
            ("ShowExpandCollapseTab", json!(false)),
        ] {
            session.document.set_property(&column.path, key, value);
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn add_children(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    ids: &mut BTreeMap<String, String>,
    page: &Page,
    children: &[Node],
    parent: &str,
) -> Result<(), Error> {
    let mut previous = None;
    for child in children {
        let added = add(
            solution,
            session,
            allocator,
            ids,
            page,
            child,
            (parent, previous.as_deref()),
        )?;
        if !is_function(child)? {
            previous = Some(added);
        }
    }
    Ok(())
}

pub(super) fn descendants(node: &Node) -> impl Iterator<Item = &Node> {
    let b = node.body();
    b.children
        .iter()
        .chain(&b.fields)
        .chain(b.tabs.iter().flat_map(|t| t.content.iter().flatten()))
}

fn styles(body: &schema::Body) -> Vec<String> {
    let mut styles: Vec<_> = body.style.iter().map(|s| s.value.clone()).collect();
    if let Some(surface) = body.surface {
        styles.push(format!(
            "background={}",
            if surface { "#FFFFFF" } else { "transparent" }
        ));
    }
    styles
}

fn apply_styles(
    solution: &Solution,
    session: &mut Session,
    page: &Page,
    id: &str,
    body: &schema::Body,
) -> Result<(), Error> {
    let styles = styles(body);
    if !styles.is_empty() {
        session
            .set(
                solution,
                &authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: id.into(),
                    properties: vec![],
                    style_from: None,
                    styles,
                },
            )
            .map_err(|e| Error::at(body.id.referenced, e))?;
    }
    Ok(())
}

fn apply_size(
    solution: &Solution,
    session: &mut Session,
    page: &Page,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    for child in descendants(node) {
        apply_size(solution, session, page, ids, child)?;
    }
    if matches!(node, Node::Toolbar(_)) && body.band.unwrap_or(true) {
        session
            .recipe(
                solution,
                authoring::Recipe::ToolbarBand {
                    container: ids[&body.id.value].clone(),
                },
            )
            .map_err(|e| Error::at(body.id.referenced, e))?;
        apply_styles(solution, session, page, &ids[&body.id.value], body)?;
    }
    let id = ids[&body.id.value].clone();
    if matches!(node, Node::Spacer(_)) {
        session
            .set(
                solution,
                &authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: id.clone(),
                    properties: session
                        .document
                        .rules()
                        .evaluate(
                            &crate::core::mashup::layout::context::Node::op(
                                "page-spacer",
                                crate::core::mashup::layout::Element::Container,
                            )
                            .context(),
                        )
                        .patch()
                        .into_iter()
                        .map(|(k, v)| (k.into(), v))
                        .collect(),
                    style_from: None,
                    styles: vec![],
                },
            )
            .map_err(|e| Error::at(body.id.referenced, e))?;
    }
    let loc = body.id.referenced;
    let mut properties = vec![];
    if let Some(size) = &body.size {
        let widget = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?;
        let container = widget.type_id == "flexcontainer";
        let sizing_id = if container {
            id.clone()
        } else {
            let parent_path = &widget.path[..widget.path.len() - 1];
            session.document.at(parent_path)["Properties"]["Id"]
                .as_str()
                .expect("parent id")
                .to_string()
        };
        let mut sizing = vec![];
        match size.value {
            Size::Fixed(px) if px > 0 && container => {
                let parent_path = &widget.path[..widget.path.len() - 1];
                let dimension =
                    if session.document.at(parent_path)["Properties"]["flex-direction"] == "row" {
                        "width"
                    } else {
                        "height"
                    };
                sizing.extend([
                    ("flex-size".into(), json!("fixed")),
                    ("flex-grow".into(), json!(0)),
                    (format!("flex-min-{dimension}"), json!(format!("{px}px"))),
                    (format!("flex-max-{dimension}"), json!(format!("{px}px"))),
                ]);
            }
            Size::Fixed(px) if px > 0 => properties.push(("Height".into(), json!(px))),
            Size::Fixed(_) => {
                return Err(Error::at(size.referenced, "fixed size must be above zero"))
            }
            Size::Sm | Size::Md | Size::Lg | Size::Display => {}
            Size::Fit => sizing.push(("flex-grow".into(), json!(0))),
            Size::Fill => sizing.extend([
                ("flex-size".into(), json!("range")),
                ("flex-max-height".into(), json!("")),
                ("flex-grow".into(), json!(1)),
                ("flex-shrink".into(), json!(1)),
                ("flex-basis".into(), json!("0px")),
            ]),
        }
        if container && matches!(size.value, Size::Fill) {
            sizing.push(("flex-min-height".into(), json!("0px")));
        }
        if !sizing.is_empty() {
            session
                .set(
                    solution,
                    &authoring::SetRequest {
                        mashup: page.name.value.clone(),
                        widget: sizing_id,
                        properties: sizing,
                        style_from: None,
                        styles: vec![],
                    },
                )
                .map_err(|e| Error::at(size.referenced, e))?;
        }
    }
    if !properties.is_empty() {
        session
            .set(
                solution,
                &authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: id.clone(),
                    properties,
                    style_from: None,
                    styles: vec![],
                },
            )
            .map_err(|e| Error::at(loc, e))?;
    }
    Ok(())
}

fn stabilize(
    session: &mut Session,
    allocator: &mut Ids,
    old: &BTreeSet<String>,
    placed: &str,
    node: &str,
    loc: Location,
) -> Result<String, Error> {
    let mut mapping = BTreeMap::new();
    let new = session
        .document
        .widgets()
        .into_iter()
        .filter(|w| !old.contains(&w.id))
        .collect::<Vec<_>>();
    let main = new.iter().find(|w| w.id == placed).expect("placed widget");
    let id = allocator
        .widget(&main.type_id, node, "widget")
        .map_err(|e| Error::at(loc, e))?;
    mapping.insert(placed.to_string(), id.clone());
    // Wrapper roles name the enclosed widget, so unrelated siblings cannot move their ids.
    for widget in new.iter().rev().filter(|w| w.id != placed) {
        let child = session.document.at(&widget.path)["Widgets"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v["Properties"]["Id"].as_str())
            .unwrap_or(placed);
        let child = mapping.get(child).map_or(child, String::as_str);
        let role = if let Some((_, tab)) = widget.display_name.rsplit_once(" #") {
            let label = session.document.at(&main.path)["Properties"][format!("Tab{tab}Name")]
                .as_str()
                .unwrap_or(tab);
            format!("tab:{label}")
        } else {
            if crate::core::widgets::catalog()
                .by_type(&widget.type_id)
                .is_some_and(|w| w.function)
            {
                format!(
                    "function:{}",
                    session.document.at(&widget.path)["Properties"]["Expression"]
                        .as_str()
                        .unwrap_or(&widget.type_id)
                )
            } else if session.document.at(&widget.path)["twacoTemplateSlot"] == true
                && !widget.display_name.starts_with("divider-")
            {
                format!("slot:{}", widget.display_name)
            } else if widget.display_name.starts_with("divider-") {
                let parent = &widget.path[..widget.path.len() - 1];
                let next = widget.path.last().expect("divider index") + 1;
                let next_slot = session.document.at(parent)["Widgets"]
                    .as_array()
                    .and_then(|items| items.get(next))
                    .and_then(|v| v["Properties"]["DisplayName"].as_str())
                    .unwrap_or("end");
                let parent_slot = session.document.at(parent)["Properties"]["DisplayName"]
                    .as_str()
                    .filter(|_| session.document.at(parent)["twacoTemplateSlot"] == true)
                    .unwrap_or("root");
                format!("divider:{parent_slot}:before:{next_slot}")
            } else if !widget.path.starts_with(&main.path) {
                format!("wrapper:{child}")
            } else if widget.path.len() > main.path.len() + 1
                && session.document.at(&widget.path)["Widgets"]
                    .as_array()
                    .is_none_or(Vec::is_empty)
            {
                // Leaves nested in a recipe's own containers are told apart by position.
                format!(
                    "leaf:{}",
                    widget.path[main.path.len()..]
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(".")
                )
            } else if widget.type_id == "ptcslabel" && widget.path.len() == main.path.len() + 1 {
                format!(
                    "label:{}",
                    session.document.at(&widget.path)["Properties"]["LabelType"]
                        .as_str()
                        .unwrap_or("body")
                )
            } else {
                format!("wrapper:{child}")
            }
        };
        let stable = allocator
            .widget(&widget.type_id, node, &role)
            .map_err(|e| Error::at(loc, e))?;
        mapping.insert(widget.id.clone(), stable);
    }
    for widget in &new {
        let Some(stable) = mapping.get(&widget.id) else {
            continue;
        };
        let number = stable.rsplit('-').next().expect("numeric suffix");
        let old_number = widget.id.rsplit('-').next().expect("numeric suffix");
        let properties = session.document.at_mut(&widget.path)["Properties"]
            .as_object_mut()
            .expect("widget properties");
        properties.insert("Id".into(), json!(stable));
        if let Some(display) = properties.get("DisplayName").and_then(Value::as_str) {
            if let Some(prefix) = display.strip_suffix(&format!("-{old_number}")) {
                properties.insert("DisplayName".into(), json!(format!("{prefix}-{number}")));
            }
        }
    }
    fn remap(v: &mut Value, mapping: &BTreeMap<String, String>) {
        match v {
            Value::Object(m) => {
                for (key, value) in m.iter_mut() {
                    if matches!(
                        key.as_str(),
                        "SourceId"
                            | "TargetId"
                            | "EventTriggerId"
                            | "EventHandlerId"
                            | "twacoPaginationFor"
                    ) {
                        if let Some(id) = value.as_str().and_then(|s| mapping.get(s)) {
                            *value = json!(id);
                        }
                    } else {
                        remap(value, mapping);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    remap(item, mapping);
                }
            }
            _ => {}
        }
    }
    for value in session.document.root_mut().values_mut() {
        remap(value, &mapping);
    }
    fn binding_ids(items: &mut [Value], mapping: &BTreeMap<String, String>) {
        for item in items {
            let uses_generated = ["SourceId", "TargetId", "EventTriggerId", "EventHandlerId"]
                .iter()
                .any(|k| {
                    item[*k]
                        .as_str()
                        .is_some_and(|id| mapping.values().any(|stable| stable == id))
                });
            if uses_generated {
                let mut semantic = item.clone();
                semantic
                    .as_object_mut()
                    .expect("binding object")
                    .remove("Id");
                item["Id"] = json!(super::ids::uuid(&format!("remapped:{semantic}")));
            }
        }
    }
    binding_ids(session.document.bindings_mut(), &mapping);
    binding_ids(session.document.events_mut(), &mapping);
    if let Some(styles) = session
        .document
        .root_properties_mut()
        .get_mut("StyleProperties")
        .and_then(|v| v.get_mut("data"))
        .and_then(Value::as_object_mut)
    {
        let before = std::mem::take(styles);
        for (key, value) in before {
            styles.insert(mapping.get(&key).cloned().unwrap_or(key), value);
        }
    }
    session
        .document
        .root_properties_mut()
        .insert("id_index".into(), json!(0));
    session.document.reserve_ids();
    Ok(id)
}

pub(super) fn call(page: &Page, alias: &str) -> String {
    format!("data:{}[as {alias}]", page.data[alias].value.service.value)
}

fn wire_sparklines(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let b = node.body();
    if let Some(sparkline) = &b.sparkline {
        let loc = sparkline.referenced;
        let spark = &sparkline.value;
        let data = spark
            .data
            .as_deref()
            .or_else(|| {
                b.value
                    .as_ref()
                    .and_then(|v| binding(&v.value))
                    .and_then(|v| v.split_once('.').map(|v| v.0))
            })
            .filter(|data| page.data.contains_key(*data))
            .ok_or_else(|| Error::at(loc, "sparkline needs data naming a page data source"))?;
        for field in [&spark.x, &spark.field] {
            let base = session
                .source_type(
                    solution,
                    model,
                    platform,
                    &format!("{}.{field}", call(page, data)),
                )
                .map_err(|e| Error::at(loc, e))?;
            if field == &spark.field && !["NUMBER", "INTEGER", "LONG"].contains(&base.as_str()) {
                return Err(Error::at(
                    loc,
                    format!("sparkline field {field} must be numeric"),
                ));
            }
        }
        session
            .bind(
                solution,
                model,
                platform,
                &call(page, data),
                &format!("ui:{}.Data", ids[&format!("{}--sparkline", b.id.value)]),
            )
            .map_err(|e| Error::at(loc, e))?;
    }
    for child in descendants(node) {
        wire_sparklines(solution, session, model, platform, page, ids, child)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn wire_selections(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    allocator: &mut Ids,
    ids: &mut BTreeMap<String, String>,
) -> Result<(), Error> {
    fn collect(node: &Node, refs: &mut BTreeMap<String, Location>) {
        let b = node.body();
        for value in b
            .props
            .values()
            .chain(b.value.iter())
            .chain(b.status.iter())
        {
            if let Some(text) = binding(&value.value).filter(|s| s.contains(".selected.")) {
                refs.insert(text.to_string(), value.referenced);
            }
        }
        for child in descendants(node) {
            collect(child, refs);
        }
    }
    let mut refs = BTreeMap::new();
    for node in page.slots.values().flat_map(|s| &s.value) {
        collect(node, &mut refs);
    }
    for source in page.data.values() {
        for value in source.value.params.values() {
            if let Some(text) = binding(&value.value).filter(|s| s.contains(".selected.")) {
                refs.insert(text.to_string(), value.referenced);
            }
        }
    }
    for (text, loc) in refs {
        let (grid, field) = text.split_once(".selected.").expect("selection");
        let data = find_grid(page, grid)
            .ok_or_else(|| Error::at(loc, format!("{text} needs a grid with data")))?;
        if field.is_empty() || field.contains('.') {
            return Err(Error::at(loc, format!("{text} needs one field")));
        }
        let grid_body = find_grid_body(page, grid).expect("grid with data");
        let path = session
            .document
            .find_exact(&ids[grid])
            .map_err(|e| Error::at(loc, e))?
            .path;
        if !grid_body.props.contains_key("DefaultSelectedRows") {
            session.document.set_property(
                &path,
                "DefaultSelectedRows",
                json!(if grid_body.auto_select.unwrap_or(true) {
                    "1"
                } else {
                    ""
                }),
            );
        }
        let base = session
            .source_type(
                solution,
                model,
                platform,
                &format!("{}.{field}", call(page, data)),
            )
            .map_err(|e| Error::at(loc, e))?;
        let old = session
            .document
            .widgets()
            .into_iter()
            .map(|w| w.id)
            .collect();
        let field_key = serde_json::to_string(field).expect("field string");
        let expression =
            format!("var selected = rows && (rows.rows || rows); result = selected && selected.length ? selected[0][{field_key}] : undefined;");
        let placed = session
            .add(
                solution,
                &authoring::AddRequest {
                    mashup: page.name.value.clone(),
                    widget: "expression2".into(),
                    inputs: vec!["rows:INFOTABLE".into()],
                    placement: None,
                    label: None,
                    properties: vec![
                        ("Expression".into(), expression),
                        ("OutputBaseType".into(), base),
                        ("AutoEvaluate".into(), "true".into()),
                    ],
                },
            )
            .map_err(|e| Error::at(loc, e))?;
        let id = stabilize(
            session,
            allocator,
            &old,
            &placed,
            &format!("{grid}--selected-{field}"),
            loc,
        )?;
        session
            .bind(
                solution,
                model,
                platform,
                &format!("ui:{}.SelectedRows", ids[grid]),
                &format!("ui:{id}.rows"),
            )
            .map_err(|e| Error::at(loc, e))?;
        ids.insert(text, id);
    }
    Ok(())
}

pub(super) fn address(
    page: &Page,
    ids: &BTreeMap<String, String>,
    text: &str,
    loc: Location,
) -> Result<String, Error> {
    if text == "load" {
        return Ok("mashup:Loaded".into());
    }
    if let Some(name) = text
        .strip_prefix("param.")
        .filter(|name| page.parameters.contains_key(*name))
    {
        return Ok(format!("mashup:{name}"));
    }
    if page.data.contains_key(text) {
        return Ok(call(page, text));
    }
    if text.contains(".selected.") {
        return ids
            .get(text)
            .map(|id| format!("ui:{id}.Output"))
            .ok_or_else(|| Error::at(loc, format!("unknown selection reference {text}")));
    }
    if let Some((node, property)) = text.split_once('.') {
        if let Some(id) = ids.get(node) {
            return Ok(format!("ui:{id}.{property}"));
        }
        if page.data.contains_key(node) {
            return Ok(if property == "SelectedRows" {
                format!("{}[SelectedRows]", call(page, node))
            } else {
                format!("{}.{property}", call(page, node))
            });
        }
    }
    Err(Error::at(loc, format!("unknown reference {text}")))
}

fn find_grid_body<'a>(page: &'a Page, name: &str) -> Option<&'a schema::Body> {
    fn find<'a>(node: &'a Node, name: &str) -> Option<&'a schema::Body> {
        if matches!(node, Node::Grid(_)) && node.body().id.value == name {
            return Some(node.body());
        }
        descendants(node).find_map(|n| find(n, name))
    }
    page.slots
        .values()
        .flat_map(|s| &s.value)
        .find_map(|n| find(n, name))
}

/// A list node with data, by id.
pub(super) fn find_list<'a>(page: &'a Page, name: &str) -> Option<&'a schema::Body> {
    fn find<'a>(node: &'a Node, name: &str) -> Option<&'a schema::Body> {
        if matches!(node, Node::List(_)) && node.body().id.value == name {
            return Some(node.body());
        }
        descendants(node).find_map(|n| find(n, name))
    }
    page.slots
        .values()
        .flat_map(|s| &s.value)
        .find_map(|n| find(n, name))
        .filter(|b| b.data.is_some())
}

/// What a `data:` field names as a binding source: a declared call, or the selected items of
/// another list (`<list>.selected`).
pub(super) fn data_address(page: &Page, ids: &BTreeMap<String, String>, text: &str) -> String {
    match text.strip_suffix(".selected") {
        Some(list) => format!("ui:{}.SelectedItems", ids[list]),
        None => call(page, text),
    }
}

pub(super) fn find_grid<'a>(page: &'a Page, name: &str) -> Option<&'a str> {
    find_grid_body(page, name)?
        .data
        .as_ref()
        .map(|d| d.value.as_str())
}

fn wire(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    ids: &mut BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    if matches!(node, Node::Pagination(_)) {
        return Ok(());
    }
    let id = &ids[&body.id.value];
    if let Some(auto) = body.auto_select {
        if auto && !body.props.contains_key("RowSelection") {
            let path = session
                .document
                .find_exact(id)
                .map_err(|e| Error::at(body.id.referenced, e))?
                .path;
            session
                .document
                .set_property(&path, "RowSelection", json!("single"));
        }
        if !body.props.contains_key("DefaultSelectedRows") {
            let path = session
                .document
                .find_exact(id)
                .map_err(|e| Error::at(body.id.referenced, e))?
                .path;
            session.document.set_property(
                &path,
                "DefaultSelectedRows",
                json!(if auto { "1" } else { "" }),
            );
        }
    }
    super::elements::wire(solution, session, model, platform, page, ids, node)?;
    if let Some(data) = body.data.as_ref().filter(|_| matches!(node, Node::Grid(_))) {
        let from = call(page, &data.value);
        session
            .bind(solution, model, platform, &from, &format!("ui:{id}.Data"))
            .map_err(|e| Error::at(data.referenced, e))?;
        if !body.columns.is_empty() {
            let statuses: Vec<_> = body
                .columns
                .iter()
                .filter_map(|c| {
                    c.value
                        .status
                        .as_ref()
                        .map(|s| (c.value.field.clone(), s.clone()))
                })
                .collect();
            let statuses = if statuses.is_empty() {
                vec![None]
            } else {
                statuses.into_iter().map(Some).collect()
            };
            for status in statuses {
                session
                    .recipe(
                        solution,
                        authoring::Recipe::Columns {
                            auto_select_first: false,
                            widget: id.clone(),
                            data: None,
                            columns: body
                                .columns
                                .iter()
                                .map(|c| {
                                    format!(
                                        "{}:{}:{}:{}",
                                        c.value.field,
                                        c.value.title.as_deref().unwrap_or(&c.value.field),
                                        c.value.width.map_or(String::new(), |w| w.to_string()),
                                        c.value.format.as_deref().unwrap_or("")
                                    )
                                })
                                .collect(),
                            date_format: None,
                            editable: vec![],
                            column_editors: vec![],
                            preserve_whitespace: vec![],
                            status,
                        },
                    )
                    .map_err(|e| Error::at(body.columns[0].referenced, e))?;
            }
        }
    } else if !body.columns.is_empty() {
        return Err(Error::at(
            body.columns[0].referenced,
            "grid columns need data",
        ));
    }
    for child in descendants(node) {
        wire(solution, session, model, platform, page, ids, child)?;
    }
    Ok(())
}

/// `enabled_when: <grid>.selected`: the kit's expression from the grid's `RowSelected` into the
/// button's `Disabled`, through the enable-when recipe.
fn wire_enabled(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    let gate = body.enabled_when.as_ref().expect("gated button");
    let grid = gate
        .value
        .strip_suffix(".selected")
        .expect("validated gate");
    let old: BTreeSet<_> = session
        .document
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    session
        .recipe(
            solution,
            authoring::Recipe::EnableWhen {
                widget: ids[&body.id.value].clone(),
                condition: "selected".into(),
                inputs: vec![("selected".into(), format!("ui:{}.RowSelected", ids[grid]))],
            },
        )
        .map_err(|e| Error::at(gate.referenced, e))?;
    let placed = session
        .document
        .widgets()
        .into_iter()
        .find(|w| !old.contains(&w.id) && w.type_id == "expression2")
        .expect("enable-when adds its expression")
        .id;
    stabilize(
        session,
        allocator,
        &old,
        &placed,
        &format!("{}--enabled", body.id.value),
        gate.referenced,
    )?;
    Ok(())
}

/// A collection: its cell mashup and mapping through the `mashup collection add` code. With
/// `cell:` the cell is a small page of its own, compiled by this same compiler and written beside
/// this page; the builder checks ownership before refreshing files on disk.
#[allow(clippy::too_many_arguments)]
fn wire_collection(
    solution: &Solution,
    project: &str,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    ids: &BTreeMap<String, String>,
    cells: &mut Vec<GeneratedCell>,
    node: &Node,
) -> Result<(), Error> {
    use authoring::session::CellSource;
    let body = node.body();
    let loc = body.id.referenced;
    let data = &body.data.as_ref().expect("validated data").value;
    let source = call(page, data);
    let (cell, mappings, preset) = if let Some(existing) = &body.mashup {
        (
            CellSource::Existing(existing.value.clone()),
            body.map
                .iter()
                .map(|(field, parameter)| (field.clone(), parameter.value.clone()))
                .collect(),
            body.preset.as_ref().map(|p| p.value.clone()),
        )
    } else {
        let spec = body.cell.as_ref().expect("validated cell");
        let base = page
            .name
            .value
            .strip_suffix("_MU")
            .unwrap_or(&page.name.value);
        let name = format!("{base}Cell_MU");
        let mut fields = vec![];
        for (field, role) in [
            (spec.value.title.as_ref(), "title"),
            (spec.value.kpi_dial.as_ref(), "kpi_dial"),
        ] {
            let Some(field) = field else { continue };
            let base_type = session
                .source_type(solution, model, platform, &format!("{source}.{field}"))
                .map_err(|e| Error::at(spec.referenced, e))?;
            if role == "kpi_dial" && !["NUMBER", "INTEGER", "LONG"].contains(&base_type.as_str()) {
                return Err(Error::at(
                    spec.referenced,
                    format!("cell kpi_dial field {field} must be numeric"),
                ));
            }
            fields.push((role, field.clone(), base_type));
        }
        if fields.is_empty() {
            return Err(Error::at(spec.referenced, "cell needs title or kpi_dial"));
        }
        let mappings = fields
            .iter()
            .map(|(_, f, _)| (f.clone(), f.clone()))
            .collect();
        let text = cell_spec(page, &name, &fields);
        let built = compile_nested(solution, project, &text)?;
        let cell = CellSource::Generated {
            name: name.clone(),
            entity: built.entity.clone(),
            content: built.content.clone(),
            css: built.css.clone(),
        };
        if !cells.iter().any(|c| c.name == name) {
            cells.push(GeneratedCell {
                name,
                spec: text,
                artifacts: built,
            });
        }
        (cell, mappings, Some("cards".to_string()))
    };
    let cell_name = match &cell {
        CellSource::Existing(name) | CellSource::Generated { name, .. } => name.clone(),
    };
    let preset = match preset.as_deref() {
        None => None,
        Some("cards") => Some(authoring::CollectionPreset::Cards),
        Some(_) => return Err(Error::at(loc, "preset is cards")),
    };
    session
        .collection(
            solution,
            &authoring::CollectionAddRequest {
                mashup: page.name.value.clone(),
                cell: cell_name,
                data: source,
                mappings,
                uid: None,
                width: None,
                height: None,
                columns: body.per_row.as_ref().map(|n| n.value),
                across: body.across.as_ref().map(|n| n.value),
                preset,
                menu: None,
                placement: None,
            },
            &cell,
            &ids[&body.id.value],
        )
        .map_err(|e| Error::at(loc, e))
}

/// The page spec of a generated KPI cell: a title over a dial, each bound to a parameter.
// CompareCell has image, text-pair and status bands, but no kit has a title/dial-only cell.
// Keep the KPI template for those two roles; its growing value uses the kit's LastContainer.
fn cell_spec(page: &Page, name: &str, fields: &[(&str, String, String)]) -> String {
    let parameters: serde_json::Map<_, _> = fields
        .iter()
        .map(|(_, field, base)| (field.clone(), json!(base)))
        .collect();
    let mut slots = serde_json::Map::new();
    for (role, field, _) in fields {
        let (slot, widget) = if *role == "title" {
            (
                "title",
                json!({"label": {"id": "title", "props": {"LabelText": {"bind": format!("param.{field}")}}}}),
            )
        } else {
            (
                "value",
                json!({"kpi_dial": {"id": "dial", "value": {"bind": format!("param.{field}")}}}),
            )
        };
        slots.insert(slot.into(), json!([widget]));
    }
    let mut spec = json!({"version": 1, "rules": "v1", "page": {
        "name": name, "template": "kpi-tile", "parameters": parameters, "slots": slots}});
    if let Some(theme) = &page.theme {
        spec["page"]["theme"] = json!(theme.value);
    }
    serde_json::to_string_pretty(&spec).expect("spec json")
        + "
"
}

/// `empty: <text>`: the empty-state recipe. A list has no such property, so the recipe adds a
/// notice beside it, which takes stable ids like every other generated widget.
fn wire_empty(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    page: &Page,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    let loc = body.id.referenced;
    let text = body.empty.clone().expect("empty text");
    let data = body
        .data
        .as_ref()
        .map(|d| data_address(page, ids, &d.value));
    let old: BTreeSet<_> = session
        .document
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    session
        .recipe(
            solution,
            authoring::Recipe::EmptyState {
                widget: ids[&body.id.value].clone(),
                text,
                data,
            },
        )
        .map_err(|e| Error::at(loc, e))?;
    if matches!(node, Node::List(_)) {
        let placed = session
            .document
            .widgets()
            .into_iter()
            .find(|w| !old.contains(&w.id) && w.type_id == "ptcsbutton")
            .expect("empty-state adds its notice")
            .id;
        stabilize(
            session,
            allocator,
            &old,
            &placed,
            &format!("{}--empty", body.id.value),
            loc,
        )?;
    }
    Ok(())
}

fn wire_pagination(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    page: &Page,
    ids: &mut BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    let grid = body.grid.as_ref().expect("validated grid");
    let target = ids
        .get(&grid.value)
        .ok_or_else(|| Error::at(grid.referenced, "unknown pagination grid"))?;
    let data = find_grid(page, &grid.value)
        .ok_or_else(|| Error::at(grid.referenced, "pagination for must name a grid with data"))?;
    let old: BTreeSet<_> = session
        .document
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    session
        .recipe(
            solution,
            authoring::Recipe::Pagination {
                widget: target.clone(),
                data: call(page, data),
            },
        )
        .map_err(|e| Error::at(grid.referenced, e))?;
    let placed = session
        .document
        .widgets()
        .into_iter()
        .find(|w| !old.contains(&w.id) && w.type_id == "ptcspagination")
        .ok_or_else(|| Error::at(grid.referenced, "grid already has pagination"))?
        .id;
    let id = stabilize(
        session,
        allocator,
        &old,
        &placed,
        &body.id.value,
        body.id.referenced,
    )?;
    ids.insert(body.id.value.clone(), id);
    let id = &ids[&body.id.value];
    session
        .set(
            solution,
            &authoring::SetRequest {
                mashup: page.name.value.clone(),
                widget: id.clone(),
                properties: body
                    .props
                    .iter()
                    .filter(|(_, v)| binding(&v.value).is_none())
                    .map(|(k, v)| (k.clone(), v.value.clone()))
                    .collect(),
                style_from: None,
                styles: styles(body),
            },
        )
        .map_err(|e| Error::at(body.id.referenced, e))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn wire_values(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    let id = &ids[&body.id.value];
    super::elements::wire_values(solution, session, model, platform, page, ids, node)?;
    for (prop, value) in &body.props {
        if let Some(from) = binding(&value.value) {
            let from = address(page, ids, from, value.referenced)?;
            session
                .bind(solution, model, platform, &from, &format!("ui:{id}.{prop}"))
                .map_err(|e| Error::at(value.referenced, e))?;
        }
    }
    for child in descendants(node) {
        wire_values(solution, session, model, platform, page, ids, child)?;
    }
    Ok(())
}

fn validate_contracts(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
) -> Result<(), Error> {
    for contract in &page.contracts {
        if !page.data.contains_key(&contract.data) {
            return Err(Error::at(
                contract.location,
                format!("component contract: unknown data {}", contract.data),
            ));
        }
        for (field, expected) in &contract.fields {
            let address = format!("{}.{}", call(page, &contract.data), field);
            let actual = session
                .source_type(solution, model, platform, &address)
                .map_err(|e| {
                    Error::at(
                        contract.location,
                        format!("component contract {field}: {e}"),
                    )
                })?;
            if expected != "ANY" && &actual != expected {
                return Err(Error::at(
                    contract.location,
                    format!("component contract {field}: expected {expected}, got {actual}"),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn component_recipe(
    solution: &Solution,
    mashup: &str,
    container: &str,
    name: &str,
    settings: &[String],
    loaded: authoring::ExistingMashup,
) -> Result<authoring::RecipePlan, authoring::AuthoringError> {
    super::deep(|| component_recipe_nested(solution, mashup, container, name, settings, loaded))
}

fn component_recipe_nested(
    solution: &Solution,
    mashup: &str,
    container: &str,
    name: &str,
    settings: &[String],
    mut loaded: authoring::ExistingMashup,
) -> Result<authoring::RecipePlan, authoring::AuthoringError> {
    let invalid = |e: Error| authoring::AuthoringError::Invalid(e.to_string());
    let mut invocation = json!({"use":name});
    for setting in settings {
        let (key, value) = setting
            .split_once('=')
            .filter(|(k, _)| !k.is_empty())
            .ok_or_else(|| {
                authoring::AuthoringError::Invalid("component --set needs prop=value".into())
            })?;
        if invocation.get(key).is_some() {
            return Err(authoring::AuthoringError::Invalid(format!(
                "component repeats prop {key}"
            )));
        }
        invocation[key] = serde_json::from_str(value).unwrap_or_else(|_| json!(value));
    }
    let project = crate::core::entity::parse(&loaded.entity_before)
        .map_err(|e| authoring::AuthoringError::Invalid(format!("{e:?}")))?
        .project;
    loaded.deterministic = true;
    let document = authoring::parse_loaded(&loaded)?;
    let mut data = serde_json::Map::new();
    let mut ambiguous = BTreeSet::new();
    for section in crate::core::bindings::data_sections(document.value()).values() {
        for call in &section.services {
            let service = if section.collection == "Things" {
                format!("{}.{}", section.entity, call.target)
            } else {
                format!("{}/{}.{}", section.collection, section.entity, call.target)
            };
            if data
                .insert(call.instance.clone(), json!({"service":service}))
                .is_some()
            {
                ambiguous.insert(call.instance.clone());
            }
        }
    }
    let parameters: serde_json::Map<_, _> =
        crate::core::mashup::describe::mashup_parameters(&loaded.entity_before)
            .map_err(authoring::AuthoringError::Unreadable)?
            .into_iter()
            .map(|p| (p.name, json!(p.base_type)))
            .collect();
    let mut source = json!({"version":1,"rules":"v1","page":{"name":mashup,"template":"blank","parameters":parameters,"data":data,"slots":{"center":[{"component":invocation}]}}});
    if let Some(theme) = document
        .value()
        .pointer("/UI/Properties/StyleTheme")
        .and_then(Value::as_str)
    {
        source["page"]["theme"] = json!(theme);
    }
    let text = source.to_string();
    let spec = schema::parse_solution(&text, solution).map_err(invalid)?;
    let page = &spec.page.value;
    fn uses_alias(node: &Node, alias: &str) -> bool {
        let body = node.body();
        let names_alias = |text: &str| {
            text == alias || text.strip_prefix(alias).is_some_and(|s| s.starts_with('.'))
        };
        body.data.as_ref().is_some_and(|s| s.value == alias)
            || body.options.as_ref().is_some_and(|s| s.value == alias)
            || body
                .sparkline
                .as_ref()
                .and_then(|s| s.value.data.as_deref())
                .is_some_and(|s| s == alias)
            || body
                .props
                .values()
                .chain(body.value.iter())
                .chain(body.status.iter())
                .chain(body.delta.iter())
                .any(|v| binding(&v.value).is_some_and(names_alias))
            || descendants(node).any(|child| uses_alias(child, alias))
    }
    for alias in &ambiguous {
        if page.contracts.iter().any(|c| &c.data == alias)
            || page
                .slots
                .values()
                .flat_map(|s| &s.value)
                .any(|n| uses_alias(n, alias))
        {
            return Err(authoring::AuthoringError::Ambiguous(format!(
                "component: data alias {alias} names several calls; use unique aliases"
            )));
        }
    }

    let parent = document
        .find(container)
        .map_err(authoring::AuthoringError::Edit)?
        .id;
    let before: BTreeSet<_> = document.widgets().into_iter().map(|w| w.id).collect();
    let mut session = Session::existing(
        mashup,
        loaded.entity_before.clone(),
        document,
        loaded.assets.css.clone(),
    );
    let mut allocator = Ids::new(before.iter().cloned());
    let mut ids: BTreeMap<_, _> = before.iter().map(|id| (id.clone(), id.clone())).collect();
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    for node in &page.slots["center"].value {
        add(
            solution,
            &mut session,
            &mut allocator,
            &mut ids,
            page,
            node,
            (&parent, None),
        )
        .map_err(invalid)?;
    }
    let mut cells = Vec::new();
    wire_nodes(
        solution,
        &project,
        &mut session,
        &model,
        platform.as_ref(),
        page,
        &mut allocator,
        &mut ids,
        &mut cells,
    )
    .map_err(invalid)?;
    let generated = cells
        .iter()
        .map(|cell| super::build::generated_cell(solution, &project, cell))
        .collect::<Result<Vec<_>, _>>()?;
    let notes = cells
        .iter()
        .map(|cell| {
            format!(
                "{}: generated collection cell (XML, sidecars, spec and baseline)",
                cell.name
            )
        })
        .collect();
    for node in &page.slots["center"].value {
        apply_size(solution, &mut session, page, &ids, node).map_err(invalid)?;
    }
    super::elements::finish_layout(&mut session, page, &ids).map_err(invalid)?;
    let added = session
        .document
        .widgets()
        .into_iter()
        .filter(|w| !before.contains(&w.id))
        .map(|w| w.id)
        .collect();
    Ok(authoring::RecipePlan {
        edit: authoring::finish_edit_with_css(
            loaded,
            session.document,
            "mashup recipe",
            Some(session.css),
        )?,
        name: "component",
        generated,
        added,
        removed: vec![],
        wired: vec![],
        notes,
    })
}

#[allow(clippy::too_many_arguments)]
fn wire_nodes(
    solution: &Solution,
    project: &str,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    allocator: &mut Ids,
    ids: &mut BTreeMap<String, String>,
    cells: &mut Vec<GeneratedCell>,
) -> Result<(), Error> {
    validate_contracts(solution, session, model, platform, page)?;
    for nodes in page.slots.values() {
        for node in &nodes.value {
            wire(solution, session, model, platform, page, ids, node)?;
        }
    }
    fn collections<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if matches!(node, Node::Collection(_)) {
            out.push(node);
        }
        for child in descendants(node) {
            collections(child, out);
        }
    }
    let mut shown = vec![];
    for node in page.slots.values().flat_map(|s| &s.value) {
        collections(node, &mut shown);
    }
    for node in shown {
        wire_collection(
            solution, project, session, model, platform, page, ids, cells, node,
        )?;
    }
    fn pagers<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if matches!(node, Node::Pagination(_)) {
            out.push(node);
        }
        for child in descendants(node) {
            pagers(child, out);
        }
    }
    let mut pagination = vec![];
    for node in page.slots.values().flat_map(|s| &s.value) {
        pagers(node, &mut pagination);
    }
    for node in pagination {
        wire_pagination(solution, session, allocator, page, ids, node)?;
    }
    fn gated<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if matches!(node, Node::Button(_)) && node.body().enabled_when.is_some() {
            out.push(node);
        }
        for child in descendants(node) {
            gated(child, out);
        }
    }
    let mut gates = vec![];
    for node in page.slots.values().flat_map(|s| &s.value) {
        gated(node, &mut gates);
    }
    for node in gates {
        wire_enabled(solution, session, allocator, ids, node)?;
    }
    fn emptied<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if node.body().empty.is_some() {
            out.push(node);
        }
        for child in descendants(node) {
            emptied(child, out);
        }
    }
    let mut empties = vec![];
    for node in page.slots.values().flat_map(|s| &s.value) {
        emptied(node, &mut empties);
    }
    for node in empties {
        wire_empty(solution, session, allocator, page, ids, node)?;
    }
    for node in page.slots.values().flat_map(|s| &s.value) {
        wire_sparklines(solution, session, model, platform, page, ids, node)?;
    }
    wire_selections(solution, session, model, platform, page, allocator, ids)?;
    for node in page.slots.values().flat_map(|s| &s.value) {
        wire_values(solution, session, model, platform, page, ids, node)?;
    }
    Ok(())
}
