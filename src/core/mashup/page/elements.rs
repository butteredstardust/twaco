use super::{
    schema::{Body, Node, Page, Size},
    Error,
};
use crate::core::mashup::tokens::Tokens;
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

/// The kit's information icon: a disabled transparent button whose tooltip is the text.
fn info_button(parent: &Body, role: &str, tooltip: &str, icon: &str) -> Node {
    child(
        parent,
        role,
        "button",
        json!({"text": "", "props": {
            "ButtonType": "transparent", "SVGIcon": icon, "Disabled": true,
            "TooltipField": tooltip, "Margin": "0 0 0 4"}, "style": [INFO_STYLE]}),
    )
}

/// A heading with an information icon after it: a row holding both.
fn info_heading(b: &mut Body, kind: &str, tooltip: &str) -> Node {
    let id = b.id.value.clone();
    let props: serde_json::Map<_, _> = b
        .props
        .iter()
        .map(|(key, value)| (key.clone(), value.value.clone()))
        .collect();
    let styles: Vec<_> = b.style.iter().map(|s| s.value.clone()).collect();
    let mut row: Node = serde_saphyr::from_str(
        &json!({"row": {"id": id, "props": {"align-items": "center"}, "children": [
            {kind: {"id": format!("{id}--title"), "text": b.text.take().unwrap_or_default(),
                "props": props, "style": styles}},
            {"button": {"id": format!("{id}--info"), "text": "", "props": {
                "ButtonType": "transparent", "SVGIcon": "cds:icon_info", "Disabled": true,
                "TooltipField": tooltip, "Margin": "0 0 0 4"}, "style": [INFO_STYLE]}}]}})
        .to_string(),
    )
    .expect("generated heading row schema");
    relocate(&mut row, b.id.referenced);
    row
}

/// A navigation column as the kits show their administration menu: an optional search field, then
/// each group's grey caption over its items. Items are full-width transparent buttons, the first
/// drawn as the selected page, so `<id>--item-<group>-<item>` (1-based) can be wired to events.
fn nav_column(b: &mut Body) -> Result<Node, Error> {
    let id = b.id.value.clone();
    if b.groups.is_empty() {
        return Err(Error::at(b.id.referenced, "nav_menu needs groups"));
    }
    // Administrator uses collection cells for navigation; no kit has a static button menu.
    // Keep its search/caption/item order in one column, with direct controls as in kit fields.
    default_prop(b, "CustomClass", json!("twaco-nav-menu"));
    let mut children = vec![];
    if let Some(search) = b.search.take() {
        children.push(json!({"text_field": {
            "id": format!("{id}--search"), "hint": search, "icon": "search",
            "props": {"Margin": "0 18 12"}}}));
    }
    let mut first = true;
    for (g, group) in b.groups.iter().enumerate() {
        children.push(json!({"label": {
            "id": format!("{id}--group-{}", g + 1), "text": group.value.title,
            "props": {"Margin": "12 0 6 18", "LabelType": "label"},
            "style": ["label:color=nav-caption", "label:font-size=14px"]}}));
        for (i, item) in group.value.items.iter().enumerate() {
            let mut body = json!({
                "id": format!("{id}--item-{}-{}", g + 1, i + 1), "text": item,
                "variant": "transparent",
                "props": {"LabelAlignment": "left", "Size": "standard", "Margin": "0"},
                "style": ["width=100%", "box-sizing=border-box", "border-radius=0px", "padding-left=24px",
                    "label:font-size=14px", "label:font-weight=600", "label:color=nav-text"]});
            if std::mem::take(&mut first) {
                body["style"]
                    .as_array_mut()
                    .expect("item styles")
                    .push(json!("background=nav-selected"));
            }
            children.push(json!({"button": body}));
        }
    }
    let props: serde_json::Map<_, _> = b
        .props
        .iter()
        .map(|(key, value)| (key.clone(), value.value.clone()))
        .collect();
    let mut column: Node = serde_saphyr::from_str(
        &json!({"column": {"id": id, "props": props, "children": children}}).to_string(),
    )
    .expect("generated nav column schema");
    relocate(&mut column, b.id.referenced);
    Ok(column)
}

fn spanned(text: String, at: serde_saphyr::Location) -> serde_saphyr::Spanned<String> {
    serde_saphyr::Spanned::new(text, at, at)
}

/// Change a node's element while keeping its body, as block becomes a card.
fn retag(node: &mut Node, make: fn(Body) -> Node) {
    let placeholder: Body = serde_saphyr::from_str("{id: placeholder}").expect("placeholder body");
    let body = std::mem::replace(node, Node::Spacer(placeholder)).into_body();
    *node = make(body);
}

fn colour_at(tokens: &Tokens, text: &str, at: serde_saphyr::Location) -> Result<String, Error> {
    tokens.colour(text).map_err(|e| Error::at(at, e))
}

/// The `--style` settings a heading, label or text size stands for: the label type (a prop), and
/// for `display` a larger face than any label type has, as the benchmark's hero title wants.
fn text_size(b: &mut Body) -> Result<(), Error> {
    let Some(size) = b.size.as_ref().map(|s| &s.value) else {
        return Ok(());
    };
    let (label_type, extra): (&str, &[&str]) = match size {
        Size::Sm => ("title", &[]),
        Size::Md => ("sub-header", &[]),
        Size::Lg => ("header", &[]),
        Size::Xl => ("large-header", &["label:font-size=48px"]),
        Size::Display => (
            "large-header",
            // No kit has a display face; the label type's largest size, enlarged by style.
            &[
                "label:font-size=72px",
                "label:font-weight=800",
                "label:line-height=1",
            ],
        ),
        _ => return Ok(()),
    };
    let at = b.id.referenced;
    b.size = None;
    default_prop(b, "LabelType", json!(label_type));
    for setting in extra {
        b.style.push(spanned((*setting).to_string(), at));
    }
    Ok(())
}

/// What `style: {background, color, border, font}` stands for on one kind of element.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StyleOn {
    Text,
    Box,
    Native,
}

/// Turn the `~key=value` entries of a style map into `--style` settings. Colours and fonts use
/// the names the command line takes too; `border` uses the token border width and a solid line in that colour.
fn style_map(b: &mut Body, on: StyleOn, tokens: &Tokens) -> Result<(), Error> {
    let at = b.id.referenced;
    let mut kept = vec![];
    for entry in std::mem::take(&mut b.style) {
        let Some((key, value)) = entry
            .value
            .strip_prefix('~')
            .and_then(|rest| rest.split_once('='))
        else {
            kept.push(entry);
            continue;
        };
        let settings: Vec<String> = match (key, on) {
            ("background", _) => vec![format!("background={}", colour_at(tokens, value, at)?)],
            ("border", _) => vec![
                "border-style=solid".into(),
                format!("border-width={}", tokens.border_width()),
                format!("border-color={}", colour_at(tokens, value, at)?),
            ],
            ("color", StyleOn::Native) => vec![format!("color={}", colour_at(tokens, value, at)?)],
            ("font", StyleOn::Native) => vec![format!("font-family={}", tokens.font(value))],
            ("color", StyleOn::Text) => vec![format!("label:color={}", colour_at(tokens, value, at)?)],
            ("font", StyleOn::Text) => vec![format!(
                "label:font-family={}",
                tokens.font(value)
            )],
            ("shadow", _) => vec![format!("box-shadow={}", tokens.style("box-shadow", value).map_err(|e| Error::at(at, e))?)],
            ("radius", _) => vec![format!("border-radius={}", tokens.style("border-radius", value).map_err(|e| Error::at(at, e))?)],
            ("color" | "font", StyleOn::Box) => {
                return Err(Error::at(
                    at,
                    format!("style {key} belongs to heading, label and text elements"),
                ))
            }
            _ => {
                return Err(Error::at(
                    at,
                    format!("style {key} is not supported; use background, color, border, font, shadow or radius"),
                ))
            }
        };
        kept.extend(settings.into_iter().map(|text| spanned(text, at)));
    }
    b.style = kept;
    Ok(())
}

/// An icon as the spec writes it: `search` or the full `cds:icon_search`.
pub(super) fn native_styles(b: &mut Body, tokens: &Tokens) -> Result<(), Error> {
    style_map(b, StyleOn::Native, tokens)
}

fn cds_name(icon: &str) -> String {
    if icon.starts_with("cds:") {
        icon.to_string()
    } else {
        format!("cds:icon_{icon}")
    }
}

/// The disabled-state colour the kit gives its information buttons.
const INFO_STYLE: &str = "$@disabled:color=#4E585A";

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
    "background=band",
    "border-style=solid",
    "border-width=${border.card}",
    "border-color=panel-line",
];
/// Kit KPI dial layouts: (kind, Layout, Direction, start and end angle).
type Dial = (&'static str, &'static str, &'static str, Option<(u32, u32)>);
const DIALS: [Dial; 4] = [
    ("dial", "dial", "clockwise", Some((45, 315))),
    ("half", "dial", "clockwise", Some((90, 270))),
    ("horizontal", "horizontal", "left to right", None),
    ("vertical", "vertical", "bottom to top", None),
];

pub(super) fn expand(page: &mut Page, tokens: &Tokens) -> Result<(), Error> {
    fn nodes(list: &mut [Node], tokens: &Tokens) -> Result<(), Error> {
        for node in list {
            super::native::expand(node, tokens)?;
            match node {
                Node::Heading(b) | Node::Label(b) | Node::Text(b) => {
                    text_size(b)?;
                    if let Some(highlight) = b.highlight.take() {
                        let hex = colour_at(tokens, &highlight.value, highlight.referenced)?;
                        let at = b.id.referenced;
                        b.style.push(spanned(format!("background={hex}"), at));
                    }
                    style_map(b, StyleOn::Text, tokens)?;
                }
                Node::Block(b) => {
                    for (key, value) in &tokens.block {
                        if !b
                            .style
                            .iter()
                            .any(|s| s.value.starts_with(&format!("~{key}=")))
                        {
                            b.style
                                .insert(0, spanned(format!("~{key}={value}"), b.id.referenced));
                        }
                    }
                    style_map(b, StyleOn::Box, tokens)?;
                    // Text sizes set the value's size; layout sizes (fit, fill, fixed) stay
                    // for the normal sizing pass.
                    let value_size = b.size.as_ref().and_then(|size| match size.value {
                        Size::Sm => Some("18px"),
                        Size::Md => Some("24px"),
                        Size::Lg => Some("36px"),
                        Size::Xl => Some("48px"),
                        Size::Display => Some("72px"),
                        _ => None,
                    });
                    if let Some(value_size) = value_size {
                        b.value_size = Some(value_size.to_string());
                        b.size = None;
                    }
                }
                Node::Chart(b) => style_map(b, StyleOn::Box, tokens)?,
                _ => {}
            }
            let informed = match node {
                Node::Heading(b) => b.info.take().map(|info| ("heading", info)),
                Node::Label(b) => b.info.take().map(|info| ("label", info)),
                _ => None,
            };
            let nav = match node {
                Node::NavMenu(b) => Some(nav_column(b)?),
                _ => None,
            };
            if let Some(column) = nav {
                *node = column;
            }
            if let Some((kind, info)) = informed {
                let replacement = info_heading(node.body_mut(), kind, &info);
                *node = replacement;
            }
            if let Node::Card(b) = node {
                if b.children
                    .iter()
                    .any(|n| matches!(n, Node::DetailPair(pair) if pair.chart_kind.is_some() || pair.state.is_some() || pair.status.is_some()))
                {
                    for pair in &mut b.children {
                        if let Node::DetailPair(pair) = pair {
                            if pair.chart_kind.is_none() && pair.state.is_none() && pair.status.is_none() {
                                pair.children.push(child(pair, "dot", "label", json!({"text": "", "props": {"Width": 16, "Margin": "0 8 0 0"}})));
                            }
                        }
                    }
                }
            }
            fields(node)?;
            match node {
                Node::Row(b) | Node::Column(b) => {
                    default_prop(b, "align-items", json!("stretch"));
                    default_prop(b, "flex-wrap", json!("nowrap"));
                }
                Node::PageHeader(b) => {
                    let title = b
                        .title
                        .as_ref()
                        .ok_or_else(|| Error::at(b.id.referenced, "page_header needs title"))?;
                    let mut heading = json!({"text":title});
                    if let Some(color) = b.color.take() {
                        heading["style"] = json!([format!(
                            "label:color={}",
                            colour_at(tokens, &color, b.id.referenced)?
                        )]);
                    }
                    b.children
                        .insert(0, generated(b, "title", "heading", heading));
                    if let Some(info) = b.info.take() {
                        let button = info_button(b, "info", &info, "cds:icon_help_circle");
                        b.children.push(button);
                    }
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
                    let caption = b
                        .title
                        .take()
                        .or_else(|| b.text.take().and_then(|v| v.as_str().map(str::to_owned)));
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
                    if let Some(sparkline) = &b.sparkline {
                        b.children.push(child(b, "sparkline", "widget", json!({
                            "type": "ptcschartline", "size": {"fixed": 40},
                            "props": crate::core::mashup::authoring::sparkline_props(&sparkline.value.x, &sparkline.value.field),
                            "style": crate::core::mashup::authoring::sparkline_styles()
                        })));
                        // Keep the value's source for wiring the sparkline's service rows.
                        b.value = Some(value);
                    }
                    default_prop(b, "align-items", json!("stretch"));
                }
                Node::DetailPair(b) => {
                    let loc = b.id.referenced;
                    let (Some(caption), Some(value)) = (b.text.take(), b.value.take()) else {
                        return Err(Error::at(loc, "detail_pair needs label and value"));
                    };
                    if b.state.is_some() && (b.chart_kind.is_some() || b.status.is_some()) {
                        return Err(Error::at(
                            loc,
                            "detail_pair needs either state or fixed status/kind",
                        ));
                    }
                    if let Some(status) = b.status.take() {
                        b.chart_kind = Some(
                            status
                                .value
                                .as_str()
                                .ok_or_else(|| {
                                    Error::at(
                                        status.referenced,
                                        "status is ok, warning, error or info",
                                    )
                                })?
                                .to_string(),
                        );
                    }
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
                    if b.state.is_some() {
                        let Some(bound) = super::compile::binding(&value.value) else {
                            return Err(Error::at(
                                value.referenced,
                                "state needs a bound detail_pair value",
                            ));
                        };
                        // The formatter reads the bound source field by name.
                        b.state_field = Some(bound.rsplit('.').next().unwrap_or(bound).to_string());
                        b.children.push(child(b, "dot", "widget", json!({"type": "shape", "props": {
                            "ShapeType": "circle", "Width": 12, "Height": 12, "Margin": "0 12 0 0",
                            "Data": value.value
                        }})));
                    }
                    if let Some(colour) = dot {
                        let style = format!("label:color={colour}");
                        let label = json!({"text": "\u{25CF}", "props": {"Width": 16, "Margin": "0 8 0 0"}, "style": [style]});
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
                    if let Some(align) = &b.align {
                        if !["start", "center"].contains(&align.value.as_str()) {
                            return Err(Error::at(align.referenced, "align is start or center"));
                        }
                        if b.value.is_some()
                            || b.status.is_some()
                            || b.unit.is_some()
                            || b.chart_kind.is_some()
                            || b.color.is_some()
                        {
                            return Err(Error::at(
                                align.referenced,
                                "a card with align holds children; value, status, unit, kind and color belong to a card without align",
                            ));
                        }
                    }
                    if b.children
                        .iter()
                        .any(|c| matches!(c, Node::DetailPair(_) | Node::KpiDial(_)))
                    {
                        default_prop(b, "align-items", json!("stretch"));
                    }
                }
                Node::Block(b) => {
                    let at = b.id.referenced;
                    let (Some(caption), Some(color)) = (b.caption.take(), b.color.take()) else {
                        return Err(Error::at(at, "block needs color, caption and value"));
                    };
                    if b.value.is_none() {
                        return Err(Error::at(at, "block needs color, caption and value"));
                    }
                    let hex = colour_at(tokens, &color, at)?;
                    b.title = Some(caption);
                    b.style.push(spanned(format!("background={hex}"), at));
                    retag(node, Node::Card);
                }
                Node::CardGrid(b) => {
                    let at = b.id.referenced;
                    let per_row = b
                        .per_row
                        .as_ref()
                        .ok_or_else(|| Error::at(at, "card_grid needs per_row"))?;
                    if !(1..=12).contains(&per_row.value) {
                        return Err(Error::at(per_row.referenced, "per_row is 1 to 12"));
                    }
                    let per_row = per_row.value as usize;
                    if b.children.is_empty() {
                        return Err(Error::at(at, "card_grid needs children"));
                    }
                    for cell in &mut b.children {
                        if !matches!(
                            cell,
                            Node::Card(_)
                                | Node::Block(_)
                                | Node::Column(_)
                                | Node::Row(_)
                                | Node::Panel(_)
                                | Node::TrendTile(_)
                        ) {
                            return Err(Error::at(
                                cell.body().id.referenced,
                                "card_grid cells are card, block, row, column, panel or trend_tile",
                            ));
                        }
                        let cell = cell.body_mut();
                        cell.grid_cell = true;
                        for (key, value) in crate::core::mashup::authoring::grid_cell_props() {
                            if ["flex-grow", "flex-shrink", "flex-basis", "Margin"].contains(&key) {
                                default_prop(cell, key, value);
                            }
                        }
                    }
                    let mut cells = std::mem::take(&mut b.children);
                    let mut rows = vec![];
                    while !cells.is_empty() {
                        let rest = cells.split_off(per_row.min(cells.len()));
                        let mut row = child(
                            b,
                            &format!("row-{}", rows.len() + 1),
                            "row",
                            json!({"props": crate::core::mashup::authoring::grid_row_props()
                                .into_iter().collect::<std::collections::BTreeMap<_, _>>()}),
                        );
                        row.body_mut().children = std::mem::take(&mut cells);
                        // The last row is padded with empty cells so every card keeps its width.
                        for pad in row.body().children.len()..per_row {
                            row.body_mut().children.push(child(
                                b,
                                &format!("pad-{}-{}", rows.len() + 1, pad + 1),
                                "column",
                                json!({"props": crate::core::mashup::authoring::grid_cell_props()
                                    .into_iter().collect::<std::collections::BTreeMap<_, _>>()}),
                            ));
                        }
                        rows.push(row);
                        cells = rest;
                    }
                    b.children = rows;
                    default_prop(b, "align-items", json!("stretch"));
                    default_prop(b, "flex-wrap", json!("nowrap"));
                    b.per_row = None;
                    retag(node, Node::Column);
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
                        let name = cds_name(&icon.value);
                        crate::core::cds_icons::check(&name)
                            .map_err(|e| Error::at(icon.referenced, e))?;
                        default_prop(b, "SVGIcon", json!(name));
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
                        tile.title = tile.text.take().and_then(|v| v.as_str().map(str::to_owned));
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
                Node::TextField(b) => {
                    if let Some(variant) = b.variant.take() {
                        let styles = crate::core::mashup::authoring::input_variant_styles(
                            &variant.value,
                        )
                        .ok_or_else(|| {
                            Error::at(
                                variant.referenced,
                                "variant is rounded, focus-background, focus-glow or focus-underline",
                            )
                        })?;
                        let at = b.id.referenced;
                        b.style
                            .extend(styles.into_iter().map(|text| spanned(text, at)));
                    }
                    if let Some(icon) = b.icon.take() {
                        let name = cds_name(&icon.value);
                        crate::core::cds_icons::check(&name)
                            .map_err(|e| Error::at(icon.referenced, e))?;
                        default_prop(b, "Icon", json!(name));
                    }
                }
                Node::SidePanel(b) => {
                    if b.title.is_none() {
                        return Err(Error::at(b.id.referenced, "side_panel needs title"));
                    }
                    default_prop(b, "flex-direction", json!("column"));
                }
                Node::List(b) => {
                    // The kit's lists carry no caption of their own.
                    default_prop(b, "Label", json!(""));
                }
                Node::FilterPanel(b) => {
                    if let Some(title) = b.title.take() {
                        b.fields.insert(
                            0,
                            generated(
                                b,
                                "title",
                                "heading",
                                json!({"text": title, "props": {
                                    "LabelType": "title", "HorizontalAlignment": "left"}}),
                            ),
                        );
                    }
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
            if matches!(node, Node::Heading(_) | Node::Label(_) | Node::Text(_))
                && node.body().band == Some(true)
            {
                let b = node.body_mut();
                b.band = None;
                let mut row = child(
                    b,
                    "band",
                    "row",
                    json!({"props": {
                        "flex-size": "range", "flex-min-height": "48px", "flex-max-height": "",
                        "flex-grow": 0, "flex-shrink": 0, "flex-basis": "auto",
                        "align-items": "stretch", "flex-wrap": "nowrap"
                    }}),
                );
                row.body_mut().band_row = true;
                row.body_mut().surface = b.surface.take();
                let mut text_styles = vec![];
                for style in std::mem::take(&mut b.style) {
                    let target = style.value.split_once('=').map_or("", |(target, _)| target);
                    if target.starts_with("label:")
                        || target.starts_with("label@")
                        || [
                            "color",
                            "font-family",
                            "font-size",
                            "font-weight",
                            "line-height",
                            "letter-spacing",
                            "text-align",
                        ]
                        .contains(&target)
                    {
                        text_styles.push(style);
                    } else {
                        row.body_mut().style.push(style);
                    }
                }
                b.style = text_styles;
                b.style.push(spanned("width=100%".into(), b.id.referenced));
                b.style.push(spanned("flex-grow=1".into(), b.id.referenced));
                let text = std::mem::replace(node, row);
                node.body_mut().children.push(text);
            }
            let b = node.body_mut();
            for style in &mut b.style {
                if let Some((target, value)) = style.value.split_once('=') {
                    let property = target.rsplit(':').next().unwrap();
                    style.value = format!(
                        "{target}={}",
                        tokens
                            .style(property, value)
                            .map_err(|e| Error::at(style.referenced, e))?
                    );
                }
            }
            nodes(&mut b.children, tokens)?;
            nodes(&mut b.fields, tokens)?;
            for tab in &mut b.tabs {
                if let Some(content) = &mut tab.content {
                    nodes(content, tokens)?;
                }
            }
        }
        Ok(())
    }
    for slot in page.slots.values_mut() {
        nodes(&mut slot.value, tokens)?;
    }
    Ok(())
}

fn fields(node: &Node) -> Result<(), Error> {
    let b = node.body();
    if let Some(weight) = &b.weight {
        if !weight.value.is_finite() || weight.value <= 0.0 {
            return Err(Error::at(weight.referenced, "weight must be above zero"));
        }
    }
    let checks = [
        (b.data_type.is_some() || b.input_count.is_some() || !b.routes.is_empty(), false, "data_type/input_count/routes belongs to events_router"),
        (b.unbound.is_some(), matches!(node, Node::Dropdown(_)), "unbound"),
        (b.mode.is_some() || b.total.is_some() || b.page_size.is_some() || b.page_number.is_some(), false, "mode/total/page_size/page_number belongs to server pagination"),
        (b.url.is_some() || b.target.is_some(), false, "url/target belongs to link"),
        (b.source.is_some() || b.alt.is_some(), false, "source belongs to image/icon/native collection; alt belongs to image/icon"),
        (b.scaling.is_some() || b.position.is_some(), false, "scaling/position belongs to image"),
        (b.orientation.is_some(), false, "orientation belongs to divider"),
        (b.icon_size.is_some(), false, "icon_size belongs to icon"),
        (b.group.is_some(), false, "group belongs to radio"),
        (b.date_only.is_some() || b.format.is_some(), false, "date_only/format belongs to date_picker"),
        (b.max_length.is_some(), false, "max_length belongs to textarea"),
        (!b.params.is_empty(), false, "params belongs to embed"),
        (
            b.sparkline.is_some(),
            matches!(node, Node::TrendTile(_)),
            "sparkline",
        ),
        (
            b.state.is_some(),
            matches!(node, Node::DetailPair(_)),
            "state",
        ),
        (
            b.auto_select.is_some(),
            matches!(node, Node::Grid(_)),
            "auto_select",
        ),
        (
            b.gap.is_some(),
            matches!(node, Node::Row(_) | Node::Column(_)),
            "gap",
        ),
        (b.legend.is_some(), matches!(node, Node::Chart(_)), "legend"),
        (
            b.title.is_some(),
            matches!(
                node,
                Node::PageHeader(_)
                    | Node::FilterPanel(_)
                    | Node::SidePanel(_)
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
            !b.actions.is_empty(),
            matches!(node, Node::PageHeader(_)),
            "actions",
        ),
        (
            b.help.is_some(),
            matches!(node, Node::PageHeader(_) | Node::SidePanel(_)),
            "help",
        ),
        (
            b.info.is_some(),
            matches!(
                node,
                Node::Heading(_) | Node::Label(_) | Node::PageHeader(_)
            ),
            "info",
        ),
        (b.tab.is_some(), matches!(node, Node::SidePanel(_)), "tab"),
        (
            b.empty.is_some(),
            matches!(node, Node::Grid(_) | Node::List(_) | Node::Collection(_)),
            "empty",
        ),
        (
            b.selection.is_some(),
            matches!(node, Node::List(_)),
            "selection",
        ),
        (
            b.per_row.is_some()
                || b.across.is_some()
                || b.preset.is_some()
                || b.cell.is_some()
                || b.mashup.is_some()
                || !b.map.is_empty(),
            matches!(node, Node::Collection(_) | Node::CardGrid(_)),
            "per_row/across/preset/cell/mashup/map",
        ),
        (
            b.collapsible.is_some(),
            matches!(node, Node::FilterPanel(_)),
            "collapsible",
        ),
        (
            b.search.is_some() || !b.groups.is_empty(),
            matches!(node, Node::NavMenu(_)),
            "search/groups",
        ),
        (b.band.is_some(), matches!(node, Node::Toolbar(_) | Node::Heading(_) | Node::Label(_) | Node::Text(_)), "band"),
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
        (!b.inputs.is_empty(), matches!(node, Node::Widget(_)), "inputs"),
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
                    | Node::Block(_)
            ),
            "value",
        ),
        (
            b.status.is_some() || b.color.is_some(),
            matches!(
                node,
                Node::Card(_) | Node::DetailPair(_) | Node::PageHeader(_) | Node::Block(_)
            ),
            "status/color",
        ),
        (b.align.is_some(), matches!(node, Node::Card(_)), "align"),
        (b.caption.is_some(), matches!(node, Node::Block(_)), "caption"),
        (
            b.highlight.is_some(),
            matches!(node, Node::Heading(_) | Node::Label(_) | Node::Text(_)),
            "highlight",
        ),
        (
            b.size
                .as_ref()
                .is_some_and(|s| matches!(s.value, Size::Sm | Size::Md | Size::Lg | Size::Xl | Size::Display)),
            false,
            "size sm, md, lg or display (a heading, label and text take them)",
        ),
        (
            b.style.iter().any(|s| s.value.starts_with('~')),
            false,
            "style map (background, color, border, font; label, heading, text, block and chart take it)",
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
            b.enabled_when.is_some(),
            matches!(node, Node::Button(_)),
            "enabled_when",
        ),
        (
            b.variant.is_some(),
            matches!(node, Node::Button(_) | Node::TextField(_)),
            "variant",
        ),
        (
            b.icon.is_some(),
            matches!(node, Node::Button(_) | Node::TextField(_)),
            "icon",
        ),
        (
            b.x.is_some() || b.stacked.is_some(),
            matches!(node, Node::Chart(_)),
            "x/stacked",
        ),
        (
            b.options.is_some(),
            matches!(node, Node::Dropdown(_)),
            "options",
        ),
        (
            b.display.is_some(),
            matches!(node, Node::Dropdown(_) | Node::List(_)),
            "display",
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
                if let Some(axis) = &series.value.axis {
                    if !["primary", "secondary"].contains(&axis.value.as_str()) {
                        return Err(Error::at(
                            axis.referenced,
                            "series axis is primary or secondary",
                        ));
                    }
                }
                if !["bar", "line"].contains(&series.value.kind.as_str()) {
                    return Err(Error::at(series.referenced, "series kind is bar or line"));
                }
            }
        }
        Node::Chart(_) if !b.series.is_empty() => {
            return Err(fail("series belongs to a combo chart"))
        }
        Node::Dropdown(_)
            if b.unbound != Some(true) && (b.options.is_none() || b.display.is_none()) =>
        {
            return Err(fail("dropdown needs options and display"))
        }
        Node::Pagination(_) if b.grid.is_none() => {
            return Err(fail("pagination needs for: <grid>"))
        }
        Node::Collection(_) => {
            if b.data.is_none() {
                return Err(fail("collection needs data"));
            }
            if b.cell.is_some() == b.mashup.is_some() {
                return Err(fail("collection needs exactly one of cell or mashup"));
            }
            if b.mashup.is_some() && b.map.is_empty() {
                return Err(fail(
                    "collection mashup needs map: {row field: cell parameter}",
                ));
            }
            if b.cell.is_some() && !b.map.is_empty() {
                return Err(fail("map belongs to a collection with mashup"));
            }
            for number in [&b.per_row, &b.across].into_iter().flatten() {
                if number.value == 0 {
                    return Err(Error::at(number.referenced, "must be above zero"));
                }
            }
        }
        Node::List(_) => {
            if b.data.is_none() || b.display.is_none() {
                return Err(fail("list needs data and display"));
            }
            if let Some(selection) = &b.selection {
                if !["single", "multiple"].contains(&selection.value.as_str()) {
                    return Err(Error::at(
                        selection.referenced,
                        "selection is single or multiple",
                    ));
                }
            }
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
        let name = options.value.as_str().ok_or_else(|| {
            Error::at(
                options.referenced,
                "dropdown options names a data declaration",
            )
        })?;
        if !page
            .data
            .contains_key(name.split('.').next().unwrap_or(name))
        {
            return Err(Error::at(
                options.referenced,
                format!("unknown data {name}"),
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn wire(
    solution: &crate::core::config::Solution,
    session: &mut crate::core::mashup::authoring::session::Session,
    model: &crate::core::types::Model,
    platform: Option<&crate::core::types::Platform>,
    page: &Page,
    ids: &std::collections::BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    use super::compile::{call, scalar};
    use crate::core::mashup::authoring::Recipe;
    let b = node.body();
    let id = &ids[&b.id.value];
    let loc = b.id.referenced;
    if matches!(node, Node::Dropdown(_)) && b.unbound == Some(true) {
        if b.options.is_some() {
            return Err(Error::at(loc, "unbound dropdown cannot also take options"));
        }
        if let Some(display) = &b.display {
            session
                .set(
                    solution,
                    &crate::core::mashup::authoring::SetRequest {
                        mashup: page.name.value.clone(),
                        widget: id.clone(),
                        properties: vec![("DisplayField".into(), json!(display))],
                        style_from: None,
                        styles: vec![],
                    },
                )
                .map_err(|e| Error::at(loc, e))?;
        }
    }
    let recipe = match node {
        Node::Chart(_) => Some(Recipe::Chart {
            series: vec![],
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
        Node::Dropdown(_) if b.unbound != Some(true) => Some(Recipe::Picker {
            widget: id.clone(),
            data: call(
                page,
                b.options
                    .as_ref()
                    .expect("options")
                    .value
                    .as_str()
                    .expect("options source"),
            ),
            display: b.display.clone().expect("display"),
            value: None,
            selected: None,
            no_none_row: false,
            auto_select_first: false,
            multiple: false,
        }),
        Node::List(_) => {
            let data = &b.data.as_ref().expect("list data").value;
            let display = b.display.clone().expect("list display");
            if let Some((list, _)) = data.split_once(".selected") {
                // The selected rows keep the rows' shape, so the field is checked on its source.
                let source = super::compile::find_list(page, list)
                    .and_then(|l| l.data.as_ref())
                    .expect("validated list source");
                session
                    .source_type(
                        solution,
                        model,
                        platform,
                        &format!("{}.{display}", super::compile::call(page, &source.value)),
                    )
                    .map_err(|e| Error::at(loc, e))?;
            }
            Some(Recipe::Picker {
                widget: id.clone(),
                data: super::compile::data_address(page, ids, data),
                display,
                value: None,
                selected: None,
                no_none_row: true,
                auto_select_first: false,
                multiple: b.selection.as_ref().is_some_and(|s| s.value == "multiple"),
            })
        }
        _ => None,
    };
    if let Some(recipe) = recipe {
        session
            .recipe(solution, recipe)
            .map_err(|e| Error::at(loc, e))?;
    }
    if let Some(state) = &b.state {
        let file = crate::core::workspace::entities(solution)
            .into_iter()
            .find(|f| f.info.collection == "StateDefinitions" && f.info.name == state.value)
            .ok_or_else(|| {
                Error::at(
                    state.referenced,
                    format!(
                        "no StateDefinition {}; add it to the solution first",
                        state.value
                    ),
                )
            })?;
        let bytes = std::fs::read(&file.path)
            .map_err(|e| Error::at(state.referenced, format!("{}: {e}", file.path.display())))?;
        let definition = crate::core::mashup::authoring::read_state_definition(&bytes)
            .map_err(|e| Error::at(state.referenced, e))?;
        let field = b.state_field.as_deref().unwrap_or("Data");
        let snapshot =
            crate::core::mashup::authoring::state_format::inline(&definition, &state.value, field)
                .map_err(|e| Error::at(state.referenced, e))?;
        let dot = &ids[&format!("{}--dot", b.id.value)];
        session
            .set(
                solution,
                &crate::core::mashup::authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: dot.clone(),
                    properties: vec![("ShapeFormatter".into(), snapshot)],
                    style_from: None,
                    styles: vec![],
                },
            )
            .map_err(|e| Error::at(state.referenced, e))?;
    }
    if matches!(node, Node::Chart(_)) && b.chart_kind.as_deref() == Some("combo") {
        // The recipe names every numeric column; the spec picks the series and their drawing.
        let series: Vec<_> = b
            .series
            .iter()
            .map(|s| {
                (
                    s.value.field.clone(),
                    s.value.kind.clone(),
                    s.value.title.clone(),
                    s.value.axis.as_ref().map(|a| a.value.clone()),
                )
            })
            .collect();
        let path = session
            .document
            .find_exact(id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let properties = crate::core::mashup::authoring::combo_props(
            &series,
            &session.document.at(&path)["Properties"],
        );
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
    use layout::height_floor;
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
        if session.document.at(&path)["Properties"]["flex-direction"] == "row"
            && body.children.len() > 1
            && body.children.iter().all(|n| matches!(n, Node::Chart(_)))
        {
            let mut charts = vec![];
            for child in &body.children {
                let chart = session
                    .document
                    .find_exact(&ids[&child.body().id.value])
                    .map_err(|e| Error::at(child.body().id.referenced, e))?
                    .path;
                charts.push(session.document.at(&chart[..chart.len() - 1]).clone());
            }
            session.document.at_mut(&path)["Widgets"] = json!(charts);
        }
        if session.document.at(&path)["Properties"]["Type"] == "flexcontainer"
            && matches!(
                body.size.as_ref().map(|s| &s.value),
                Some(super::schema::Size::Fit)
            )
        {
            // Placement can leave a fixed row height computed before its cards have content.
            patch(&mut session.document, &path, "page-content-height");
        }
        if matches!(node, Node::FilterPanel(_)) {
            let standalone = body.children.iter().all(|n| {
                body.footer
                    .iter()
                    .any(|f| n.body().id.value == format!("{}--{}", body.id.value, f.value))
            });
            let slots: Vec<_> = session
                .document
                .widgets()
                .into_iter()
                .filter(|w| w.path.starts_with(&path))
                .collect();
            for slot in slots {
                if slot.display_name == "filters" {
                    patch(&mut session.document, &slot.path, "page-filter-fields");
                }
                if slot.display_name == "filter-footer" {
                    patch(&mut session.document, &slot.path, "page-filter-footer");
                }
                if standalone
                    && (slot.display_name == "content"
                        || (slot.path.len() == path.len() + 1
                            && slot.display_name.starts_with("divider-")))
                {
                    session
                        .document
                        .set_property(&slot.path, "Visible", json!(false));
                }
            }
            if standalone {
                patch(&mut session.document, &path, "page-filter-only");
            }
        }
        if matches!(node, Node::DateRange(_)) {
            patch(&mut session.document, &path, "page-date-range");
            let range = body
                .props
                .get("DateRange")
                .is_none_or(|v| v.value != json!(false) && v.value != json!("false"));
            let narrow = session
                .document
                .widgets()
                .iter()
                .any(|w| path.starts_with(&w.path) && w.display_name == "filters");
            if narrow && !body.props.contains_key("Width") {
                session.document.set_property(&path, "Width", json!(240));
            }
            let width = body
                .props
                .get("Width")
                .and_then(|v| v.value.as_u64())
                .unwrap_or(if narrow { 240 } else { 340 });
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
        if let Some(value_size) = &body.value_size {
            // Reserve a caption, padding and a two-line value; wrapping stays uncapped.
            let pixels = value_size
                .trim_end_matches("px")
                .parse::<u32>()
                .expect("expanded block value size");
            for (key, value) in [
                ("flex-size", json!("range")),
                ("flex-min-height", json!(format!("{}px", pixels * 2 + 64))),
                ("flex-max-height", json!("")),
            ] {
                session.document.set_property(&path, key, value);
            }
        }
        if body.band_row {
            let label = session
                .document
                .find_exact(&ids[&body.children[0].body().id.value])
                .map_err(|e| Error::at(body.id.referenced, e))?;
            let label = session.document.at(&label.path).clone();
            // The primitive label wrapper otherwise keeps the text's measured width.
            session.document.at_mut(&path)["Widgets"] = json!([label]);
            patch(&mut session.document, &path, "page-content-height");
            session
                .document
                .set_property(&path, "flex-min-height", json!("48px"));
        }
        if matches!(
            node,
            Node::Widget(_) | Node::Row(_) | Node::Column(_) | Node::Panel(_)
        ) && session.document.at(&path)["Properties"]["Type"] == "flexcontainer"
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
        if matches!(node, Node::DetailPair(_)) && body.state.is_some() {
            // Responsive placement isolates the fixed mark in a zero-height branch.
            let dot = session
                .document
                .find_exact(&ids[&format!("{}--dot", body.id.value)])
                .map_err(|e| Error::at(body.id.referenced, e))?;
            let mark = session.document.at(&dot.path).clone();
            let mut members = vec![];
            for role in ["caption", "spacer", "value"] {
                let child = session
                    .document
                    .find_exact(&ids[&format!("{}--{role}", body.id.value)])
                    .map_err(|e| Error::at(body.id.referenced, e))?;
                let mut member = session
                    .document
                    .at(if role == "spacer" {
                        &child.path
                    } else {
                        &child.path[..child.path.len() - 1]
                    })
                    .clone();
                if role == "caption" {
                    member["Widgets"]
                        .as_array_mut()
                        .expect("caption widgets")
                        .insert(0, mark.clone());
                    member["Properties"]["flex-min-height"] = json!("12px");
                }
                members.push(member);
            }
            session.document.at_mut(&path)["Widgets"] = json!(members);
        }
        let widget = session
            .document
            .find_exact(&ids[&body.id.value])
            .map_err(|e| Error::at(body.id.referenced, e))?;
        if widget.path.len() < 2
            || crate::core::widgets::catalog()
                .by_type(&widget.type_id)
                .is_some_and(|w| w.function)
        {
            return Ok(());
        }
        let sizing = if widget.type_id == "flexcontainer" {
            path.clone()
        } else {
            path[..path.len() - 1].to_vec()
        };
        let parent = &sizing[..sizing.len() - 1];
        let horizontal = session.document.at(parent)["Properties"]["flex-direction"] == "row";
        let fill = matches!(
            body.size.as_ref().map(|s| &s.value),
            Some(super::schema::Size::Fill)
        ) || (body.size.is_none()
            && matches!(
                node,
                Node::Grid(_) | Node::Chart(_) | Node::Tabs(_) | Node::Collection(_)
            ))
            || body.weight.is_some();
        if fill {
            patch(&mut session.document, &sizing, "page-fill");
            session.document.set_property(
                &sizing,
                "flex-grow",
                number(body.weight.as_ref().map_or(1.0, |w| w.value)),
            );
        }
        if fill && matches!(node, Node::Grid(_)) && sizing.len() > 2 {
            let group = sizing[..sizing.len() - 1].to_vec();
            let has_pager = session
                .document
                .widgets()
                .iter()
                .any(|w| w.path.starts_with(&group) && w.type_id == "ptcspagination");
            if has_pager {
                patch(&mut session.document, &group, "page-fill");
            }
        }
        if horizontal
            && matches!(node, Node::Chart(_))
            && body.size.is_none()
            && body.weight.is_none()
        {
            patch(&mut session.document, &sizing, "page-panel-width");
        }
        if node.widget()? == "ptcschartline"
            && matches!(
                body.size.as_ref().map(|s| &s.value),
                Some(super::schema::Size::Fixed(_))
            )
        {
            patch(&mut session.document, &sizing, "page-content-height");
            if let Some(super::schema::Size::Fixed(px)) = body.size.as_ref().map(|s| &s.value) {
                session
                    .document
                    .set_property(&sizing, "flex-size", json!("fixed"));
                session
                    .document
                    .set_property(&sizing, "flex-min-height", json!(format!("{px}px")));
                session
                    .document
                    .set_property(&sizing, "flex-max-height", json!(format!("{px}px")));
            }
        } else if matches!(node, Node::Chart(_)) {
            let minimum = height_floor(session.document.at(&sizing)).max(240);
            patch(&mut session.document, &sizing, "page-chart-minimum");
            session.document.set_property(
                &sizing,
                "flex-min-height",
                json!(format!("{minimum}px")),
            );
            patch(&mut session.document, &path, "page-chart-legend");
            session
                .document
                .set_property(&path, "HideLegend", json!(!body.legend.unwrap_or(true)));
        }
        if let Some(gap) = &body.gap {
            let children: Vec<_> = body
                .children
                .iter()
                .skip(1)
                .map(|child| {
                    session
                        .document
                        .find_exact(&ids[&child.body().id.value])
                        .map(|w| w.path)
                        .map_err(|e| Error::at(child.body().id.referenced, e))
                })
                .collect::<Result<_, _>>()?;
            for child in children {
                let margin = if matches!(node, Node::Row(_)) {
                    format!("0 0 0 {}", gap.value)
                } else {
                    format!("{} 0 0 0", gap.value)
                };
                session
                    .document
                    .set_property(&child, "Margin", json!(margin));
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
    layout::finish_page(&mut session.document);
    fn overrides(
        document: &mut MashupDocument,
        node: &Node,
        ids: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), Error> {
        let body = node.body();
        let path = document
            .find_exact(&ids[&body.id.value])
            .map_err(|e| Error::at(body.id.referenced, e))?
            .path;
        for key in ["flex-min-height", "ShowVerticalScrollbar"] {
            if let Some(value) = body.props.get(key) {
                if super::compile::binding(&value.value).is_none() {
                    document.set_property(&path, key, value.value.clone());
                }
            }
        }
        for child in super::compile::descendants(node) {
            overrides(document, child, ids)?;
        }
        Ok(())
    }
    for node in page.slots.values().flat_map(|s| &s.value) {
        overrides(&mut session.document, node, ids)?;
    }
    Ok(())
}
