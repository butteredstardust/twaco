use super::{widget_id, AuthoringError, Work};
use crate::core::mashup::edit::{children, insert_sorted, Placement, WidgetRef};
use crate::core::mashup::layout;
use crate::core::widgets;
use serde_json::{json, Value};

fn refuse(id: &str, reason: &str) -> AuthoringError {
    AuthoringError::Refused(format!("{id}: {reason}"))
}

/// The text field looks of the kit's input gallery (the inputs gallery). The kit sets them with master CSS
/// classes (wdk_Input2..4) that live outside the mashup, except the rounded corners, which are the
/// text-box part's `border-radius` in the exported StyleProperties. The other three are the
/// nearest native equivalents on the same part and its `focus` state; they are not measured at
/// run time against the kit's classes.
const INPUT_VARIANTS: &[&str] = &[
    "rounded",
    "focus-background",
    "focus-glow",
    "focus-underline",
];

/// The `--style` settings of one input variant, `None` for a name that is not one.
pub(crate) fn input_variant_styles(variant: &str) -> Option<Vec<String>> {
    let settings: &[&str] = match variant {
        "rounded" => &["text-box:border-radius=20px"],
        "focus-background" => &["text-box@focus:background=#EAF4FB"],
        "focus-glow" => &[
            "text-box@focus:border-color=#0088CE",
            "text-box@focus:box-shadow=0 0 6px 1px #0088CE",
        ],
        "focus-underline" => &[
            "text-box:border-width=0 0 1px 0",
            "text-box@focus:border-color=#0088CE",
            "text-box@focus:border-width=0 0 2px 0",
        ],
        _ => return None,
    };
    Some(settings.iter().map(|text| (*text).to_string()).collect())
}

/// One row of a kit gallery (the kit's inputs gallery): a row that takes an equal share of
/// the page's height. The kit keeps the size at `default` with `flex-basis: auto`; basis 0 makes
/// the shares equal whatever the cards hold.
pub(crate) fn grid_row_props() -> Vec<(&'static str, Value)> {
    vec![
        ("flex-direction", json!("row")),
        ("align-items", json!("stretch")),
        ("flex-size", json!("default")),
        ("flex-grow", json!(1)),
        ("flex-shrink", json!(1)),
        ("flex-basis", json!("0px")),
        ("flex-min-height", json!("")),
        ("flex-max-height", json!("")),
        ("Margin", json!("")),
    ]
}

/// One cell of a kit gallery row (containers 6 and 7): a growing column, with the equal widths
/// that basis 0 gives. The 10px margin is the 20px gap the kit's section class draws.
pub(crate) fn grid_cell_props() -> Vec<(&'static str, Value)> {
    vec![
        ("flex-direction", json!("column")),
        ("align-items", json!("stretch")),
        ("flex-size", json!("default")),
        ("flex-grow", json!(1)),
        ("flex-shrink", json!(1)),
        ("flex-basis", json!("0px")),
        ("flex-min-height", json!("")),
        ("flex-max-height", json!("")),
        ("Margin", json!("10")),
    ]
}

/// The hairline frame of a kit section (the kit draws it with the master class wdk_Section).
const SECTION_FRAME: [&str; 3] = [
    "border-style=solid",
    "border-width=1px",
    "border-color=#BEC6CA",
];

fn panel_css(id: &str, compact: Option<u32>) -> String {
    let class = format!(".twaco-panel-{id}");
    let mut css = format!("/* twaco:panel:{id}:start */\n{class} {{\n  box-sizing: border-box;\n  flex: 1 1 auto !important;\n  min-height: 0 !important;\n  margin: 24px 24px 12px !important;\n  width: calc(100% - 48px) !important;\n  height: calc(100% - 36px) !important;\n}}\n");
    if let Some(width) = compact {
        css.push_str(&format!("@media (max-width: {width}px) {{\n  {class} {{\n    margin: 12px !important;\n    width: calc(100% - 24px) !important;\n    height: calc(100% - 24px) !important;\n  }}\n}}\n"));
    }
    css.push_str(&format!("/* twaco:panel:{id}:end */\n"));
    css
}

const CARD_STYLES: &[(&str, &str)] = &[
    ("background", "#ffffff"),
    ("border-color", "#BEC6CA"),
    ("border-radius", "initial"),
    ("border-style", "solid"),
    ("border-width", "1px"),
    ("padding-bottom", "12px"),
    ("padding-left", "24px"),
    ("padding-right", "24px"),
    ("padding-top", "12px"),
];

fn panel_css_for(id: &str, compact: Option<u32>, style: &str) -> String {
    let css = panel_css(id, compact);
    if style == "card" {
        // 8px on every side: 16px between cards and 8px at the page edge (kit cards sit ~16px apart).
        css.replace("margin: 24px 24px 12px", "margin: 8px")
            .replace("width: calc(100% - 48px)", "width: calc(100% - 16px)")
            .replace("height: calc(100% - 36px)", "height: calc(100% - 16px)")
    } else {
        css
    }
}

fn managed_css(css: &str, id: &str, block: &str) -> Result<String, AuthoringError> {
    let newline = crate::core::workspace::line_ending(css);
    let block = block.replace("\r\n", "\n").replace('\n', newline);
    let mut open = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut owned = None;
    let mut offset = 0;
    for line in css.split_inclusive('\n') {
        let marker = line.trim();
        if marker.starts_with("/* twaco:panel:") && marker.ends_with(" */") {
            let Some(inner) = marker
                .strip_prefix("/* twaco:panel:")
                .and_then(|s| s.strip_suffix(" */"))
            else {
                return Err(refuse(
                    id,
                    "malformed managed panel CSS marker; repair custom.css",
                ));
            };
            let Some((target, kind)) = inner.rsplit_once(':') else {
                return Err(refuse(
                    id,
                    "malformed managed panel CSS marker; repair custom.css",
                ));
            };
            match kind {
                "start" if open.is_none() && !target.is_empty() && seen.insert(target) => {
                    open = Some((target, offset))
                }
                "end" if open.is_some_and(|(name, _)| name == target) => {
                    let (_, start) = open.take().unwrap();
                    if target == id {
                        owned = Some(start..offset + line.len());
                    }
                }
                _ => {
                    return Err(refuse(
                        id,
                        "duplicate or malformed managed panel CSS markers; repair custom.css",
                    ))
                }
            }
        }
        offset += line.len();
    }
    if open.is_some() {
        return Err(refuse(
            id,
            "unclosed managed panel CSS marker; repair custom.css",
        ));
    }
    if let Some(range) = owned {
        let mut result = css.to_string();
        result.replace_range(range, &block);
        Ok(result)
    } else {
        Ok(format!(
            "{css}{}{block}",
            if css.is_empty() || css.ends_with('\n') {
                ""
            } else {
                newline
            }
        ))
    }
}

impl Work<'_> {
    fn layout_container(&self, target: &WidgetRef, direction: &str) -> Result<(), AuthoringError> {
        let p = &self.document.at(&target.path)["Properties"];
        if target.path.is_empty()
            || target.type_id != "flexcontainer"
            || p["ResponsiveLayout"] != true
            || p["positioning"] == "static"
        {
            return Err(refuse(
                &target.id,
                "choose a non-root responsive flexcontainer",
            ));
        }
        if p["flex-direction"] != direction {
            return Err(refuse(
                &target.id,
                &format!("choose or create a {direction} container"),
            ));
        }
        Ok(())
    }

    fn checked_styles(&self, id: &str) -> Result<(), AuthoringError> {
        if let Some(styles) =
            super::super::set::style_properties(self.document.root_properties(), id)?
        {
            for (widget, entry) in styles["data"].as_object().unwrap() {
                let valid = entry.as_object().is_some_and(|e| {
                    e.get("parts").is_some_and(|parts| {
                        parts.as_object().is_some_and(|parts| {
                            parts.values().all(|states| {
                                states
                                    .as_object()
                                    .is_some_and(|states| states.values().all(Value::is_object))
                            })
                        })
                    })
                });
                if !valid {
                    return Err(refuse(
                        widget,
                        "malformed style parts; repair StyleProperties in Composer",
                    ));
                }
            }
        }
        Ok(())
    }

    fn guard_properties(&self, id: &str, names: &[&str]) -> Result<(), AuthoringError> {
        for binding in self.document.value()["DataBindings"]
            .as_array()
            .into_iter()
            .flatten()
        {
            for (side, property) in [
                ("TargetId", "TargetProperty"),
                ("SourceId", "SourceProperty"),
            ] {
                if binding[side] == id
                    && binding["PropertyMaps"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|map| {
                            map[property]
                                .as_str()
                                .is_some_and(|name| names.contains(&name))
                        })
                {
                    return Err(refuse(id, "a binding uses a property this recipe overwrites; remove that binding first"));
                }
            }
        }
        Ok(())
    }

    fn layout_set(
        &mut self,
        target: &WidgetRef,
        properties: &[(&str, Value)],
    ) -> Result<(), AuthoringError> {
        self.guard_properties(
            &target.id,
            &properties.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        )?;
        for (key, value) in properties {
            let before = self.document.at(&target.path)["Properties"][*key].clone();
            self.wired
                .push(format!("{}.{}: {before} -> {value}", target.id, key));
            self.document.set_property(&target.path, key, value.clone());
        }
        Ok(())
    }

    fn background(
        &mut self,
        target: &WidgetRef,
        panel: Option<&str>,
        allow_named: bool,
    ) -> Result<bool, AuthoringError> {
        let p = &self.document.at(&target.path)["Properties"];
        if p["Style"]
            .as_str()
            .is_some_and(|style| !style.is_empty() && style != "DefaultContainerStyle")
        {
            if allow_named {
                self.wired.push(format!(
                    "{}: preserve named container Style and background",
                    target.id
                ));
                return Ok(false);
            }
            return Err(refuse(
                &target.id,
                "non-default named Style; choose DefaultContainerStyle before applying this recipe",
            ));
        }
        let before = self
            .document
            .root_properties()
            .get("StyleProperties")
            .and_then(|s| s.get("data"))
            .and_then(|data| data.get(&target.id))
            .cloned();
        let mut entry = before
            .clone()
            .unwrap_or_else(|| json!({"widget": "flex-container", "parts": {"$": {"$": {}}}}));
        let explicit = entry["parts"]["$"]["$"].get("background").is_some();
        if panel.is_none() && explicit {
            self.wired
                .push(format!("{}.style: preserve {entry}", target.id));
            return Ok(false);
        }
        self.guard_properties(&target.id, &["StyleProperties"])?;
        self.guard_properties(crate::core::mashup::edit::ROOT_ID, &["StyleProperties"])?;
        let object = entry.as_object_mut().unwrap();
        insert_sorted(object, "widget", json!("flex-container"));
        let parts = object.get_mut("parts").unwrap().as_object_mut().unwrap();
        if !parts.contains_key("$") {
            insert_sorted(parts, "$", json!({}));
        }
        let states = parts.get_mut("$").unwrap().as_object_mut().unwrap();
        if !states.contains_key("$") {
            insert_sorted(states, "$", json!({}));
        }
        let styles = states.get_mut("$").unwrap().as_object_mut().unwrap();
        insert_sorted(
            styles,
            "background",
            json!(if panel.is_some() {
                "#F7F7F7"
            } else {
                "inherit"
            }),
        );
        if panel.is_some() {
            for (key, value) in [
                ("border-color", "inherit"),
                ("border-radius", "initial"),
                ("border-style", "none"),
                ("border-width", "0px"),
                ("padding-bottom", ""),
                ("padding-left", ""),
                ("padding-right", ""),
                ("padding-top", ""),
            ] {
                insert_sorted(styles, key, json!(value));
            }
        }
        if panel == Some("card") {
            for (key, value) in CARD_STYLES {
                insert_sorted(styles, key, json!(value));
            }
        }
        self.wired.push(format!(
            "{}.style: {} -> {entry}",
            target.id,
            before.unwrap_or(Value::Null)
        ));
        super::super::set::copy_style_entry(
            &mut self.document,
            &target.id,
            entry,
            None,
            &mut Vec::new(),
        );
        Ok(true)
    }

    pub(super) fn toolbar_band(&mut self, container: &str) -> Result<(), AuthoringError> {
        let target = widget_id(&self.document, container)?;
        self.layout_container(&target, "row")?;
        self.checked_styles(&target.id)?;
        let decided = self.document.rules().evaluate(
            &layout::context::Node::op("toolbar-band", layout::Element::Container).context(),
        );
        self.layout_set(&target, &decided.patch())?;
        self.background(&target, Some("grey"), false)?;
        self.notes.push("toolbar band: #F7F7F7, border none, fixed 48px; add date picker and buttons in this row".into());
        Ok(())
    }

    /// One bordered box with a caption, a value and a status chip, for grids of small status cards.
    /// `value` and `status` are text, or an address (`data:`, `ui:`) bound to the label's text.
    pub(super) fn card(
        &mut self,
        container: &str,
        caption: &str,
        value: Option<&str>,
        status: Option<&str>,
        kind: &str,
    ) -> Result<(), AuthoringError> {
        let Some((fill, ink)) = [
            ("info", ("#dbeafe", "#1e40af")),
            ("ok", ("#d2f2e0", "#0b6b3a")),
            ("warning", ("#ffe9c7", "#8a4b00")),
            ("error", ("#fbd5d5", "#8a1c1c")),
        ]
        .into_iter()
        .find_map(|(name, colours)| (name == kind).then_some(colours)) else {
            return Err(AuthoringError::Invalid(
                "card: --kind is info, ok, warning or error".into(),
            ));
        };
        let target = widget_id(&self.document, container)?;
        self.layout_container(&target, "column")?;
        if !children(self.document.at(&target.path)).is_empty() {
            return Err(refuse(
                &target.id,
                "card needs an empty container; it adds the caption, value and status itself",
            ));
        }
        self.checked_styles(&target.id)?;
        let decided = self.document.rules().evaluate(
            &layout::context::Node::op("card-recipe", layout::Element::Container).context(),
        );
        self.layout_set(&target, &decided.patch())?;
        self.background(&target, Some("card"), false)?;
        let is_address = |text: &str| text.starts_with("data:") || text.starts_with("ui:");
        let mut labels = vec![("caption", caption, "sub-header")];
        if let Some(value) = value {
            labels.push(("value", value, "title"));
        }
        for (role, text, kind_of_label) in labels {
            let literal = if is_address(text) { "" } else { text };
            let id = self.add_in(
                &target.path,
                "ptcslabel",
                &[
                    ("LabelText", json!(literal)),
                    ("LabelType", json!(kind_of_label)),
                    ("HorizontalAlignment", json!("left")),
                    ("Margin", json!("0 0 6")),
                    ("UseTheme", json!(true)),
                ],
            );
            if is_address(text) {
                self.bind(text, &format!("ui:{id}.LabelText"))?;
            }
            self.notes.push(format!("{id}: card {role}"));
        }
        if let Some(status) = status {
            let literal = if is_address(status) { "" } else { status };
            let width = (literal.chars().count() * 9 + 28).clamp(96, 240);
            let id = self.add_in(
                &target.path,
                "ptcslabel",
                &[
                    ("LabelText", json!(literal)),
                    ("LabelType", json!("body")),
                    ("HorizontalAlignment", json!("center")),
                    ("Margin", json!("0")),
                    ("Width", json!(width)),
                    ("Height", json!(24)),
                    ("UseTheme", json!(true)),
                ],
            );
            if is_address(status) {
                self.bind(status, &format!("ui:{id}.LabelText"))?;
            }
            super::super::set::copy_style_entry(
                &mut self.document,
                &id,
                json!({"widget": "ptcs-label", "parts": {
                    "$": {"$": {"background": fill, "border-radius": "3px"}},
                    "label": {"$": {"color": ink}}}}),
                None,
                &mut Vec::new(),
            );
            self.notes.push(format!("{id}: card status chip ({kind})"));
        }
        self.notes.push(format!(
            "{}: card; equal flex (grow 1, basis 0) so cards added to one row share its width",
            target.id
        ));
        Ok(())
    }

    /// The kit's grey left column (Compare, Administrator): 276px, a title row with an optional
    /// help icon over a one-tab strip. The tab's container is the column's content area.
    pub(super) fn side_panel(
        &mut self,
        container: &str,
        title: &str,
        info: Option<&str>,
        tab: &str,
    ) -> Result<(), AuthoringError> {
        let target = widget_id(&self.document, container)?;
        self.layout_container(&target, "column")?;
        if !children(self.document.at(&target.path)).is_empty() {
            return Err(refuse(
                &target.id,
                "side-panel needs an empty container; it adds the title row and the tab strip itself",
            ));
        }
        if title.trim().is_empty() || tab.trim().is_empty() {
            return Err(AuthoringError::Invalid(
                "side-panel: --title and --tab cannot be empty".into(),
            ));
        }
        self.checked_styles(&target.id)?;
        self.layout_set(
            &target,
            &[
                ("align-items", json!("stretch")),
                ("flex-size", json!("fixed")),
                ("flex-grow", json!(0)),
                ("flex-shrink", json!(0)),
                ("flex-min-width", json!("276px")),
                ("flex-max-width", json!("276px")),
                ("iconClass", json!("widgets-flexcontainer-fixed")),
            ],
        )?;
        self.background(&target, Some("grey"), false)?;
        let catalog = widgets::catalog();
        let flex = catalog
            .by_type("flexcontainer")
            .expect("flexcontainer catalogue");
        // The title row: a fixed 72px band, the label at the kit's header size.
        let row = self
            .document
            .place(flex, &Placement::In(target.id.clone()))
            .map_err(AuthoringError::Edit)?;
        self.added.push(row.id.clone());
        let row = widget_id(&self.document, &row.id)?;
        for (key, value) in [
            ("flex-direction", json!("row")),
            ("align-items", json!("flex-start")),
            ("flex-size", json!("fixed")),
            ("flex-grow", json!(0)),
            ("flex-min-height", json!("72px")),
            ("flex-max-height", json!("72px")),
            ("Margin", json!("0")),
        ] {
            self.document.set_property(&row.path, key, value);
        }
        self.background(&row, None, true)?;
        let label = self.add_in(
            &row.path,
            "ptcslabel",
            &[
                ("LabelText", json!(title)),
                ("LabelType", json!("header")),
                ("HorizontalAlignment", json!("left")),
                ("Margin", json!("18 0 18 18")),
                ("UseTheme", json!(true)),
            ],
        );
        self.notes.push(format!("{label}: side panel title"));
        if let Some(info) = info {
            let button = self.add_in(
                &row.path,
                "ptcsbutton",
                &[
                    ("ButtonType", json!("transparent")),
                    ("Disabled", json!(true)),
                    ("Label", json!("")),
                    ("Margin", json!("15 0 0")),
                    ("SVGIcon", json!("cds:icon_help_circle")),
                    ("TooltipField", json!(info)),
                ],
            );
            super::super::set::copy_style_entry(
                &mut self.document,
                &button,
                json!({"widget": "ptcs-button", "parts": {"$": {"disabled": {"color": "#4E585A"}}}}),
                None,
                &mut Vec::new(),
            );
            self.notes.push(format!("{button}: side panel help icon"));
        }
        // The tab strip (one tab, as the kits have it) fills the rest of the column.
        let tabs = self
            .document
            .place(
                catalog.by_type("ptcstabset").expect("tabset catalogue"),
                &Placement::In(target.id.clone()),
            )
            .map_err(AuthoringError::Edit)?;
        self.added.push(tabs.id.clone());
        let tabs = widget_id(&self.document, &tabs.id)?;
        self.document
            .set_property(&tabs.path, "Tab1Name", json!(tab));
        if self.document.rules().version() == "v1" {
            // Compare flexcontainer-103 wraps the tab strip and fills below the title band.
            let row = &tabs.path[..tabs.path.len() - 1];
            for (key, value) in [
                ("flex-size", json!("default")),
                ("flex-grow", json!(1)),
                ("flex-min-height", json!("")),
                ("flex-max-height", json!("")),
                ("LastContainer", json!(true)),
            ] {
                self.document.set_property(row, key, value);
            }
        }
        let content = super::super::new::sync_tabs(&mut self.document, &tabs.id, 1)?;
        super::super::set::copy_style_entry(
            &mut self.document,
            &tabs.id,
            json!({"widget": "ptcs-tab-set", "parts": {
                "pages": {"$": {"background": "inherit", "margin-bottom": "19px", "margin-left": "13px", "margin-right": "13px", "margin-top": "19px"}},
                "tabs-header": {"$": {"background": "inherit", "padding-left": "18px"}},
                "tabs-tab": {"selected": {"background": "inherit"}}}}),
            None,
            &mut Vec::new(),
        );
        let held = widget_id(&self.document, &content[0])?;
        for (key, value) in [
            ("flex-direction", json!("column")),
            ("align-items", json!("stretch")),
            ("flex-min-width", json!("0px")),
            ("flex-max-width", json!("250px")),
        ] {
            self.document.set_property(&held.path, key, value);
        }
        self.background(&held, None, true)?;
        self.wired.push(format!(
            "{}: side panel, 276px, tab {tab:?} holds {}",
            target.id, held.id
        ));
        Ok(())
    }

    /// A kit section (the kit's inputs gallery): a column of equal growth holding a fixed 40px
    /// title row, centred vertically, over a `LastContainer` body row. `center` centres the body's
    /// content on both axes, as the gallery centres its inputs.
    pub(super) fn section(
        &mut self,
        container: &str,
        title: &str,
        center: bool,
    ) -> Result<(), AuthoringError> {
        let target = widget_id(&self.document, container)?;
        self.layout_container(&target, "column")?;
        if !children(self.document.at(&target.path)).is_empty() {
            return Err(refuse(
                &target.id,
                "section needs an empty container; it adds the title row and the body row itself",
            ));
        }
        if title.trim().is_empty() {
            return Err(AuthoringError::Invalid(
                "section: --title cannot be empty".into(),
            ));
        }
        self.checked_styles(&target.id)?;
        let mut properties = grid_cell_props();
        properties.retain(|(key, _)| *key != "flex-direction");
        self.layout_set(&target, &properties)?;
        let frame: Vec<String> = SECTION_FRAME.iter().map(|s| (*s).to_string()).collect();
        super::super::set::merge_widget_styles(
            &mut self.document,
            &target.id,
            "flexcontainer",
            &frame,
            "mashup",
        )?;
        let catalog = widgets::catalog();
        let flex = catalog
            .by_type("flexcontainer")
            .expect("flexcontainer catalogue");
        let bar = self
            .document
            .place(flex, &Placement::In(target.id.clone()))
            .map_err(AuthoringError::Edit)?;
        self.added.push(bar.id.clone());
        let bar = widget_id(&self.document, &bar.id)?;
        for (key, value) in [
            ("flex-direction", json!("row")),
            ("align-items", json!("center")),
            ("justify-content", json!("flex-start")),
            ("flex-size", json!("fixed")),
            ("flex-grow", json!(1)),
            ("flex-shrink", json!(1)),
            ("flex-min-height", json!("40px")),
            ("flex-max-height", json!("40px")),
            ("Margin", json!("")),
            ("iconClass", json!("widgets-flexcontainer-fixed")),
        ] {
            self.document.set_property(&bar.path, key, value);
        }
        let label = self.add_in(
            &bar.path,
            "ptcslabel",
            &[
                ("LabelText", json!(title)),
                ("LabelType", json!("sub-header")),
                ("HorizontalAlignment", json!("left")),
                ("Margin", json!("0 0 0 10")),
            ],
        );
        self.notes.push(format!("{label}: section title"));
        let body = self
            .document
            .place(flex, &Placement::In(target.id.clone()))
            .map_err(AuthoringError::Edit)?;
        self.added.push(body.id.clone());
        let body = widget_id(&self.document, &body.id)?;
        let side = if center { "center" } else { "flex-start" };
        for (key, value) in [
            ("flex-direction", json!("row")),
            ("align-items", json!(side)),
            ("justify-content", json!(side)),
            ("flex-size", json!("default")),
            ("flex-grow", json!(1)),
            ("flex-shrink", json!(1)),
            ("flex-basis", json!("auto")),
            ("flex-min-height", json!("")),
            ("flex-max-height", json!("")),
            ("Margin", json!("")),
            ("LastContainer", json!(true)),
        ] {
            self.document.set_property(&body.path, key, value);
        }
        self.wired.push(format!(
            "{}: section {title:?}, content goes in {}",
            target.id, body.id
        ));
        Ok(())
    }

    /// A kit gallery (the kit's inputs gallery): rows of equal height, each holding
    /// `per_row` equal growing columns. The last row is filled with empty cells so every card
    /// keeps the same width.
    pub(super) fn card_grid(
        &mut self,
        container: &str,
        per_row: u32,
        cells: u32,
    ) -> Result<(), AuthoringError> {
        let target = widget_id(&self.document, container)?;
        self.layout_container(&target, "column")?;
        if !children(self.document.at(&target.path)).is_empty() {
            return Err(refuse(
                &target.id,
                "card-grid needs an empty container; it adds the rows and cells itself",
            ));
        }
        if !(1..=12).contains(&per_row) || !(1..=120).contains(&cells) {
            return Err(AuthoringError::Invalid(
                "card-grid: --per-row is 1 to 12 and --cells is 1 to 120".into(),
            ));
        }
        self.checked_styles(&target.id)?;
        self.layout_set(&target, &[("align-items", json!("stretch"))])?;
        let flex = widgets::catalog()
            .by_type("flexcontainer")
            .expect("flexcontainer catalogue");
        let rows = cells.div_ceil(per_row);
        let mut made = vec![];
        for _ in 0..rows {
            let row = self
                .document
                .place(flex, &Placement::In(target.id.clone()))
                .map_err(AuthoringError::Edit)?;
            self.added.push(row.id.clone());
            let row = widget_id(&self.document, &row.id)?;
            for (key, value) in grid_row_props() {
                self.document.set_property(&row.path, key, value);
            }
            for _ in 0..per_row {
                let cell = self
                    .document
                    .place(flex, &Placement::In(row.id.clone()))
                    .map_err(AuthoringError::Edit)?;
                self.added.push(cell.id.clone());
                let cell = widget_id(&self.document, &cell.id)?;
                for (key, value) in grid_cell_props() {
                    self.document.set_property(&cell.path, key, value);
                }
                made.push(cell.id);
            }
        }
        self.wired.push(format!(
            "{}: card grid, {rows} rows of {per_row}; cells {}",
            target.id,
            made.join(", ")
        ));
        Ok(())
    }

    /// One of the kit's text field looks (see `input_variant_styles`), written as the same
    /// managed style entry `mashup set --style` would write.
    pub(super) fn input_variant(
        &mut self,
        widget: &str,
        variant: &str,
    ) -> Result<(), AuthoringError> {
        let target = widget_id(&self.document, widget)?;
        if target.type_id != "ptcstextfield" {
            return Err(refuse(
                &target.id,
                &format!(
                    "input-variant needs a text field; this is a {}",
                    target.type_id
                ),
            ));
        }
        let styles = input_variant_styles(variant).ok_or_else(|| {
            AuthoringError::Invalid(format!(
                "input-variant: --variant is {}",
                INPUT_VARIANTS.join(", ")
            ))
        })?;
        self.checked_styles(&target.id)?;
        self.guard_properties(&target.id, &["StyleProperties"])?;
        super::super::set::merge_widget_styles(
            &mut self.document,
            &target.id,
            "ptcstextfield",
            &styles,
            "mashup",
        )?;
        self.wired
            .push(format!("{}: input look {variant}", target.id));
        Ok(())
    }

    /// The kit's "No results." state: a collection's and a grid's own text, or for a list (which
    /// has no such property) an eye-icon notice shown while the rows it shows are empty.
    pub(super) fn empty_state(
        &mut self,
        widget: &str,
        text: &str,
        data: Option<&str>,
    ) -> Result<(), AuthoringError> {
        let target = widget_id(&self.document, widget)?;
        match target.type_id.as_str() {
            "ptcscollection" => self.set(
                &target,
                &[
                    ("NoDataText", json!(text)),
                    ("NoDataIcon", json!("cds:icon_not_visible")),
                ],
            ),
            "ptcsgrid" => self.set(&target, &[("NoResultsLabel", json!(text))]),
            "ptcslist" => {
                let data = data.ok_or_else(|| {
                    AuthoringError::Invalid(
                        "empty-state on a list needs --data <the address of the rows it shows>"
                            .into(),
                    )
                })?;
                let button = self
                    .document
                    .place(
                        widgets::catalog()
                            .by_type("ptcsbutton")
                            .expect("button catalogue"),
                        &Placement::BelowInColumn(target.id.clone()),
                    )
                    .map_err(AuthoringError::Edit)?;
                self.added.push(button.id.clone());
                let button = widget_id(&self.document, &button.id)?;
                for (key, value) in [
                    ("ButtonType", json!("transparent")),
                    ("Disabled", json!(true)),
                    ("IconPosition", json!("top")),
                    ("Label", json!(text)),
                    ("SVGIcon", json!("cds:icon_not_visible")),
                    ("Margin", json!("24 0")),
                    ("MultiLine", json!(true)),
                ] {
                    self.document.set_property(&button.path, key, value);
                }
                super::super::set::copy_style_entry(
                    &mut self.document,
                    &button.id,
                    json!({"widget": "ptcs-button", "parts": {"$": {"disabled": {"color": "#000000"}}}}),
                    None,
                    &mut Vec::new(),
                );
                let notice_row = button.path[..button.path.len() - 1].to_vec();
                for (key, value) in [
                    ("flex-size", json!("range")),
                    ("flex-grow", json!(0)),
                    ("flex-shrink", json!(0)),
                    ("flex-basis", json!("auto")),
                    ("flex-min-height", json!("100px")),
                    ("flex-max-height", json!("")),
                    ("justify-content", json!("center")),
                    ("align-items", json!("center")),
                ] {
                    self.document.set_property(&notice_row, key, value);
                }
                let list = widget_id(&self.document, &target.id)?;
                let list_row = list.path[..list.path.len() - 1].to_vec();
                let region = list_row[..list_row.len() - 1].to_vec();
                // Only one branch is visible, so its height must not add to the other branch's.
                for (key, value) in [
                    ("flex-direction", json!("row")),
                    ("align-items", json!("stretch")),
                    ("flex-size", json!("range")),
                    ("flex-min-height", json!("0px")),
                    ("flex-max-height", json!("")),
                ] {
                    self.document.set_property(&region, key, value);
                }
                for row in [&list_row, &notice_row] {
                    self.document.set_property(row, "flex-grow", json!(1));
                    self.document.set_property(row, "flex-basis", json!("0px"));
                }
                let list_row_id = self.document.at(&list_row)["Properties"]["Id"]
                    .as_str()
                    .expect("list row id")
                    .to_string();
                let notice_row_id = self.document.at(&notice_row)["Properties"]["Id"]
                    .as_str()
                    .expect("notice row id")
                    .to_string();
                self.document
                    .set_property(&list_row, "Visible", json!(false));
                self.condition_gate(
                    &list_row_id,
                    false,
                    "!!(rows && (rows.rows || rows).length)",
                    &[("rows".into(), data.to_string())],
                )?;
                self.condition_gate(
                    &notice_row_id,
                    false,
                    "!(rows && (rows.rows || rows).length)",
                    &[("rows".into(), data.to_string())],
                )?;
                self.notes.push(format!(
                    "{}: empty-state notice, visible while the list's rows are empty",
                    button.id
                ));
            }
            other => {
                return Err(AuthoringError::Invalid(format!(
                    "{} is a {other}; empty-state needs a collection, grid or list",
                    target.id
                )))
            }
        }
        Ok(())
    }

    pub(super) fn panel(
        &mut self,
        container: &str,
        compact: Option<u32>,
        style: &str,
        fit: &str,
        css: &str,
    ) -> Result<String, AuthoringError> {
        if !["grey", "card"].contains(&style) {
            return Err(refuse(container, "--style is grey or card"));
        }
        if compact.is_some_and(|n| ![768, 960].contains(&n)) {
            return Err(refuse(container, "--compact must be 768 or 960"));
        }
        let target = widget_id(&self.document, container)?;
        self.layout_container(&target, "column")?;
        if !target
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(refuse(
                &target.id,
                "widget ID cannot form a safe CSS class token",
            ));
        }
        self.checked_styles(&target.id)?;
        self.guard_properties(crate::core::mashup::edit::ROOT_ID, &["CustomMashupCss"])?;
        let p = &self.document.at(&target.path)["Properties"];
        if p["Style"]
            .as_str()
            .is_some_and(|style| !style.is_empty() && style != "DefaultContainerStyle")
        {
            return Err(refuse(
                &target.id,
                "non-default named Style; choose DefaultContainerStyle before applying panel",
            ));
        }
        if !p["CustomClass"].is_null() && !p["CustomClass"].is_string() {
            return Err(refuse(
                &target.id,
                "CustomClass must be a string of class tokens",
            ));
        }
        for key in ["flex-min-width", "flex-max-width"] {
            if !p[key].is_null() && p[key] != "" {
                return Err(refuse(
                    &target.id,
                    "clear min/max width constraints for a full-width panel",
                ));
            }
        }
        let classes = p["CustomClass"].as_str().unwrap_or("");
        let token = format!("twaco-panel-{}", target.id);
        let classes = if classes.split_whitespace().any(|class| class == token) {
            classes.to_string()
        } else if classes.is_empty() {
            token
        } else {
            format!("{classes} {token}")
        };
        if !["fill", "content"].contains(&fit) {
            return Err(refuse(container, "--fit is fill or content"));
        }
        let mut block = panel_css_for(&target.id, compact, style);
        if fit == "content" {
            block = block.replace("flex: 1 1 auto", "flex: 0 1 auto");
            block = block
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with("height:") {
                        "  height: auto !important;"
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
        }
        self.notes.push(format!(
            "panel fit {fit}: {}",
            if fit == "content" {
                "no grow; height auto"
            } else {
                "fill available height"
            }
        ));
        if self.document.value()["twacoPageRules"] == "v1" {
            block = block
                .lines()
                .filter(|line| !line.trim_start().starts_with("min-height:"))
                .map(|line| {
                    if line.trim_start().starts_with("height:") {
                        "  height: auto !important;".to_string()
                    } else {
                        line.replace("flex: 1 1 auto", "flex: 1 0 auto")
                            .replace("flex: 0 1 auto", "flex: 0 0 auto")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
        }
        let css = managed_css(css, &target.id, &block)?;
        let node = layout::context::Node::op("panel-recipe", layout::Element::Container)
            .fact("style-card", style == "card")
            .fact("fit-content", fit == "content");
        let decided = self.document.rules().evaluate(&node.context());
        let mut patch = decided.patch();
        patch.push(("CustomClass", json!(classes)));
        self.layout_set(&target, &patch)?;
        self.background(&target, Some(style), false)?;
        let mut paths = vec![target.path.clone()];
        let mut inherited = 0;
        let mut preserved = 0;
        while let Some(path) = paths.pop() {
            let count = children(self.document.at(&path)).len();
            for index in 0..count {
                let mut child = path.clone();
                child.push(index);
                let node = self.document.at(&child);
                let id = node["Properties"]["Id"].as_str().unwrap_or("").to_string();
                let kind = node["Properties"]["Type"].as_str().unwrap_or("");
                if kind == "mashupcontainer" || kind == "containedmashup" {
                    continue;
                }
                if kind == "flexcontainer"
                    && node["Properties"]["ResponsiveLayout"] == true
                    && node["Properties"]["positioning"] != "static"
                {
                    let wrapper = widget_id(&self.document, &id)?;
                    if self.background(&wrapper, None, false)? {
                        inherited += 1;
                    } else {
                        preserved += 1;
                    }
                }
                paths.push(child);
            }
        }
        self.notes.push(format!("{style} growing panel; inherited backgrounds on {inherited} wrappers; preserve {preserved} explicit backgrounds; widgets added: 0; bindings changed: 0; events changed: 0"));
        if style == "card" {
            self.notes.push("card: white #ffffff, solid 1px #BEC6CA border, radius initial, padding 12px 24px; 8px margins keep 16px between cards".into());
        }
        self.notes.push(match compact {
            Some(width) => format!(
                "panel gutters {}; compact gutters 12px at viewport width <= {width}px",
                if style == "card" { "8" } else { "24/24/12" }
            ),
            None => format!(
                "panel gutters {}; no compact media query",
                if style == "card" { "8" } else { "24/24/12" }
            ),
        });
        self.wired.push(block);
        Ok(css)
    }

    pub(super) fn slider_action(
        &mut self,
        slider: &str,
        button: &str,
    ) -> Result<(), AuthoringError> {
        let slider = widget_id(&self.document, slider)?;
        let button = widget_id(&self.document, button)?;
        if slider.id == button.id
            || slider.type_id != "ptcsslider"
            || button.type_id != "ptcsbutton"
        {
            return Err(refuse(
                &slider.id,
                "slider-action needs distinct ptcsslider and ptcsbutton operands",
            ));
        }
        let Some((_, parent_path)) = slider.path.split_last() else {
            return Err(refuse(&slider.id, "choose controls in a responsive row"));
        };
        if button.path.split_last().map(|(_, path)| path) != Some(parent_path)
            || slider.path.last() != Some(&0)
            || button.path.last() != Some(&1)
            || children(self.document.at(parent_path)).len() != 2
        {
            return Err(refuse(
                &slider.id,
                "choose exactly a slider then a button as direct children of the same row",
            ));
        }
        let parent_id = self.document.at(parent_path)["Properties"]["Id"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let parent = widget_id(&self.document, &parent_id)?;
        self.layout_container(&parent, "row")?;
        self.checked_styles(&parent.id)?;
        let p = &self.document.at(&slider.path)["Properties"];
        if p["VerticalSlider"] == true {
            return Err(refuse(
                &slider.id,
                "VerticalSlider is unsupported; choose a horizontal slider",
            ));
        }
        let labels = p["DisplayMinMaxLabels"] != false;
        for control in [&slider, &button] {
            let p = &self.document.at(&control.path)["Properties"];
            if p["ResponsiveLayout"] == true || p["FillContainer"] == true {
                return Err(refuse(
                    &control.id,
                    "filled/responsive leaves prevent flex alignment; use flow-sized controls",
                ));
            }
        }
        self.layout_set(&slider, &[("Margin", json!("0"))])?;
        let margin = if labels { "0 0 18 12" } else { "0 0 0 12" };
        self.layout_set(&button, &[("Margin", json!(margin))])?;
        self.layout_set(
            &parent,
            &[
                ("Margin", json!("")),
                ("flex-direction", json!("row")),
                ("align-items", json!("flex-end")),
                ("align-content", json!("flex-start")),
                ("justify-content", json!("flex-start")),
                ("flex-wrap", json!("nowrap")),
                ("flex-size", json!("range")),
                ("flex-min-height", json!("0px")),
                ("flex-max-height", json!("")),
                ("flex-grow", json!(0)),
                ("flex-shrink", json!(0)),
                ("iconClass", json!("widgets-flexcontainer")),
            ],
        )?;
        self.background(&parent, None, true)?;
        self.notes.push(format!("button Margin {margin:?}: {}px for the min/max label row, 12px horizontal gap; track-centre alignment assumes the default 34px button", if labels {18} else {0}));
        self.notes.push(
            "preserve slider values, labels, dimensions, button type and existing wiring; no CSS"
                .into(),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_css_has_exact_rules_and_replaces_only_the_owned_block() {
        let plain = panel_css("flexcontainer-2", None);
        assert_eq!(plain, "/* twaco:panel:flexcontainer-2:start */\n.twaco-panel-flexcontainer-2 {\n  box-sizing: border-box;\n  flex: 1 1 auto !important;\n  min-height: 0 !important;\n  margin: 24px 24px 12px !important;\n  width: calc(100% - 48px) !important;\n  height: calc(100% - 36px) !important;\n}\n/* twaco:panel:flexcontainer-2:end */\n");
        for width in [768, 960] {
            let compact = panel_css("flexcontainer-2", Some(width));
            assert!(compact.contains(&format!("@media (max-width: {width}px) {{\n  .twaco-panel-flexcontainer-2 {{\n    margin: 12px !important;\n    width: calc(100% - 24px) !important;\n    height: calc(100% - 24px) !important;\n  }}\n}}\n")));
            let other = panel_css("flexcontainer-9", None);
            let original = format!("/* user CSS */\n{plain}{other}.after {{ color: red; }}");
            let expected = format!("/* user CSS */\n{compact}{other}.after {{ color: red; }}");
            assert_eq!(
                managed_css(&original, "flexcontainer-2", &compact).unwrap(),
                expected
            );
            assert_eq!(
                managed_css(&expected, "flexcontainer-2", &plain).unwrap(),
                original
            );
        }
        for css in [format!("{plain}{plain}"), "/* twaco:panel:x:start */\n/* twaco:panel:y:end */\n".into(), "/* twaco:panel:x:start */\n/* twaco:panel:y:start */\n/* twaco:panel:y:end */\n/* twaco:panel:x:end */\n".into()] {
            assert!(managed_css(&css, "flexcontainer-2", &plain).is_err());
        }
    }

    #[test]
    fn layout_recipe_arguments_require_only_their_own_options() {
        use super::super::{Recipe, RecipeArgs};
        let panel = RecipeArgs {
            widget: Some("ui:Panel".into()),
            ..Default::default()
        };
        assert!(Recipe::from_args("panel", &panel).is_ok());
        for compact in [0, 767, 769, 959, 961] {
            assert!(Recipe::from_args(
                "panel",
                &RecipeArgs {
                    compact: Some(compact),
                    ..panel.clone()
                }
            )
            .is_err());
        }
        let slider = RecipeArgs {
            widget: Some("ui:Slider".into()),
            button: Some("Action".into()),
            ..Default::default()
        };
        assert!(Recipe::from_args("slider-action", &slider).is_ok());
        assert!(Recipe::from_args("panel", &slider).is_err());
        assert!(Recipe::from_args("slider-action", &panel).is_err());
        assert!(Recipe::from_args(
            "slider-action",
            &RecipeArgs {
                compact: Some(768),
                ..slider.clone()
            }
        )
        .is_err());
        assert!(Recipe::from_args(
            "panel",
            &RecipeArgs {
                data: Some("data:Acme.Manager.GetData".into()),
                ..panel
            }
        )
        .is_err());
        assert!(Recipe::from_args(
            "slider-action",
            &RecipeArgs {
                width: Some(240),
                ..slider
            }
        )
        .is_err());
    }
}

#[cfg(test)]
mod css_tests {
    use super::*;

    #[test]
    fn managed_blocks_keep_crlf_and_ignore_embedded_markers() {
        let css =
            "a { content: \"/* twaco:panel:x:start */\"; }\r\n/* example twaco:panel:x:end */\r\n";
        let block = panel_css("x", None);
        let inserted = managed_css(css, "x", &block).unwrap();
        assert!(inserted.starts_with(css));
        assert!(!inserted.replace("\r\n", "").contains('\n'));
        assert_eq!(managed_css(&inserted, "x", &block).unwrap(), inserted);
        let indented = "  /* twaco:panel:x:start */\nold\n  /* twaco:panel:x:end */\n";
        assert_eq!(managed_css(indented, "x", &block).unwrap(), block);
    }
}
