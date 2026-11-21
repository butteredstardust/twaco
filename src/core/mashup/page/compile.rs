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
}

pub(crate) fn compile(solution: &Solution, project: &str, text: &str) -> Result<Artifacts, Error> {
    let spec = schema::parse(text)?;
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
    let mut allocator = Ids::new(session.document.widgets().into_iter().map(|w| w.id));
    let mut ids = BTreeMap::new();
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
    for nodes in page.slots.values() {
        for node in &nodes.value {
            wire(
                solution,
                &mut session,
                &model,
                platform.as_ref(),
                page,
                &ids,
                node,
            )?;
        }
    }
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
    session
        .sync()
        .map_err(|e| Error::at(spec.page.referenced, e))?;
    Ok(Artifacts {
        entity: session.entity,
        content: session.document.to_content(),
        css: session.css,
        ids,
    })
}

fn scalar(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

fn binding(value: &Value) -> Option<&str> {
    value
        .as_object()
        .filter(|m| m.len() == 1)?
        .get("bind")?
        .as_str()
}

fn is_function(node: &Node) -> Result<bool, Error> {
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
    let mut props: Vec<_> = body
        .props
        .iter()
        .filter(|(_, v)| binding(&v.value).is_none())
        .map(|(k, v)| (k.clone(), scalar(&v.value)))
        .collect();
    if matches!(node, Node::Heading(_)) {
        props.push(("LabelType".into(), "header".into()));
    }
    let (parent, previous) = position;
    let placement = if let Some(anchor) = previous {
        let parent = session
            .document
            .find_exact(parent)
            .map_err(|e| Error::at(loc, e))?;
        Placement::Beside {
            anchor: anchor.into(),
            side: if session.document.at(&parent.path)["Properties"]["flex-direction"] == "row" {
                Side::Right
            } else {
                Side::Below
            },
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
                inputs: vec![],
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
    let id = stabilize(session, allocator, &old, &placed, &body.id.value, loc)?;
    ids.insert(body.id.value.clone(), id.clone());
    let mut styles: Vec<_> = body.style.iter().map(|s| s.value.clone()).collect();
    if let Some(surface) = body.surface {
        styles.push(format!(
            "background={}",
            if surface { "#FFFFFF" } else { "transparent" }
        ));
    }
    if !styles.is_empty() {
        session
            .set(
                solution,
                &authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: id.clone(),
                    properties: vec![],
                    style_from: None,
                    styles: std::mem::take(&mut styles),
                },
            )
            .map_err(|e| Error::at(loc, e))?;
    }
    let mut previous = None;
    for child in &body.children {
        let added = add(
            solution,
            session,
            allocator,
            ids,
            page,
            child,
            (&id, previous.as_deref()),
        )?;
        if !is_function(child)? {
            previous = Some(added);
        }
    }
    Ok(id)
}

fn apply_size(
    solution: &Solution,
    session: &mut Session,
    page: &Page,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    for child in &body.children {
        apply_size(solution, session, page, ids, child)?;
    }
    let id = ids[&body.id.value].clone();
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
            Size::Fit => sizing.push(("flex-grow".into(), json!(0))),
            Size::Fill => sizing.push(("flex-grow".into(), json!(1))),
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
            format!("tab:{tab}")
        } else {
            format!("wrapper:{child}")
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

fn call(page: &Page, alias: &str) -> String {
    format!("data:{}[as {alias}]", page.data[alias].value.service.value)
}

fn address(
    page: &Page,
    ids: &BTreeMap<String, String>,
    text: &str,
    loc: Location,
) -> Result<String, Error> {
    if text == "load" {
        return Ok("mashup:Loaded".into());
    }
    if page.data.contains_key(text) {
        return Ok(call(page, text));
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

fn wire(
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
    if let Some(data) = &body.data {
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
    for (prop, value) in &body.props {
        if let Some(from) = binding(&value.value) {
            let from = address(page, ids, from, value.referenced)?;
            session
                .bind(solution, model, platform, &from, &format!("ui:{id}.{prop}"))
                .map_err(|e| Error::at(value.referenced, e))?;
        }
    }
    for child in &body.children {
        wire(solution, session, model, platform, page, ids, child)?;
    }
    Ok(())
}
