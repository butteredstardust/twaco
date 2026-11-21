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

fn relocate(node: &mut Node, at: serde_saphyr::Location) {
    let b = node.body_mut();
    b.id.referenced = at;
    for prop in b.props.values_mut() {
        prop.referenced = at;
        prop.defined = at;
    }
    for child in &mut b.children {
        relocate(child, at);
    }
}

fn child(parent: &Body, role: &str, kind: &str, props: serde_json::Value) -> Node {
    let mut node = generated(parent, role, kind, props);
    relocate(&mut node, parent.id.referenced);
    node
}

#[allow(clippy::cast_possible_truncation)]
fn number(value: f64) -> serde_json::Value {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn default_prop(body: &mut Body, key: &str, value: serde_json::Value) {
    let at = body.id.referenced;
    body.props
        .entry(key.to_string())
        .or_insert_with(|| serde_saphyr::Spanned::new(value, at, at));
}

/// A label body showing a literal, or bound to a source field.
fn text_or_bind(value: &serde_json::Value) -> serde_json::Value {
    match super::compile::binding(value) {
        Some(_) => json!({"props": {"LabelText": value}}),
        None => json!({"text": super::compile::scalar(value)}),
    }
}

fn merge(mut label: serde_json::Value, props: serde_json::Value) -> serde_json::Value {
    let object = label.as_object_mut().expect("label object");
    let merged = object
        .entry("props")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("props object");
    for (key, value) in props.as_object().expect("props") {
        merged.insert(key.clone(), value.clone());
    }
    label
}

/// Trend direction: glyph and colour (kit trend tiles show a grey triangle; ptcsicon has no
/// known styler, so a coloured label glyph carries the direction).
const DIRECTIONS: [(&str, &str, &str); 3] = [
    ("up", "\u{25B2}", "#3C9A5F"),
    ("down", "\u{25BC}", "#D64545"),
    ("flat", "\u{25AC}", "#8A949B"),
];
const DOTS: [(&str, &str); 4] = [
    ("ok", "#4CAF50"),
    ("warning", "#F2A93B"),
    ("error", "#E0575B"),
    ("info", "#8D96A0"),
];

/// A framed grey panel as in the kit: grey fill, hairline border.
const PANEL_STYLE: [&str; 4] = [
    "background=#F7F7F7",
    "border-style=solid",
    "border-width=1px",
    "border-color=#E4E7E9",
];
/// Kit KPI dial layouts: (kind, Layout, Direction, start and end angle).
type Dial = (&'static str, &'static str, &'static str, Option<(u32, u32)>);
const DIALS: [Dial; 4] = [
    ("dial", "dial", "clockwise", Some((45, 315))),
    ("half", "dial", "clockwise", Some((90, 270))),
    ("horizontal", "horizontal", "left to right", None),
    ("vertical", "vertical", "bottom to top", None),
];

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
                Node::TrendTile(b) => {
                    let loc = b.id.referenced;
                    let caption = b.title.take().or_else(|| b.text.take());
                    let (Some(caption), Some(value)) = (caption, b.value.take()) else {
                        return Err(Error::at(
                            loc,
                            "trend_tile needs label and value: {bind: source.field}",
                        ));
                    };
                    if super::compile::binding(&value.value).is_none() {
                        return Err(Error::at(
                            value.referenced,
                            "trend_tile value must be {bind: source.field}",
                        ));
                    }
                    b.title = Some(caption);
                    let mut labels = vec![(
                        "value",
                        merge(
                            text_or_bind(&value.value),
                            json!({"LabelType": "large-header", "Margin": "0 6 0 0"}),
                        ),
                    )];
                    if let Some(unit) = b.unit.take() {
                        labels.push((
                            "unit",
                            json!({"text": unit, "props": {"LabelType": "title", "Margin": "0 8 0 0"}}),
                        ));
                    }
                    let trend = b.trend.take();
                    let delta = b.delta.take();
                    if let Some(trend) = trend {
                        let Some((_, glyph, colour)) =
                            DIRECTIONS.iter().find(|(name, _, _)| *name == trend.value)
                        else {
                            return Err(Error::at(
                                trend.referenced,
                                "trend must be up, down or flat",
                            ));
                        };
                        let style = json!([format!("label:color={colour}")]);
                        labels.push((
                            "arrow",
                            json!({"text": glyph, "props": {"Margin": "0 4 0 0"}, "style": style}),
                        ));
                        if let Some(delta) = delta {
                            let mut label =
                                merge(text_or_bind(&delta.value), json!({"Margin": "0"}));
                            label["style"] = style;
                            labels.push(("delta", label));
                        }
                    } else if let Some(delta) = delta {
                        return Err(Error::at(delta.referenced, "delta needs trend"));
                    }
                    let kids: Vec<_> = labels
                        .into_iter()
                        .map(|(role, mut label)| {
                            label["id"] = json!(format!("{}--{role}", b.id.value));
                            json!({"label": label})
                        })
                        .collect();
                    b.children.push(child(
                        b,
                        "figure",
                        "widget",
                        json!({"type": "flexcontainer", "children": kids, "props": {
                            "flex-direction": "row", "align-items": "center",
                            "justify-content": "center", "flex-grow": 1, "flex-shrink": 0,
                            "flex-size": "range", "flex-min-height": "56px", "Margin": "8 0"}}),
                    ));
                    default_prop(b, "align-items", json!("stretch"));
                }
                Node::DetailPair(b) => {
                    let loc = b.id.referenced;
                    let (Some(caption), Some(value)) = (b.text.take(), b.value.take()) else {
                        return Err(Error::at(loc, "detail_pair needs label and value"));
                    };
                    let dot = match b.chart_kind.take() {
                        None => None,
                        Some(kind) => Some(
                            DOTS.iter()
                                .find(|(name, _)| *name == kind)
                                .map(|(_, colour)| *colour)
                                .ok_or_else(|| {
                                    Error::at(loc, "detail_pair kind is ok, warning, error or info")
                                })?,
                        ),
                    };
                    if let Some(colour) = dot {
                        let style = format!("label:color={colour}");
                        let label = json!({"text": "\u{25CF}", "props": {"Margin": "0 8 0 0"}, "style": [style]});
                        b.children.push(child(b, "dot", "label", label));
                    }
                    b.children.push(child(
                        b,
                        "caption",
                        "label",
                        json!({"text": caption, "props": {"Margin": "0", "HorizontalAlignment": "left"}}),
                    ));
                    b.children.push(child(b, "spacer", "spacer", json!({})));
                    let label = merge(
                        text_or_bind(&value.value),
                        json!({"Margin": "0", "HorizontalAlignment": "right"}),
                    );
                    b.children.push(child(b, "value", "label", label));
                    default_prop(b, "flex-direction", json!("row"));
                    default_prop(b, "align-items", json!("center"));
                    default_prop(b, "Margin", json!("4 0"));
                }
                Node::Panel(b) => {
                    // The title sits in the band; a bare label first in the panel would make
                    // placement wrap the whole panel body.
                    let title = b.title.take();
                    if title.is_some() || !b.toolbar.is_empty() {
                        let mut toolbar = generated(b, "toolbar", "toolbar", json!({}));
                        let mut items = Vec::new();
                        if let Some(title) = title {
                            items.push(generated(
                                b,
                                "title",
                                "label",
                                json!({"text": title, "props": {"LabelType": "label", "Margin": "0 12 0 8"}}),
                            ));
                        }
                        items.append(&mut b.toolbar);
                        toolbar.body_mut().children = items;
                        b.children.insert(0, toolbar);
                    }
                    let at = b.id.referenced;
                    let mut styles: Vec<_> = PANEL_STYLE
                        .iter()
                        .map(|s| serde_saphyr::Spanned::new(s.to_string(), at, at))
                        .collect();
                    styles.append(&mut b.style);
                    b.style = styles;
                    for (key, value) in [
                        ("flex-direction", json!("column")),
                        ("align-items", json!("stretch")),
                        ("flex-grow", json!(1)),
                        ("flex-shrink", json!(1)),
                        ("flex-basis", json!("0px")),
                        ("Margin", json!("8")),
                    ] {
                        default_prop(b, key, value);
                    }
                }
                Node::Slider(b) => {
                    let min = b.min.as_ref().map_or(0.0, |m| m.value);
                    let max = b.max.as_ref().map_or(100.0, |m| m.value);
                    if min >= max {
                        let at = b.max.as_ref().map_or(b.id.referenced, |m| m.referenced);
                        return Err(Error::at(at, "slider max must exceed min"));
                    }
                    default_prop(b, "Minimum", number(min));
                    default_prop(b, "Maximum", number(max));
                    default_prop(b, "DisplayMinMaxLabels", json!(b.labels.unwrap_or(true)));
                    let title = b.title.take().unwrap_or_default();
                    default_prop(b, "Label", json!(title));
                    if let Some(value) = b.value.take() {
                        default_prop(b, "Value", value.value);
                    }
                    default_prop(b, "Height", json!(110));
                }
                Node::KpiDial(b) => {
                    let kind = b.chart_kind.take().unwrap_or_else(|| "dial".into());
                    let Some((_, layout, direction, angles)) =
                        DIALS.iter().find(|(name, ..)| *name == kind)
                    else {
                        return Err(Error::at(
                            b.id.referenced,
                            "kpi_dial kind is dial, half, horizontal or vertical",
                        ));
                    };
                    default_prop(b, "Layout", json!(layout));
                    default_prop(b, "Direction", json!(direction));
                    if let Some((start, end)) = angles {
                        default_prop(b, "StartAngle", json!(start));
                        default_prop(b, "EndAngle", json!(end));
                    }
                    default_prop(b, "LabelAlignment", json!("center"));
                    let min = b.min.as_ref().map_or(0.0, |m| m.value);
                    let max = b.max.as_ref().map_or(100.0, |m| m.value);
                    default_prop(b, "MinimumValue", number(min));
                    default_prop(b, "MaximumValue", number(max));
                    if let Some(title) = b.title.take() {
                        default_prop(b, "Label", json!(title));
                    }
                    if let Some(unit) = b.unit.take() {
                        default_prop(b, "UnitOfMeasure", json!(unit));
                        default_prop(b, "ShowUOMLabel", json!(true));
                    }
                    if let Some(value) = b.value.take() {
                        default_prop(b, "Value", value.value);
                    }
                }
                Node::Card(b) => {
                    if b.children
                        .iter()
                        .any(|c| matches!(c, Node::DetailPair(_) | Node::KpiDial(_)))
                    {
                        default_prop(b, "align-items", json!("stretch"));
                    }
                }
                Node::Button(b) => {
                    if let Some(variant) = b.variant.take() {
                        if !["primary", "secondary", "tertiary", "transparent", "danger"]
                            .contains(&variant.value.as_str())
                        {
                            return Err(Error::at(
                                variant.referenced,
                                "variant is primary, secondary, tertiary, transparent or danger",
                            ));
                        }
                        default_prop(b, "ButtonType", json!(variant.value));
                    }
                    if let Some(icon) = b.icon.take() {
                        crate::core::cds_icons::check(&icon.value)
                            .map_err(|e| Error::at(icon.referenced, e))?;
                        default_prop(b, "SVGIcon", json!(icon.value));
                    }
                }
                Node::KpiRow(b) => {
                    if b.tiles.is_empty() && b.children.is_empty() {
                        return Err(Error::at(
                            b.id.referenced,
                            "kpi_row needs tiles or children",
                        ));
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
            matches!(
                node,
                Node::PageHeader(_)
                    | Node::Card(_)
                    | Node::Chart(_)
                    | Node::TrendTile(_)
                    | Node::Panel(_)
                    | Node::Slider(_)
                    | Node::KpiDial(_)
            ),
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
            matches!(
                node,
                Node::Chart(_) | Node::Card(_) | Node::DetailPair(_) | Node::KpiDial(_)
            ),
            "kind",
        ),
        (
            b.value.is_some(),
            matches!(
                node,
                Node::Chart(_)
                    | Node::Card(_)
                    | Node::TrendTile(_)
                    | Node::DetailPair(_)
                    | Node::Slider(_)
                    | Node::KpiDial(_)
            ),
            "value",
        ),
        (
            b.status.is_some() || b.color.is_some(),
            matches!(node, Node::Card(_)),
            "status/color",
        ),
        (
            b.unit.is_some(),
            matches!(node, Node::Card(_) | Node::TrendTile(_) | Node::KpiDial(_)),
            "unit",
        ),
        (
            b.trend.is_some() || b.delta.is_some(),
            matches!(node, Node::TrendTile(_)),
            "trend/delta",
        ),
        (
            b.variant.is_some() || b.icon.is_some() || b.enabled_when.is_some(),
            matches!(node, Node::Button(_)),
            "variant/icon/enabled_when",
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
            b.min.is_some() || b.max.is_some(),
            matches!(node, Node::Slider(_) | Node::KpiDial(_)),
            "min/max",
        ),
        (
            b.labels.is_some(),
            matches!(node, Node::Slider(_)),
            "labels",
        ),
        (
            !b.series.is_empty(),
            matches!(node, Node::Chart(_)),
            "series",
        ),
        (
            !b.toolbar.is_empty(),
            matches!(node, Node::Panel(_)),
            "toolbar",
        ),
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
        Node::Card(_) | Node::TrendTile(_) if b.title.is_none() => {
            return Err(fail("card needs title (tile needs label)"))
        }
        Node::Chart(_) if b.data.is_none() || b.x.is_none() => {
            return Err(fail("chart needs data and x"))
        }
        Node::Chart(_) if b.chart_kind.as_deref() == Some("combo") => {
            if b.series.is_empty() || b.series.len() > 8 {
                return Err(fail("combo chart needs series: 1 to 8 {field, kind}"));
            }
            for series in &b.series {
                if !["bar", "line"].contains(&series.value.kind.as_str()) {
                    return Err(Error::at(series.referenced, "series kind is bar or line"));
                }
            }
        }
        Node::Chart(_) if !b.series.is_empty() => {
            return Err(fail("series belongs to a combo chart"))
        }
        Node::Dropdown(_) if b.options.is_none() || b.display.is_none() => {
            return Err(fail("dropdown needs options and display"))
        }
        Node::Pagination(_) if b.grid.is_none() => {
            return Err(fail("pagination needs for: <grid>"))
        }
        Node::Button(_) if b.enabled_when.is_some() => {
            let gate = b.enabled_when.as_ref().expect("checked");
            let grid = gate.value.strip_suffix(".selected").unwrap_or_default();
            if super::compile::find_grid(page, grid).is_none() {
                return Err(Error::at(
                    gate.referenced,
                    "enabled_when must be <grid>.selected naming a grid with data",
                ));
            }
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
    if matches!(node, Node::Chart(_)) && b.chart_kind.as_deref() == Some("combo") {
        // The recipe names every numeric column; the spec picks the series and their drawing.
        let path = session
            .document
            .find_exact(id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let mut properties = vec![("NumberOfSeries".to_string(), json!(b.series.len()))];
        for (index, series) in b.series.iter().enumerate() {
            let n = index + 1;
            properties.push((format!("DataField{n}"), json!(series.value.field)));
            properties.push((format!("Series{n}Type"), json!(series.value.kind)));
        }
        for n in b.series.len() + 1..=8 {
            let key = format!("DataField{n}");
            let named = session.document.at(&path)["Properties"][key.as_str()]
                .as_str()
                .is_some_and(|field| !field.is_empty());
            if named {
                properties.push((key, json!("")));
            }
        }
        session
            .set(
                solution,
                &crate::core::mashup::authoring::SetRequest {
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
            let range = body
                .props
                .get("DateRange")
                .is_none_or(|v| v.value != json!(false) && v.value != json!("false"));
            let width = body
                .props
                .get("Width")
                .and_then(|v| v.value.as_u64())
                .unwrap_or(340);
            // The empty range hint "DD-MM-YY" truncates in two inputs narrower than ~300px.
            if range && width < 300 {
                patch(&mut session.document, &path, "page-date-range-narrow");
            }
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
        if matches!(node, Node::Slider(_)) && body.size.is_none() {
            // A row wrapper shrink-wraps the slider (min/max labels collide); a column wrapper
            // that stretches its child gives it the full row width at content height.
            patch(
                &mut session.document,
                &path[..path.len() - 1],
                "page-slider-wrapper",
            );
        }
        if matches!(node, Node::KpiRow(_) | Node::Card(_) | Node::TrendTile(_))
            && body.size.is_none()
        {
            let parent = &path[..path.len() - 1];
            let op = if session.document.at(parent)["Properties"]["flex-direction"] == "row" {
                "page-panel-width"
            } else {
                "page-content-height"
            };
            patch(&mut session.document, &path, op);
        }
        if matches!(node, Node::Widget(_) | Node::Panel(_))
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
            if matches!(node, Node::DetailPair(_)) {
                patch(&mut session.document, &path, "page-content-height");
            }
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
