use super::{
    schema::{Body, Node, Size},
    Error,
};
use crate::core::mashup::tokens::Tokens;
use serde_json::{json, Value};
use serde_saphyr::Spanned;

fn put(body: &mut Body, key: &str, value: Value) {
    body.props
        .entry(key.into())
        .or_insert_with(|| Spanned::new(value, body.id.referenced, body.id.referenced));
}

fn field(body: &mut Body, key: &str, value: Option<Spanned<Value>>) {
    if let Some(value) = value {
        body.props.entry(key.into()).or_insert(value);
    }
}

/// A whole number is stored without a fraction, as Composer writes it.
#[allow(clippy::cast_possible_truncation)]
fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn choice(value: &Spanned<String>, choices: &[&str], name: &str) -> Result<(), Error> {
    if !choices.contains(&value.value.as_str()) {
        return Err(Error::at(
            value.referenced,
            format!("{name} must be {}", choices.join(", ")),
        ));
    }
    Ok(())
}

pub(super) fn expand(node: &mut Node, tokens: &Tokens) -> Result<(), Error> {
    let (element, widget) = match node {
        Node::Collection(b)
            if b.data.is_none()
                && b.mashup.is_some()
                && (b.source.is_some() || b.unbound == Some(true)) =>
        {
            ("collection", "ptcscollection")
        }
        Node::Pagination(b) if b.mode.is_some() => ("pagination", "ptcspagination"),
        Node::Link(_) => ("link", "ptcslink"),
        Node::Image(_) => ("image", "ptcsimage"),
        Node::Icon(_) => ("icon", "ptcsicon"),
        Node::Divider(_) => ("divider", "ptcsdivider"),
        Node::Shape(_) => ("shape", "shape"),
        Node::Checkbox(_) => ("checkbox", "ptcscheckbox"),
        Node::Radio(_) => ("radio", "ptcsradio"),
        Node::Toggle(_) => ("toggle", "ptcstogglebutton"),
        Node::Textarea(_) => ("textarea", "ptcstextarea"),
        Node::DatePicker(_) => ("date_picker", "ptcsdatepicker"),
        Node::ValueDisplay(_) => ("value_display", "ptcsvaluedisplay"),
        Node::MenuBar(_) => ("menu_bar", "ptcsmenubar"),
        Node::PageHost(_) => ("page_host", "pagemashupcontainer"),
        Node::Embed(_) => ("embed", "mashupcontainer"),
        Node::EventsRouter(_) => ("events_router", "eventsrouter"),
        Node::StatusMessage(_) => ("status_message", "statusmessage"),
        Node::RichText(_) => ("rich_text", "ptcsrte"),
        Node::Navigate(_) => ("navigate", "navigationfunction"),
        Node::MashupContainer(_) => ("mashup_container", "mashupcontainer"),
        Node::DynamicPanel(_) => ("dynamic_panel", "ptcsdynamicpanel"),
        _ => return Ok(()),
    };
    if element == "value_display"
        && node
            .body()
            .value
            .as_ref()
            .is_some_and(|v| super::compile::binding(&v.value).is_none())
    {
        let mut body: Body = serde_saphyr::from_str("{id: placeholder}").expect("body");
        std::mem::swap(node.body_mut(), &mut body);
        let value = body.value.take().expect("value");
        let base = match &value.value {
            Value::Bool(_) => "BOOLEAN",
            Value::Number(_) => "NUMBER",
            Value::String(_) => "STRING",
            _ => {
                return Err(Error::at(
                    value.referenced,
                    "value_display value must be scalar or a binding",
                ))
            }
        };
        let id = body.id.value.clone();
        let mut row: Node = serde_saphyr::from_str(
            &json!({"column": {"id": format!("{id}--value-row"), "size": "fit"}}).to_string(),
        )
        .expect("column");
        row.body_mut().id.referenced = body.id.referenced;
        let mut function: Node = serde_saphyr::from_str(&serde_json::json!({"widget": {"id": format!("{id}--value"), "type": "expression2", "props": {"Expression": format!("result = {};", value.value), "OutputBaseType": base, "AutoEvaluate": true, "FireOnFirstValue": true}}}).to_string()).expect("value expression");
        function.body_mut().initialize = true;
        function.body_mut().id.referenced = value.referenced;
        body.value = Some(Spanned::new(
            json!({"bind": format!("{id}--value.Output")} ),
            value.referenced,
            value.referenced,
        ));
        row.body_mut().children = vec![function, Node::ValueDisplay(body)];
        *node = row;
        return Ok(());
    }
    if element == "link" && node.body().icon.is_some() {
        let mut placeholder: Body = serde_saphyr::from_str("{id: placeholder}").expect("body");
        std::mem::swap(node.body_mut(), &mut placeholder);
        let mut link = placeholder;
        let icon = link.icon.take().expect("icon");
        let id = link.id.value.clone();
        let mut row: Node =
            serde_saphyr::from_str(&json!({"row": {"id": format!("{id}--row")}}).to_string())
                .expect("row");
        row.body_mut().id.referenced = link.id.referenced;
        let mut icon_node: Node = serde_saphyr::from_str(
            &json!({"icon": {"id": format!("{id}--icon"), "icon": icon.value}}).to_string(),
        )
        .expect("icon");
        icon_node.body_mut().id.referenced = link.id.referenced;
        row.body_mut().children = vec![icon_node, Node::Link(link)];
        *node = row;
        return Ok(());
    }
    if element == "radio" && node.body().options.is_some() {
        let mut placeholder: Body = serde_saphyr::from_str("{id: placeholder}").expect("body");
        std::mem::swap(node.body_mut(), &mut placeholder);
        let mut group = placeholder;
        let options = group.options.take().expect("options");
        let choices = options.value.as_array().filter(|a| !a.is_empty()).ok_or_else(|| Error::at(options.referenced, "radio options is a nonempty list; native data-bound radios use value and group"))?;
        let id = group.id.value.clone();
        let name = group.group.take().map_or(json!(id), |v| v.value);
        for (index, option) in choices.iter().enumerate() {
            if !option.is_string() {
                return Err(Error::at(
                    options.referenced,
                    "radio options entries must be labels",
                ));
            }
            let mut child: Node = serde_saphyr::from_str(&json!({"radio": {"id": format!("{id}--{}", index + 1), "text": option, "group": name}}).to_string()).expect("radio");
            child.body_mut().id.referenced = options.referenced;
            group.children.push(child);
        }
        *node = Node::Column(group);
        return Ok(());
    }
    let b = node.body_mut();
    let at = b.id.referenced;
    if b.kind.is_some() {
        return Err(Error::at(at, "type belongs to widget"));
    }
    if element != "dynamic_panel" && !b.children.is_empty() {
        return Err(Error::at(at, format!("{element} does not take children")));
    }
    if let Some(text) = b.text.take() {
        let property = match element {
            "link" => "Text",
            "checkbox" | "radio" | "toggle" | "textarea" | "date_picker" | "value_display" => {
                "Label"
            }
            _ => return Err(Error::at(at, format!("text does not belong to {element}"))),
        };
        field(b, property, Some(text));
    }
    match element {
        "navigate" | "mashup_container" => {
            b.native_parameters = true;
            let destination = b.destination.take();
            field(
                b,
                if element == "navigate" {
                    "TargetMashup"
                } else {
                    "Name"
                },
                destination,
            );
            if element == "navigate" {
                if let Some(mode) = b.mode.take() {
                    choice(
                        &mode,
                        &["replace", "new", "popup", "modal-popup", "fullscreen"],
                        "navigate mode",
                    )?;
                    put(b, "TargetWindowType", json!(mode.value));
                }
                let title = b.popup_title.take();
                let width = b.popup_width.take();
                let height = b.popup_height.take();
                let defs = b.parameter_defs.take();
                field(b, "PopupTitle", title);
                field(b, "PopupWidth", width);
                field(b, "PopupHeight", height);
                field(b, "_currentParameterDefs", defs);
            }
            for (name, value) in std::mem::take(&mut b.params) {
                b.native_param_names.insert(name.clone());
                field(b, &name, Some(value));
            }
        }
        "dynamic_panel" => {
            if !b.children.is_empty() && b.children.len() != 2 {
                return Err(Error::at(at, "dynamic_panel needs two pane children"));
            }
            let collapsed = b.collapsed.take();
            let side = b.side.take();
            let size = b.panel_size.take();
            let collapsed_size = b.collapsed_size.take();
            field(b, "Collapsed", collapsed);
            field(b, "AnchorLocation", side);
            field(b, "PanelSize", size);
            field(b, "SizeWhenCollapsed", collapsed_size);
        }
        "page_host" | "menu_bar" => {}
        "status_message" => {
            let message = b.message.take();
            field(b, "Message", message);
            if let Some(kind) = b.message_type.take() {
                choice(&kind, &["error", "info", "warning"], "message_type")?;
                put(b, "MessageType", json!(kind.value));
            }
        }
        "rich_text" => {
            let content = b.content.take();
            field(b, "Data", content);
            if let Some(read_only) = b.read_only.take() {
                put(b, "ReadOnly", json!(read_only.value));
            }
            if let Some(toolbar) = b.show_toolbar.take() {
                put(b, "HideToolbar", json!(!toolbar.value));
            }
            if let Some(mut font_size) = b.font_size.take() {
                let widget = crate::core::widgets::catalog()
                    .by_type("ptcsrte")
                    .expect("rich text");
                font_size.value = crate::core::mashup::authoring::property_value(
                    widget,
                    "DefaultFontSize",
                    font_size.value,
                )
                .map_err(|e| Error::at(font_size.referenced, e))?;
                field(b, "DefaultFontSize", Some(font_size));
            }
        }
        "events_router" => {
            let default_count = u32::try_from(b.routes.len().max(2))
                .map_err(|_| Error::at(at, "events_router supports at most 20 routes"))?;
            let count = b.input_count.take().map_or(default_count, |n| n.value);
            if !(1..=20).contains(&count) || b.routes.len() > count as usize {
                return Err(Error::at(
                    at,
                    "events_router input_count is 1 to 20 and must cover every route",
                ));
            }
            let base = b.data_type.take().map_or("STRING".into(), |v| v.value);
            crate::core::mashup::authoring::validate_expression_type(&base)
                .map_err(|e| Error::at(at, e))?;
            put(b, "DataType", json!(base));
            put(b, "NumberOfInputs", json!(count));
            for (index, route) in std::mem::take(&mut b.routes).into_iter().enumerate() {
                if route.value.is_null() {
                    continue;
                }
                if super::compile::binding(&route.value).is_none() {
                    return Err(Error::at(
                        route.referenced,
                        "events_router routes use {bind: ...} or null",
                    ));
                }
                field(b, &format!("Input{}", index + 1), Some(route));
            }
        }
        "collection" => {
            b.unbound = None;
            let mashup = b.mashup.take().expect("cell mashup");
            put(b, "Mashup", json!(mashup.value));
            if !b.map.is_empty() {
                let map = std::mem::take(&mut b.map)
                    .into_iter()
                    .map(|(k, v)| (k, json!(v.value)))
                    .collect::<serde_json::Map<_, _>>();
                put(b, "MashupPropertyBinding", Value::Object(map));
            }
            let source = b.source.take();
            field(b, "Data", source);
        }
        "pagination" => {
            let mode = b.mode.take().expect("mode");
            choice(&mode, &["server"], "pagination mode")?;
            let total = b.total.take();
            field(b, "ResultsNumber", total);
            if let Some(size) = b.page_size.take() {
                if size == 0 {
                    return Err(Error::at(at, "page_size must be above zero"));
                }
                put(b, "PageSize", json!(size));
            }
            if let Some(number) = b.page_number.take() {
                if number == 0 {
                    return Err(Error::at(at, "page_number must be above zero"));
                }
                put(b, "PageNumber", json!(number));
            }
        }
        "link" => {
            if b.url.is_some() && b.mashup.is_some() {
                return Err(Error::at(at, "link takes url or mashup, not both"));
            }
            if let Some(mashup) = b.mashup.take() {
                put(
                    b,
                    "LinkURL",
                    json!(format!(
                        "/Thingworx/Runtime/index.html#mashup={}",
                        mashup.value
                    )),
                );
            }
            let url = b.url.take();
            field(b, "LinkURL", url);
            if let Some(target) = b.target.take() {
                choice(&target, &["_self", "_blank", "_popup"], "target")?;
                put(b, "TargetWindow", json!(target.value));
            }
            if b.icon.is_some() {
                return Err(Error::at(
                    at,
                    "ptcslink has no icon property; place an icon beside the link",
                ));
            }
        }
        "image" | "icon" => {
            if let Some(mut source) = b.source.take() {
                if let Some(name) = source
                    .value
                    .as_str()
                    .filter(|s| !s.contains('/') && !s.contains(':') && !s.is_empty())
                {
                    source.value = json!(format!("/Thingworx/MediaEntities/{name}"));
                }
                field(b, "SourceURL", Some(source));
            }
            let alt = b.alt.take();
            field(b, "AlternateText", alt);
            if element == "image" {
                if let Some(scaling) = b.scaling.take() {
                    choice(
                        &scaling,
                        &[
                            "fit-x", "fit-y", "fit", "contain", "cover", "auto", "25%", "50%",
                            "75%",
                        ],
                        "scaling",
                    )?;
                    put(b, "Scaling", json!(scaling.value));
                }
                if let Some(position) = b.position.take() {
                    choice(
                        &position,
                        &["top", "center", "bottom", "left", "right"],
                        "position",
                    )?;
                    put(b, "Position", json!(position.value));
                }
            } else {
                if let Some(icon) = b.icon.take() {
                    let name = if icon.value.starts_with("cds:") {
                        icon.value
                    } else {
                        format!("cds:icon_{}", icon.value)
                    };
                    put(b, "SVGIcon", json!(name));
                }
                if let Some(size) = b.icon_size.take() {
                    choice(
                        &size,
                        &["small", "medium", "large", "xlarge", "custom"],
                        "icon_size",
                    )?;
                    put(b, "Size", json!(size.value));
                }
                if let Some(size) = &b.size {
                    let native = match size.value {
                        Size::Sm => Some("small"),
                        Size::Md => Some("medium"),
                        Size::Lg => Some("large"),
                        Size::Xl => Some("xlarge"),
                        _ => None,
                    };
                    if let Some(native) = native {
                        b.size = None;
                        put(b, "Size", json!(native));
                    }
                }
            }
        }
        "divider" => {
            if let Some(orientation) = b.orientation.take() {
                choice(&orientation, &["horizontal", "vertical"], "orientation")?;
                put(b, "VerticalDivider", json!(orientation.value == "vertical"));
            }
        }
        "shape" => {
            if let Some(form) = b.form.take() {
                choice(
                    &form,
                    &["circle", "rectangle", "roundrect", "ellipse"],
                    "form",
                )?;
                put(b, "ShapeType", json!(form.value));
            }
            for (key, value) in [
                ("ShapeStyle", b.fill.take()),
                ("UnfilledBackgroundStyle", b.unfilled.take()),
            ] {
                if let Some(style) = value {
                    field(b, key, Some(style));
                }
            }
            if let Some(radius) = b.radius.take() {
                put(b, "RectangleCornerRadius", number(radius.value));
            }
            if let Some(dynamic) = b.dynamic.take() {
                put(b, "EnableDynamicFill", json!(dynamic.value));
            }
            for (key, value) in [
                ("DynamicRangeMinimum", b.min.take()),
                ("DynamicRangeMaximum", b.max.take()),
            ] {
                if let Some(limit) = value {
                    put(b, key, number(limit.value));
                }
            }
            if let Some(orientation) = b.orientation.take() {
                choice(&orientation, &["horizontal", "vertical"], "orientation")?;
                put(b, "DynamicFillOrientation", json!(orientation.value));
            }
        }
        "checkbox" | "radio" | "toggle" => {
            let value = b.value.take();
            field(b, "State", value);
            if element == "radio" {
                let group = b.group.take();
                field(b, "Radiogroup", group);
            }
        }
        "textarea" => {
            let value = b.value.take();
            field(b, "Text", value);
            if let Some(max) = b.max_length.take() {
                put(b, "MaxNumberOfCharacters", json!(max));
            }
        }
        "date_picker" => {
            let value = b.value.take();
            field(b, "DateTime", value);
            put(b, "DateRange", json!(false));
            if let Some(only) = b.date_only.take() {
                put(b, "DateOnly", json!(only));
            }
            if let Some(format) = b.format.take() {
                put(b, "FormatToken", json!(format));
            }
        }
        "value_display" => {
            let value = b.value.take();
            field(b, "Data", value);
        }
        "embed" => {
            if b.mashup.is_none() {
                return Err(Error::at(
                    at,
                    "embed needs mashup; use mashup_container with destination for a runtime name",
                ));
            }
            b.native = b.mashup.take().map(|m| m.value);
            let params = std::mem::take(&mut b.params);
            for (name, value) in params {
                field(b, &name, Some(value));
            }
        }
        _ => unreachable!(),
    }
    if let Some(hint) = b.hint.take() {
        if !["textarea", "date_picker"].contains(&element) {
            return Err(Error::at(at, format!("hint does not belong to {element}")));
        }
        put(b, "HintText", json!(hint));
    }
    if let Some(color) = b.color.take() {
        if !["icon", "divider"].contains(&element) {
            return Err(Error::at(at, format!("color does not belong to {element}")));
        }
        b.style.push(Spanned::new(
            format!(
                "{}={}",
                if element == "divider" {
                    "line:background"
                } else {
                    "color"
                },
                tokens
                    .style("color", &color)
                    .map_err(|e| Error::at(at, e))?
            ),
            at,
            at,
        ));
    }
    super::elements::native_styles(b, tokens)?;
    let mut body: Body = serde_saphyr::from_str("{id: placeholder}").expect("body");
    std::mem::swap(b, &mut body);
    body.kind = Some(Spanned::new(widget.into(), at, at));
    *node = Node::Widget(body);
    Ok(())
}

/// Normalize named panes before component expansion and node-event hoisting recurse.
pub(super) fn panes(node: &mut Node) -> Result<(), Error> {
    if let Node::DynamicPanel(b) = node {
        if b.main.is_some() || b.panel.is_some() {
            if !b.children.is_empty() || b.main.is_none() || b.panel.is_none() {
                return Err(Error::at(
                    b.id.referenced,
                    "dynamic_panel needs both main and panel panes; do not also supply children",
                ));
            }
            b.children.push(*b.main.take().expect("main"));
            b.children.push(*b.panel.take().expect("panel"));
        }
    }
    Ok(())
}
