use super::{
    schema::{Body, Node, Page},
    Error,
};
use serde_json::json;

fn generated(parent: &Body, role: &str, kind: &str, props: serde_json::Value) -> Node {
    let mut body = props;
    body["id"] = json!(format!("{}--{role}", parent.id.value));
    let mut node: Node =
        serde_saphyr::from_str(&json!({kind: body}).to_string()).expect("generated element schema");
    node.body_mut().id.referenced = parent.id.referenced;
    node
}

pub(super) fn expand(page: &mut Page) -> Result<(), Error> {
    fn nodes(list: &mut [Node]) -> Result<(), Error> {
        for node in list {
            fields(node)?;
            match node {
                Node::PageHeader(b) => {
                    let title = b
                        .title
                        .as_ref()
                        .ok_or_else(|| Error::at(b.id.referenced, "page_header needs title"))?;
                    b.children
                        .insert(0, generated(b, "title", "heading", json!({"text":title})));
                    if let Some(help) = &b.help {
                        b.children
                            .push(generated(b, "help", "label", json!({"text":help})));
                    }
                    if !b.actions.is_empty() {
                        b.children.push(generated(b, "spacer", "spacer", json!({})));
                        b.children
                            .extend(std::mem::take(&mut b.actions).into_iter().map(Node::Button));
                    }
                }
                Node::KpiRow(b) => {
                    if b.tiles.is_empty() {
                        return Err(Error::at(b.id.referenced, "kpi_row needs tiles"));
                    }
                    for mut tile in std::mem::take(&mut b.tiles) {
                        if tile
                            .value
                            .as_ref()
                            .and_then(|v| super::compile::binding(&v.value))
                            .is_none()
                            || tile.unit.is_none()
                        {
                            return Err(Error::at(
                                tile.id.referenced,
                                "KPI tile needs value: {bind: source.field} and unit",
                            ));
                        }
                        tile.title = tile.text.take();
                        if let Some(unit) = &tile.unit {
                            tile.children.push(generated(
                                &tile,
                                "unit",
                                "label",
                                json!({"text":unit}),
                            ));
                        }
                        b.children.push(Node::Card(tile));
                    }
                }
                Node::FilterPanel(b) => {
                    for action in &b.footer {
                        if !["apply", "reset"].contains(&action.value.as_str()) {
                            return Err(Error::at(
                                action.referenced,
                                "filter_panel footer must contain apply or reset",
                            ));
                        }
                        b.children.push(generated(
                            b,
                            &action.value,
                            "button",
                            json!({"label": if action.value == "apply" {"Apply"} else {"Reset"}}),
                        ));
                    }
                }
                _ => {}
            }
            let b = node.body_mut();
            nodes(&mut b.children)?;
            nodes(&mut b.fields)?;
            for tab in &mut b.tabs {
                if let Some(content) = &mut tab.content {
                    nodes(content)?;
                }
            }
        }
        Ok(())
    }
    for slot in page.slots.values_mut() {
        nodes(&mut slot.value)?;
    }
    Ok(())
}

fn fields(node: &Node) -> Result<(), Error> {
    let b = node.body();
    let checks = [
        (
            b.title.is_some(),
            matches!(node, Node::PageHeader(_) | Node::Card(_) | Node::Chart(_)),
            "title",
        ),
        (
            b.help.is_some() || !b.actions.is_empty(),
            matches!(node, Node::PageHeader(_)),
            "help/actions",
        ),
        (b.band.is_some(), matches!(node, Node::Toolbar(_)), "band"),
        (
            !b.tiles.is_empty(),
            matches!(node, Node::KpiRow(_)),
            "tiles",
        ),
        (
            !b.fields.is_empty() || !b.footer.is_empty(),
            matches!(node, Node::FilterPanel(_)),
            "fields/footer",
        ),
        (!b.tabs.is_empty(), matches!(node, Node::Tabs(_)), "tabs"),
        (
            b.chart_kind.is_some(),
            matches!(node, Node::Chart(_) | Node::Card(_)),
            "kind",
        ),
        (
            b.value.is_some(),
            matches!(node, Node::Chart(_) | Node::Card(_)),
            "value",
        ),
        (
            b.status.is_some() || b.color.is_some() || b.unit.is_some(),
            matches!(node, Node::Card(_)),
            "status/color/unit",
        ),
        (
            b.x.is_some() || b.stacked.is_some(),
            matches!(node, Node::Chart(_)),
            "x/stacked",
        ),
        (
            b.options.is_some() || b.display.is_some(),
            matches!(node, Node::Dropdown(_)),
            "options/display",
        ),
        (b.grid.is_some(), matches!(node, Node::Pagination(_)), "for"),
        (
            b.hint.is_some(),
            matches!(
                node,
                Node::TextField(_) | Node::Dropdown(_) | Node::DateRange(_)
            ),
            "hint",
        ),
    ];
    for (present, allowed, field) in checks {
        if present && !allowed {
            return Err(Error::at(
                b.id.referenced,
                format!("{field} does not belong to this element"),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate(node: &Node, page: &Page) -> Result<(), Error> {
    let b = node.body();
    let fail = |s| Error::at(b.id.referenced, s);
    match node {
        Node::Card(_) if b.title.is_none() => {
            return Err(fail("card needs title (tile needs label)"))
        }
        Node::Chart(_) if b.data.is_none() || b.x.is_none() => {
            return Err(fail("chart needs data and x"))
        }
        Node::Dropdown(_) if b.options.is_none() || b.display.is_none() => {
            return Err(fail("dropdown needs options and display"))
        }
        Node::Pagination(_) if b.grid.is_none() => {
            return Err(fail("pagination needs for: <grid>"))
        }
        Node::Tabs(_) => {
            if !(1..=16).contains(&b.tabs.len()) {
                return Err(fail("tabs needs 1 to 16 tabs"));
            }
            let mut labels = std::collections::BTreeSet::new();
            for tab in &b.tabs {
                if !labels.insert(&tab.label.value) {
                    return Err(Error::at(tab.label.referenced, "duplicate tab label"));
                }
                if tab.mashup.is_some() == tab.content.is_some() {
                    return Err(Error::at(
                        tab.label.referenced,
                        "tab needs exactly one of mashup or content",
                    ));
                }
            }
        }
        _ => {}
    }
    if let Some(options) = &b.options {
        if !page.data.contains_key(&options.value) {
            return Err(Error::at(
                options.referenced,
                format!("unknown data {}", options.value),
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn wire(
    solution: &crate::core::config::Solution,
    session: &mut crate::core::mashup::authoring::session::Session,
    _model: &crate::core::types::Model,
    _platform: Option<&crate::core::types::Platform>,
    page: &Page,
    ids: &std::collections::BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    use super::compile::{call, scalar};
    use crate::core::mashup::authoring::Recipe;
    let b = node.body();
    let id = &ids[&b.id.value];
    let loc = b.id.referenced;
    let recipe = match node {
        Node::Chart(_) => Some(Recipe::Chart {
            widget: id.clone(),
            data: call(page, &b.data.as_ref().expect("chart data").value),
            x: b.x.clone().expect("chart x"),
            value: b.value.as_ref().map(|v| scalar(&v.value)),
            kind: None,
            title: b.title.clone(),
            x_title: None,
            y_title: None,
            stacked: b.stacked.unwrap_or(false),
        }),
        Node::Dropdown(_) => Some(Recipe::Picker {
            widget: id.clone(),
            data: call(page, &b.options.as_ref().expect("options").value),
            display: b.display.clone().expect("display"),
            value: None,
            selected: None,
            no_none_row: false,
            auto_select_first: false,
        }),
        _ => None,
    };
    if let Some(recipe) = recipe {
        session
            .recipe(solution, recipe)
            .map_err(|e| Error::at(loc, e))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn wire_values(
    solution: &crate::core::config::Solution,
    session: &mut crate::core::mashup::authoring::session::Session,
    model: &crate::core::types::Model,
    platform: Option<&crate::core::types::Platform>,
    page: &Page,
    ids: &std::collections::BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    use super::compile::{address, binding};
    let b = node.body();
    let id = &ids[&b.id.value];
    let loc = b.id.referenced;
    if matches!(node, Node::Card(_)) {
        let root = session
            .document
            .find_exact(id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let labels: Vec<_> = session
            .document
            .widgets()
            .into_iter()
            .filter(|w| w.path.starts_with(&root) && w.type_id == "ptcslabel")
            .collect();
        for (value, index) in [
            (b.value.as_ref(), 1),
            (b.status.as_ref(), if b.value.is_some() { 2 } else { 1 }),
        ] {
            if let Some(v) = value {
                if let Some(from) = binding(&v.value) {
                    let from = address(page, ids, from, v.referenced)?;
                    session
                        .bind(
                            solution,
                            model,
                            platform,
                            &from,
                            &format!("ui:{}.LabelText", labels[index].id),
                        )
                        .map_err(|e| Error::at(v.referenced, e))?;
                }
            }
        }
        if let Some(color) = &b.color {
            if b.value.is_some() {
                session
                    .set(
                        solution,
                        &crate::core::mashup::authoring::SetRequest {
                            mashup: page.name.value.clone(),
                            widget: labels[1].id.clone(),
                            properties: vec![],
                            style_from: None,
                            styles: vec![format!("color={color}")],
                        },
                    )
                    .map_err(|e| Error::at(loc, e))?;
            }
        }
    }
    Ok(())
}

pub(super) fn finish_layout(
    session: &mut crate::core::mashup::authoring::session::Session,
    page: &Page,
    ids: &std::collections::BTreeMap<String, String>,
) -> Result<(), Error> {
    use crate::core::mashup::{
        edit::MashupDocument,
        layout::{self, context},
    };
    fn patch(document: &mut MashupDocument, path: &[usize], op: &'static str) {
        let node = context::Node::op(op, layout::Element::Container);
        for (key, value) in document.rules().evaluate(&node.context()).patch() {
            document.set_property(path, key, value);
        }
    }
    fn controls(
        document: &mut MashupDocument,
        path: &[usize],
        spacers: &std::collections::BTreeSet<String>,
    ) -> bool {
        let id = document.at(path)["Properties"]["Id"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if spacers.contains(&id) {
            patch(document, path, "page-spacer");
            return true;
        }
        let count = document.at(path)["Widgets"].as_array().map_or(0, Vec::len);
        let mut grows = false;
        for index in 0..count {
            let child = [path, &[index]].concat();
            grows |= controls(document, &child, spacers);
        }
        if document.at(path)["Properties"]["Type"] == "flexcontainer" {
            patch(document, path, "page-control-row");
            patch(document, path, "page-control-member");
            if grows {
                patch(document, path, "page-spacer");
            }
        }
        grows
    }
    fn height_floor(widget: &serde_json::Value) -> u32 {
        let props = &widget["Properties"];
        let minimum = props["flex-min-height"]
            .as_str()
            .and_then(|s| s.strip_suffix("px"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let heights = widget["Widgets"]
            .as_array()
            .into_iter()
            .flatten()
            .map(height_floor);
        let content = if props["flex-direction"] == "row" {
            heights.max().unwrap_or(0)
        } else {
            heights.sum()
        };
        minimum.max(content)
    }
    fn finish(
        session: &mut crate::core::mashup::authoring::session::Session,
        page: &Page,
        node: &Node,
        ids: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), Error> {
        let body = node.body();
        for child in super::compile::descendants(node) {
            finish(session, page, child, ids)?;
        }
        let path = session
            .document
            .find_exact(&ids[&body.id.value])
            .map_err(|e| Error::at(body.id.referenced, e))?
            .path;
        if matches!(node, Node::DateRange(_)) {
            patch(&mut session.document, &path, "page-date-range");
            if page.data.values().any(|source| {
                source.value.params.values().any(|value| {
                    super::compile::binding(&value.value).is_some_and(|from| {
                        ["StartDate", "EndDate"]
                            .iter()
                            .any(|property| from == format!("{}.{property}", body.id.value))
                    })
                })
            }) {
                session
                    .document
                    .set_property(&path, "InitializeWithCurrentDateTime", json!(false));
            }
        }
        if matches!(node, Node::KpiRow(_) | Node::Card(_)) && body.size.is_none() {
            let parent = &path[..path.len() - 1];
            let op = if session.document.at(parent)["Properties"]["flex-direction"] == "row" {
                "page-panel-width"
            } else {
                "page-content-height"
            };
            patch(&mut session.document, &path, op);
        }
        if matches!(node, Node::Widget(_))
            && session.document.at(&path)["Properties"]["Type"] == "flexcontainer"
            && !matches!(
                body.size.as_ref().map(|s| &s.value),
                Some(super::schema::Size::Fixed(_))
            )
            && session
                .document
                .widgets()
                .iter()
                .any(|w| w.path.starts_with(&path) && w.type_id.starts_with("ptcschart"))
        {
            let minimum = height_floor(session.document.at(&path));
            patch(&mut session.document, &path, "page-chart-region");
            session
                .document
                .set_property(&path, "flex-min-height", json!(format!("{minimum}px")));
        }
        if matches!(node, Node::Toolbar(_) | Node::PageHeader(_))
            || (session.document.at(&path)["Properties"]["flex-direction"] == "row"
                && body.children.iter().any(|n| matches!(n, Node::Spacer(_))))
        {
            let spacers = body
                .children
                .iter()
                .filter(|n| matches!(n, Node::Spacer(_)))
                .map(|n| ids[&n.body().id.value].clone())
                .collect();
            // Primitive placement adds responsive wrappers; only the spacer's branch may grow.
            let authored_size: Vec<_> = [
                "flex-size",
                "flex-min-height",
                "flex-max-height",
                "flex-grow",
                "flex-shrink",
                "flex-basis",
            ]
            .into_iter()
            .filter(|_| body.size.is_some())
            .map(|key| (key, session.document.at(&path)["Properties"][key].clone()))
            .collect();
            controls(&mut session.document, &path, &spacers);
            if matches!(node, Node::Toolbar(_)) && body.band.unwrap_or(true) {
                patch(&mut session.document, &path, "toolbar-band");
            } else if matches!(node, Node::PageHeader(_)) {
                patch(&mut session.document, &path, "page-header");
            }
            for (key, value) in authored_size {
                session.document.set_property(&path, key, value);
            }
        }
        for (key, value) in &body.props {
            if super::compile::binding(&value.value).is_none() {
                session
                    .document
                    .set_property(&path, key, value.value.clone());
            }
        }
        Ok(())
    }
    for node in page.slots.values().flat_map(|s| &s.value) {
        finish(session, page, node, ids)?;
    }
    Ok(())
}
