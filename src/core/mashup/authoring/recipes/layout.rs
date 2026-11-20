use super::{widget_id, AuthoringError, Work};
use crate::core::mashup::edit::{children, insert_sorted, WidgetRef};
use serde_json::{json, Value};

fn refuse(id: &str, reason: &str) -> AuthoringError {
    AuthoringError::Refused(format!("{id}: {reason}"))
}

fn panel_css(id: &str, compact: Option<u32>) -> String {
    let class = format!(".twaco-panel-{id}");
    let mut css = format!("/* twaco:panel:{id}:start */\n{class} {{\n  box-sizing: border-box;\n  flex: 1 1 auto !important;\n  min-height: 0 !important;\n  margin: 24px 24px 12px !important;\n  width: calc(100% - 48px) !important;\n}}\n");
    if let Some(width) = compact {
        css.push_str(&format!("@media (max-width: {width}px) {{\n  {class} {{\n    margin: 12px !important;\n    width: calc(100% - 24px) !important;\n  }}\n}}\n"));
    }
    css.push_str(&format!("/* twaco:panel:{id}:end */\n"));
    css
}

fn managed_css(css: &str, id: &str, block: &str) -> Result<String, AuthoringError> {
    let mut open = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut owned = None;
    let mut offset = 0;
    for line in css.split_inclusive('\n') {
        if line.contains("twaco:panel:") {
            let marker = line.trim_end_matches(['\r', '\n']);
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
        result.replace_range(range, block);
        Ok(result)
    } else {
        Ok(format!(
            "{css}{}{block}",
            if css.is_empty() || css.ends_with('\n') {
                ""
            } else {
                "\n"
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
        panel: bool,
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
        if !panel && explicit {
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
            json!(if panel { "#F7F7F7" } else { "inherit" }),
        );
        if panel {
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

    pub(super) fn panel(
        &mut self,
        container: &str,
        compact: Option<u32>,
        css: &str,
    ) -> Result<String, AuthoringError> {
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
        let block = panel_css(&target.id, compact);
        let css = managed_css(css, &target.id, &block)?;
        self.layout_set(
            &target,
            &[
                ("Style", json!("DefaultContainerStyle")),
                ("UseTheme", json!(true)),
                ("Margin", json!("24 24 12")),
                ("flex-direction", json!("column")),
                ("flex-wrap", json!("nowrap")),
                ("align-items", json!("flex-start")),
                ("align-content", json!("flex-start")),
                ("justify-content", json!("flex-start")),
                ("flex-size", json!("range")),
                ("flex-min-height", json!("0px")),
                ("flex-max-height", json!("")),
                ("flex-grow", json!(1)),
                ("flex-shrink", json!(1)),
                ("flex-basis", json!("auto")),
                ("iconClass", json!("widgets-flexcontainer")),
                ("CustomClass", json!(classes)),
            ],
        )?;
        self.background(&target, true, false)?;
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
                    if self.background(&wrapper, false, false)? {
                        inherited += 1;
                    } else {
                        preserved += 1;
                    }
                }
                paths.push(child);
            }
        }
        self.notes.push(format!("grey growing panel; inherited backgrounds on {inherited} wrappers; preserve {preserved} explicit backgrounds; widgets added: 0; bindings changed: 0; events changed: 0"));
        self.notes.push(match compact {
            Some(width) => format!(
                "panel gutters 24/24/12; compact gutters 12px at viewport width <= {width}px"
            ),
            None => "panel gutters 24/24/12; no compact media query".into(),
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
        self.background(&parent, false, true)?;
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
        assert_eq!(plain, "/* twaco:panel:flexcontainer-2:start */\n.twaco-panel-flexcontainer-2 {\n  box-sizing: border-box;\n  flex: 1 1 auto !important;\n  min-height: 0 !important;\n  margin: 24px 24px 12px !important;\n  width: calc(100% - 48px) !important;\n}\n/* twaco:panel:flexcontainer-2:end */\n");
        for width in [768, 960] {
            let compact = panel_css("flexcontainer-2", Some(width));
            assert!(compact.contains(&format!("@media (max-width: {width}px) {{\n  .twaco-panel-flexcontainer-2 {{\n    margin: 12px !important;\n    width: calc(100% - 24px) !important;\n  }}\n}}\n")));
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
