//! `mashup set`: checked property changes on one existing widget.

use super::new::validate_theme;
use super::{
    finish_edit, finish_edit_with_css, load_existing, parse_loaded, property_value, AuthoringError,
    EditPlan,
};
use crate::core::config::Solution;
use crate::core::mashup::edit::{insert_sorted, ROOT_ID};
use crate::core::mashup::layout;
use crate::core::{mashup, widgets};
use serde_json::{json, Map, Value};

#[derive(Clone, Debug)]
pub struct SetRequest {
    pub mashup: String,
    pub widget: String,
    pub properties: Vec<(String, Value)>,
    /// `<mashup>:<widget>` whose root style entry is copied to this widget.
    pub style_from: Option<String>,
    /// `[<part>[@<state>]:]<property>=<value>` settings merged into this widget's root style
    /// entry, after any `style_from` copy; an empty value removes the property.
    pub styles: Vec<String>,
}

#[derive(Debug)]
pub struct SetPlan {
    pub(crate) edit: Option<EditPlan>,
    pub mashup: String,
    pub widget: String,
    pub type_id: String,
    pub changed: Vec<Change>,
    pub style_copy: Option<StyleCopy>,
    pub notes: Vec<String>,
}

#[derive(Debug)]
pub struct StyleCopy {
    pub reference_mashup: String,
    pub reference_widget: String,
    /// False when the reference has no entry to copy.
    pub copied: bool,
}

#[derive(Debug)]
pub struct Change {
    pub property: String,
    pub before: Option<Value>,
    pub after: Value,
}

fn validate_properties(request: &SetRequest) -> Result<(), AuthoringError> {
    for (_, value) in &request.properties {
        crate::core::repo::refuse_git_bash_value(value)
            .map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    }
    Ok(())
}

pub(crate) fn plan_set(
    solution: &Solution,
    request: &SetRequest,
) -> Result<SetPlan, AuthoringError> {
    validate_properties(request)?;
    let loaded = load_existing(solution, &request.mashup)?;
    plan_set_loaded(solution, request, loaded, None)
}

pub(super) fn plan_set_loaded(
    solution: &Solution,
    request: &SetRequest,
    loaded: super::ExistingMashup,
    tokens: Option<&crate::core::mashup::tokens::Tokens>,
) -> Result<SetPlan, AuthoringError> {
    validate_properties(request)?;
    let mut document = parse_loaded(&loaded)?;
    let found = document
        .find_exact(&request.widget)
        .map_err(AuthoringError::Edit)?;
    let mut properties = request.properties.clone();
    let mut notes = menubar_properties(&document, &found.type_id, &found.id, &mut properties)?;
    notes.extend(menubar_limit(
        solution,
        &document,
        &found.type_id,
        &found.id,
        &mut properties,
    )?);
    let root = found.id == ROOT_ID;
    let widget = if root {
        None
    } else {
        Some(widgets::catalog().by_type(&found.type_id).ok_or_else(|| {
            AuthoringError::Refused(format!(
                "{} is not in the catalogue; twaco cannot check its properties",
                found.type_id
            ))
        })?)
    };
    if let Some(widget) = widget {
        if !widget.has_contract() {
            return Err(AuthoringError::Refused(format!(
                "{} is not in the catalogue; twaco cannot check its properties",
                widget.type_id
            )));
        }
    }
    let mut changed = Vec::new();
    let mut released = false;
    let before_responsive = document.at(&found.path)["Properties"]["ResponsiveLayout"].as_bool();
    let mut css = None;
    // What the widget's style was before this call: --style reports its change against it.
    let original_styles = document.root_properties().get("StyleProperties").cloned();
    let original_entry = original_styles
        .as_ref()
        .and_then(|styles| styles.get("data"))
        .and_then(|data| data.get(&found.id))
        .cloned();
    let style_copy = match &request.style_from {
        None => None,
        Some(reference) => {
            let (reference_mashup, reference_widget) = parse_style_from(reference)?;
            let reference_loaded = load_existing(solution, reference_mashup)?;
            let reference_document = parse_loaded(&reference_loaded)?;
            let reference_found = reference_document
                .find_exact(reference_widget)
                .map_err(AuthoringError::Edit)?;
            if reference_found.type_id != found.type_id {
                return Err(AuthoringError::Refused(format!(
                    "cannot copy style from {}:{} ({}) to {}:{} ({})",
                    reference_mashup,
                    reference_found.id,
                    reference_found.type_id,
                    request.mashup,
                    found.id,
                    found.type_id
                )));
            }
            let reference_styles =
                style_properties(reference_document.root_properties(), reference_mashup)?;
            // Check the target before any supplied root property can replace a malformed map.
            let _ = style_properties(document.root_properties(), &request.mashup)?;
            let entry = reference_styles
                .and_then(|styles| styles.get("data"))
                .and_then(Value::as_object)
                .and_then(|data| data.get(&reference_found.id))
                .cloned();
            if let Some(entry) = entry {
                let reference_meta = reference_styles
                    .and_then(|styles| styles.get("meta"))
                    .cloned();
                copy_style_entry(
                    &mut document,
                    &found.id,
                    entry,
                    reference_meta,
                    &mut changed,
                );
                Some(StyleCopy {
                    reference_mashup: reference_mashup.to_string(),
                    reference_widget: reference_found.id,
                    copied: true,
                })
            } else {
                Some(StyleCopy {
                    reference_mashup: reference_mashup.to_string(),
                    reference_widget: reference_found.id,
                    copied: false,
                })
            }
        }
    };
    if !request.styles.is_empty() {
        let active;
        let tokens = match tokens {
            Some(tokens) => tokens,
            None => {
                active = crate::core::mashup::tokens::Tokens::active(solution)
                    .map_err(AuthoringError::Invalid)?;
                &active
            }
        };
        let settings = request
            .styles
            .iter()
            .map(|text| parse_style_with(text, tokens))
            .collect::<Result<Vec<_>, _>>()?;
        if root {
            let page = document
                .widgets()
                .into_iter()
                .find(|widget| widget.type_id == "flexcontainer");
            let instead = page.map_or_else(String::new, |page| {
                format!(
                    "; style the page container instead: twaco mashup set {} {} --style background=<colour> --apply",
                    request.mashup, page.id
                )
            });
            return Err(AuthoringError::Refused(format!(
                "--style sets a widget's look and the mashup root has no style entry (page colour and fonts go on a container, or in a theme: --prop StyleTheme=<name>){instead}"
            )));
        }
        // The runtime applies a container's own style inside its bounding box, so `flex: 0 0 74px`
        // on a header made a 74px wide one; the container's flex properties are real properties.
        if found.type_id == "flexcontainer" {
            if let Some(setting) = settings.iter().find(|setting| {
                setting.part == "$"
                    && matches!(
                        setting.property.as_str(),
                        "flex" | "flex-grow" | "flex-shrink" | "flex-basis"
                    )
            }) {
                return Err(AuthoringError::Refused(format!(
                    "--style {} on a container resizes it inside its own box (a header became 74px wide); set it with --prop flex-grow=, flex-shrink=, flex-basis= or flex-min-height=/flex-max-height= instead",
                    setting.property
                )));
            }
        }
        let styler = styler_name(&found.type_id).ok_or_else(|| {
            AuthoringError::Refused(format!(
                "twaco does not know the style parts of {}; copy a styled one with --style-from <mashup:widget>",
                found.type_id
            ))
        })?;
        let current = style_properties(document.root_properties(), &request.mashup)?
            .and_then(|styles| styles.get("data"))
            .and_then(Value::as_object)
            .and_then(|data| data.get(&found.id))
            .cloned();
        // An entry --style-from copied a moment ago is the one these settings refine.
        let mut entry = current.unwrap_or_else(|| json!({"parts": {}, "widget": styler}));
        merge_styles(&mut entry, &settings, &request.mashup, &found.id)?;
        // The plan lists the entry once, measured against what the widget had before this call.
        let property = format!("StyleProperties.data.{}", found.id);
        changed.retain(|change| change.property != property);
        let emptied = entry
            .get("parts")
            .and_then(Value::as_object)
            .is_some_and(Map::is_empty);
        if emptied {
            // Nothing left to style: the entry goes, as Composer drops an unstyled widget's, and
            // a style map this call created for it goes too.
            if original_styles.is_none() {
                document.root_properties_mut().remove("StyleProperties");
            } else if let Some(data) = document
                .root_properties_mut()
                .get_mut("StyleProperties")
                .and_then(|styles| styles.get_mut("data"))
                .and_then(Value::as_object_mut)
            {
                data.remove(&found.id);
            }
        } else {
            copy_style_entry(&mut document, &found.id, entry.clone(), None, &mut changed);
            changed.retain(|change| change.property != property);
        }
        notes.extend(
            crate::core::mashup::style_check::entry_notes(&found.id, &entry)
                .into_iter()
                .map(|(_, note)| note),
        );
        let after = (!emptied).then_some(entry);
        if after != original_entry {
            changed.push(Change {
                property,
                before: original_entry.clone(),
                after: after.unwrap_or(Value::Null),
            });
        }
    }
    for (name, supplied) in &properties {
        if root {
            if name == "CustomMashupCss" {
                let text = supplied.as_str().ok_or_else(|| {
                    AuthoringError::Invalid("CustomMashupCss must be CSS text".into())
                })?;
                let before = css_text(&loaded.assets.css);
                if before != text {
                    css = Some(css_file_text(text));
                    changed.push(Change {
                        property: name.clone(),
                        before: Some(Value::String(before)),
                        after: Value::String(text.to_string()),
                    });
                }
                continue;
            }
            let current = document.at(&found.path)["Properties"].get(name).cloned();
            let parameter = document.value()["UI"]["Properties"]["_currentParameterDefs"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|definition| definition["ParameterName"] == name.as_str());
            if parameter {
                return Err(AuthoringError::Refused(format!(
                    "{name} is a mashup parameter's default; use mashup param set --default"
                )));
            }
            let root_widget = root_contract(name, current.as_ref())?;
            let value = property_value(&root_widget, name, supplied.clone())?;
            if name == "StyleTheme" {
                let theme = value
                    .as_str()
                    .ok_or_else(|| AuthoringError::Invalid("StyleTheme must be a string".into()))?;
                validate_theme(solution, theme)?;
            }
            change_property(&mut document, &found.path, name, value, &mut changed);
            continue;
        }
        let widget = widget.expect("non-root has a catalogue widget");
        if [
            "Id",
            "Type",
            "DisplayName",
            "__TypeDisplayName",
            "LastContainer",
            "userCannotRemove",
            "positioning",
        ]
        .contains(&name.as_str())
            || (name == "ResponsiveLayout" && widget.container)
        {
            return Err(AuthoringError::Refused(format!("twaco sets {name}")));
        }
        if ["_currentParameterDefs", "MashupParameters"].contains(&name.as_str()) {
            return Err(AuthoringError::Refused(format!("{name} is managed by mashup param / add-contained / the function-widget inputs; use those")));
        }
        let known = name == "ResponsiveLayout"
            || widget.defaults.contains_key(name)
            || widget.property(name).is_some()
            || widget.numbered_default(name).is_some()
            || is_state_parameters(widget, name);
        if !known {
            return Err(AuthoringError::Invalid(
                widget.no_property(&widget.type_id, name),
            ));
        }
        if widget
            .property(name)
            .is_some_and(|contract| !contract.editable)
        {
            return Err(AuthoringError::Refused(format!("{name} is not editable")));
        }
        let value = if name == "ResponsiveLayout" {
            match supplied {
                Value::Bool(value) => Value::Bool(*value),
                Value::String(value) if value == "true" || value == "false" => {
                    Value::Bool(value == "true")
                }
                _ => {
                    return Err(AuthoringError::Invalid(
                        "ResponsiveLayout must be true or false".into(),
                    ))
                }
            }
        } else if matches!(name.as_str(), "Width" | "Height")
            && (supplied.is_null() || supplied == "")
        {
            Value::Null
        } else {
            property_value(widget, name, supplied.clone())?
        };
        if let Some(contract) = widget.property(name) {
            if !contract.options.is_empty() && !contract.options.contains(&value) {
                return Err(AuthoringError::Invalid(format!(
                    "{name} for {} must be one of {}",
                    widget.type_id,
                    contract
                        .options
                        .iter()
                        .map(Value::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        if widget.type_id == "flexcontainer" {
            validate_flex_property(name, &value)?;
        }
        change_property(&mut document, &found.path, name, value, &mut changed);
    }
    if widget.is_some_and(|widget| !widget.container) {
        let explicit = properties
            .iter()
            .any(|(name, _)| name == "ResponsiveLayout");
        let sizing = properties
            .iter()
            .any(|(name, _)| matches!(name.as_str(), "FillContainer" | "Width" | "Height"));
        let props = &document.at(&found.path)["Properties"];
        let sized = crate::core::mashup::edit::has_leaf_size(props);
        let was_fixed = before_responsive == Some(false);
        let fill_on = properties.iter().any(|(name, value)| {
            name == "FillContainer" && (value == &Value::Bool(true) || value == "true")
        });
        if !explicit && sizing {
            // Fixed size switches responsive layout off; only turning FillContainer on or removing
            // the last size switches it back, so a deliberate setting survives a Width edit.
            let responsive = if props["FillContainer"] == false && sized {
                Some(false)
            } else if fill_on || !sized {
                Some(true)
            } else {
                None
            };
            if let Some(responsive) = responsive {
                change_property(
                    &mut document,
                    &found.path,
                    "ResponsiveLayout",
                    Value::Bool(responsive),
                    &mut changed,
                );
                if !responsive {
                    notes.push("ResponsiveLayout set to false so Width/Height apply".into());
                }
                released = responsive && was_fixed;
            }
        }
        if document.at(&found.path)["Properties"]["ResponsiveLayout"] == false && !sized {
            notes.push(format!(
                "{}: ResponsiveLayout=false without Width/Height; set Width and/or Height",
                found.id
            ));
        }
    }
    if found.type_id == "ptcstabset" {
        let count = super::new::tab_count(&document.at(&found.path)["Properties"]["NumberOfTabs"]);
        for (name, _) in &properties {
            let Some(rest) = name.strip_prefix("Tab") else {
                continue;
            };
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = digits.parse::<usize>() {
                if n == 0 || n > count {
                    return Err(AuthoringError::Invalid(format!(
                        "{name}: the tab set has {count} tab(s); set NumberOfTabs first"
                    )));
                }
            }
        }
        if changed
            .iter()
            .any(|change| change.property == "NumberOfTabs")
        {
            let added = super::new::sync_tabs(&mut document, &found.id, count)?;
            if !added.is_empty() {
                notes.push(format!("added tab container(s) {}", added.join(", ")));
            }
        }
    }
    // Composer shows a fixed-size container with its own palette icon and saves it that way.
    if found.type_id == "flexcontainer" {
        if let Some(size) = changed
            .iter()
            .find(|change| change.property == "flex-size")
            .map(|change| change.after.clone())
        {
            let node = layout::context::Node::op("set-flex-size", layout::Element::Container)
                .fact("size-fixed", size == "fixed");
            let decided = document.rules().evaluate(&node.context());
            for (key, value) in decided.patch() {
                change_property(&mut document, &found.path, key, value, &mut changed);
            }
        }
    }
    if found.type_id == "flexcontainer"
        && !changed.is_empty()
        && request
            .properties
            .iter()
            .any(|(key, _)| key.starts_with("flex-") || matches!(key.as_str(), "Width" | "Height"))
    {
        document
            .at_mut(&found.path)
            .as_object_mut()
            .expect("widget")
            .insert("twacoLayoutAuthored".into(), Value::Bool(true));
    }
    if !changed.is_empty() {
        document.reflow_for(&found.id);
        if widget.is_some_and(|widget| !widget.container) {
            document.fit_leaf_row(&found.id, released);
        }
    }
    let edit = if changed.is_empty() {
        None
    } else if let Some(css) = css {
        if loaded.sidecars.is_some() {
            document.root_mut().remove("CustomMashupCss");
        } else {
            document.set_top("CustomMashupCss", Value::String(css_text(&css)));
        }
        Some(finish_edit_with_css(
            loaded,
            document,
            "mashup set",
            Some(css),
        )?)
    } else {
        Some(finish_edit(loaded, document, "mashup set")?)
    };
    Ok(SetPlan {
        edit,
        mashup: request.mashup.clone(),
        widget: found.id,
        type_id: found.type_id,
        changed,
        style_copy,
        notes,
    })
}

fn parse_style_from(reference: &str) -> Result<(&str, &str), AuthoringError> {
    let Some((mashup, widget)) = reference.split_once(':') else {
        return Err(AuthoringError::Invalid(
            "--style-from must be <mashup>:<widget>".into(),
        ));
    };
    if mashup.is_empty() || widget.is_empty() {
        return Err(AuthoringError::Invalid(
            "--style-from must be <mashup>:<widget>".into(),
        ));
    }
    Ok((mashup, widget))
}

/// One `--style` setting: a CSS property of one part of a widget, in one state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StyleSetting {
    pub(crate) part: String,
    pub(crate) state: String,
    pub(crate) property: String,
    pub(crate) value: String,
}

/// `[<part>[@<state>]:]<property>=<value>`; the part and the state default to `$`, the widget
/// itself in its normal state, as Composer's style editor stores them.
pub(crate) fn parse_style(text: &str) -> Result<StyleSetting, AuthoringError> {
    parse_style_with(text, crate::core::mashup::tokens::Tokens::convergence())
}

fn parse_style_with(
    text: &str,
    tokens: &crate::core::mashup::tokens::Tokens,
) -> Result<StyleSetting, AuthoringError> {
    let bad = || {
        AuthoringError::Invalid(format!(
            "--style {text:?} must be [<part>[@<state>]:]<property>=<value>, such as background=#ffffff or cell@hover:border-color=#BEC6CA"
        ))
    };
    let (target, value) = text.split_once('=').ok_or_else(bad)?;
    let (scope, property) = match target.rsplit_once(':') {
        Some((scope, property)) => (Some(scope), property),
        None => (None, target),
    };
    let (part, state) = match scope {
        None => ("$", "$"),
        Some(scope) => scope.split_once('@').unwrap_or((scope, "$")),
    };
    let word = |text: &str, extra: &[char]| {
        !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || extra.contains(&c))
    };
    if !word(property, &[]) || !word(part, &['$']) || !word(state, &['$', '+']) {
        return Err(bad());
    }
    Ok(StyleSetting {
        part: part.to_string(),
        state: state.to_string(),
        property: property.to_string(),
        value: tokens
            .style(property, value.trim())
            .map_err(AuthoringError::Invalid)?,
    })
}

#[cfg(test)]
pub(crate) fn colour(text: &str) -> Result<String, String> {
    crate::core::mashup::tokens::Tokens::convergence().colour(text)
}

/// The styler name Composer records beside a widget's style parts. Each pair was read from the
/// root StyleProperties of the platform kits' mashups; a type they never style is not guessed.
fn styler_name(type_id: &str) -> Option<&'static str> {
    Some(match type_id {
        "flexcontainer" => "flex-container",
        "ptcsbreadcrumb" => "ptcs-breadcrumb",
        "ptcsbutton" => "ptcs-button",
        "ptcschartbar" => "ptcs-chart-bar",
        "ptcschartcombo" => "ptcs-chart-combo",
        "ptcschartline" => "ptcs-chart-line",
        "ptcschartpareto" => "ptcs-chart-pareto",
        "ptcschartpie" => "ptcs-chart-pie",
        "ptcschartschedule" => "ptcs-chart-schedule",
        "ptcscheckbox" => "ptcs-checkbox",
        "ptcscollection" => "ptcs-collection",
        "ptcsdatepicker" => "ptcs-datepicker",
        "ptcsdivider" => "ptcs-divider",
        "ptcsdropdown" => "ptcs-dropdown",
        "ptcsdynamicpanel" => "ptcs-dynamic-panel",
        "ptcsgrid" => "ptcs-grid",
        "ptcsimage" => "ptcs-image",
        "ptcskpidial" => "ptcs-kpi-dial",
        "ptcslabel" => "ptcs-label",
        "ptcslink" => "ptcs-link",
        "ptcslist" => "ptcs-list",
        "ptcslistshuttle" => "ptcs-list-shuttle",
        "ptcsmenubar" => "ptcs-menubar",
        "ptcspropertydisplay" => "ptcs-property-display",
        "ptcsradio" => "ptcs-radio",
        "ptcsslider" => "ptcs-slider",
        "ptcstabset" => "ptcs-tab-set",
        "ptcstextarea" => "ptcs-textarea",
        "ptcstextfield" => "ptcs-textfield",
        "ptcstogglebutton" => "ptcs-toggle-button",
        "ptcstoolbar" => "ptcs-toolbar",
        "ptcsvaluedisplay" => "ptcs-value-display",
        _ => return None,
    })
}

/// Set each property in `entry.parts.<part>.<state>`, keys sorted as Composer writes them; an
/// empty value removes the property, and a state or part left empty goes with it.
fn merge_styles(
    entry: &mut Value,
    settings: &[StyleSetting],
    mashup: &str,
    widget: &str,
) -> Result<(), AuthoringError> {
    let malformed = || {
        AuthoringError::Refused(format!(
            "{mashup}: the style entry of {widget} is not Composer's parts map; fix it in Composer"
        ))
    };
    let entry = entry.as_object_mut().ok_or_else(malformed)?;
    if !entry.contains_key("parts") {
        insert_sorted(entry, "parts", json!({}));
    }
    let parts = entry["parts"].as_object_mut().ok_or_else(malformed)?;
    for setting in settings {
        if !parts.contains_key(&setting.part) {
            insert_sorted(parts, &setting.part, json!({}));
        }
        let states = parts[&setting.part].as_object_mut().ok_or_else(malformed)?;
        if !states.contains_key(&setting.state) {
            insert_sorted(states, &setting.state, json!({}));
        }
        let css = states[&setting.state]
            .as_object_mut()
            .ok_or_else(malformed)?;
        if setting.value.is_empty() {
            css.remove(&setting.property);
        } else {
            insert_sorted(css, &setting.property, Value::String(setting.value.clone()));
        }
        if css.is_empty() {
            states.remove(&setting.state);
        }
        if states.is_empty() {
            parts.remove(&setting.part);
        }
    }
    Ok(())
}

/// Merge `--style` settings into one widget's style entry of a document being edited; what a
/// recipe needs to style a widget exactly as `mashup set --style` would.
pub(super) fn merge_widget_styles(
    document: &mut crate::core::mashup::edit::MashupDocument,
    id: &str,
    type_id: &str,
    texts: &[String],
    mashup: &str,
) -> Result<(), AuthoringError> {
    let settings = texts
        .iter()
        .map(|text| parse_style(text))
        .collect::<Result<Vec<_>, _>>()?;
    let styler = styler_name(type_id).ok_or_else(|| {
        AuthoringError::Refused(format!("twaco does not know the style parts of {type_id}"))
    })?;
    let current = style_properties(document.root_properties(), mashup)?
        .and_then(|styles| styles.get("data"))
        .and_then(Value::as_object)
        .and_then(|data| data.get(id))
        .cloned();
    let mut entry = current.unwrap_or_else(|| json!({"parts": {}, "widget": styler}));
    merge_styles(&mut entry, &settings, mashup, id)?;
    copy_style_entry(document, id, entry, None, &mut Vec::new());
    Ok(())
}

/// A missing StyleProperties is allowed; an existing one must be Composer's object/map shape.
pub(super) fn style_properties<'a>(
    properties: &'a Map<String, Value>,
    mashup: &str,
) -> Result<Option<&'a Map<String, Value>>, AuthoringError> {
    let Some(value) = properties.get("StyleProperties") else {
        return Ok(None);
    };
    let styles = value.as_object().ok_or_else(|| {
        AuthoringError::Refused(format!("{mashup}: StyleProperties must be an object"))
    })?;
    if !styles.get("data").is_some_and(Value::is_object) {
        return Err(AuthoringError::Refused(format!(
            "{mashup}: StyleProperties.data must be an object"
        )));
    }
    Ok(Some(styles))
}

pub(super) fn copy_style_entry(
    document: &mut crate::core::mashup::edit::MashupDocument,
    target_id: &str,
    entry: Value,
    reference_meta: Option<Value>,
    changed: &mut Vec<Change>,
) {
    let properties = document.root_properties_mut();
    if !properties.contains_key("StyleProperties") {
        let mut styles = Map::new();
        if let Some(meta) = reference_meta {
            insert_sorted(&mut styles, "meta", meta);
        } else {
            insert_sorted(
                &mut styles,
                "meta",
                json!({"type": "WidgetStyler", "version": "1.0"}),
            );
        }
        insert_sorted(&mut styles, "data", json!({target_id: entry.clone()}));
        insert_sorted(properties, "StyleProperties", Value::Object(styles));
        changed.push(Change {
            property: format!("StyleProperties.data.{target_id}"),
            before: None,
            after: entry,
        });
        return;
    }
    let styles = properties["StyleProperties"]
        .as_object_mut()
        .expect("checked: StyleProperties is an object");
    let data = styles["data"]
        .as_object_mut()
        .expect("checked: StyleProperties.data is an object");
    let before = data.get(target_id).cloned();
    if before.as_ref() != Some(&entry) {
        insert_sorted(data, target_id, entry.clone());
        changed.push(Change {
            property: format!("StyleProperties.data.{target_id}"),
            before,
            after: entry,
        });
    }
}

/// What the root takes: its styling, `Master` (the master mashup it opens in), and any other
/// scalar property it already has, typed by its current value. twaco's own keys stay refused.
fn root_contract(name: &str, current: Option<&Value>) -> Result<widgets::Widget, AuthoringError> {
    let mut widget = widgets::catalog()
        .by_type("flexcontainer")
        .expect("catalogue has flexcontainer")
        .clone();
    widget.type_id = "mashup-root".into();
    widget.defaults.clear();
    widget.defaults.insert("StyleProperties".into(), json!({}));
    widget.defaults.insert("CustomClass".into(), json!(""));
    widget
        .defaults
        .insert("DisableLegacyCustomClassStyles".into(), json!(false));
    widget.defaults.insert("Master".into(), json!(""));
    let reserved = [
        "Id",
        "Type",
        "DisplayName",
        "ResponsiveLayout",
        "id_index",
        "_currentParameterDefs",
        "MashupParameters",
    ]
    .contains(&name);
    if !reserved && !widget.defaults.contains_key(name) {
        if let Some(value @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) = current {
            widget.defaults.insert(name.into(), value.clone());
        }
    }
    if reserved || !widget.defaults.contains_key(name) {
        return Err(AuthoringError::Refused(format!(
            "mashup-root takes StyleProperties, CustomClass, DisableLegacyCustomClassStyles, CustomMashupCss, Master and the scalar properties it already has, not {name}"
        )));
    }
    Ok(widget)
}

fn is_state_parameters(widget: &widgets::Widget, name: &str) -> bool {
    name.strip_suffix("_StateParameters")
        .is_some_and(|property| {
            widget.property(property).is_some_and(|contract| {
                matches!(
                    contract.base_type.as_deref(),
                    Some("STATEFORMATTING" | "RENDERERWITHSTATE")
                )
            })
        })
}

fn change_property(
    document: &mut crate::core::mashup::edit::MashupDocument,
    path: &[usize],
    name: &str,
    after: Value,
    changed: &mut Vec<Change>,
) {
    let before = document.at(path)["Properties"].get(name).cloned();
    if before.as_ref() != Some(&after) && !(before.is_none() && after.is_null()) {
        if after.is_null() && matches!(name, "Width" | "Height") {
            document.at_mut(path)["Properties"]
                .as_object_mut()
                .expect("checked properties")
                .remove(name);
        } else {
            document.set_property(path, name, after.clone());
        }
        changed.push(Change {
            property: name.into(),
            before,
            after,
        });
    }
}

fn css_text(css: &str) -> String {
    css.strip_suffix("\r\n")
        .or_else(|| css.strip_suffix('\n'))
        .unwrap_or(css)
        .to_string()
}
fn css_file_text(css: &str) -> String {
    if css.ends_with('\n') {
        css.into()
    } else {
        format!("{css}\n")
    }
}

pub fn set_plan_json(solution: &Solution, plan: &SetPlan, applied: bool) -> Value {
    let files = plan.edit.as_ref().map_or_else(Vec::new, |edit| {
        let mut files = vec![solution.relative(&edit.entity_path)];
        if let Some(sidecars) = &edit.sidecars {
            if sidecars.content_before != sidecars.content_after {
                files.push(solution.relative(&sidecars.content_path));
            }
            if !mashup::same_css_bytes(&sidecars.css_before, &sidecars.css_after) {
                files.push(solution.relative(&sidecars.css_path));
            }
        }
        files
    });
    json!({"notes": plan.notes, "mashup": plan.mashup, "widget": plan.widget, "type": plan.type_id, "changed": plan.changed.iter().map(|change| json!({"property": change.property, "before": change.before, "after": change.after})).collect::<Vec<_>>(), "style_copy": plan.style_copy.as_ref().map(|copy| json!({"mashup": copy.reference_mashup, "widget": copy.reference_widget, "copied": copy.copied})), "files": files, "applied": applied})
}

/// A Menu entity and a service-backed menu are exclusive Runtime data sources.
pub(super) fn menubar_properties(
    document: &crate::core::mashup::edit::MashupDocument,
    type_id: &str,
    id: &str,
    properties: &mut Vec<(String, Value)>,
) -> Result<Vec<String>, AuthoringError> {
    if type_id != "ptcsmenubar"
        || !properties.iter().any(|(name, value)| {
            name == "PrimaryNavMenu" && value.as_str().is_some_and(|name| !name.is_empty())
        })
    {
        return Ok(Vec::new());
    }
    if document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|binding| {
            binding["TargetArea"] == "UI"
                && binding["TargetId"] == id
                && binding["PropertyMaps"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|map| {
                        matches!(
                            map["TargetProperty"].as_str(),
                            Some("Data" | "PrimaryNavData")
                        )
                    })
        })
    {
        return Err(AuthoringError::Refused(format!(
            "{id}.PrimaryNavMenu conflicts with its Data binding; remove that binding first with mashup unbind or keep the service-backed menu"
        )));
    }
    if properties.iter().any(|(name, _)| name == "DataSource") {
        if properties
            .iter()
            .any(|(name, value)| name == "DataSource" && value != "menuentity")
        {
            return Err(AuthoringError::Refused(
                "PrimaryNavMenu requires DataSource=menuentity; do not set a conflicting DataSource".into()
            ));
        }
        return Ok(Vec::new());
    }
    properties.push(("DataSource".into(), json!("menuentity")));
    Ok(vec![
        "DataSource set to menuentity because PrimaryNavMenu is set".into(),
    ])
}

pub(super) fn menubar_limit(
    solution: &Solution,
    document: &crate::core::mashup::edit::MashupDocument,
    type_id: &str,
    id: &str,
    properties: &mut Vec<(String, Value)>,
) -> Result<Vec<String>, AuthoringError> {
    if type_id != "ptcsmenubar" {
        return Ok(Vec::new());
    }
    let Some(menu) = properties
        .iter()
        .find(|(name, _)| name == "PrimaryNavMenu")
        .and_then(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
    else {
        return Ok(Vec::new());
    };
    let Some(entity) = crate::core::workspace::entities(solution)
        .into_iter()
        .find(|entity| entity.info.collection == "Menus" && entity.info.name == menu)
    else {
        return Ok(Vec::new());
    };
    let bytes = std::fs::read(&entity.path)
        .map_err(|error| AuthoringError::Unreadable(format!("Menu {menu}: {error}")))?;
    let tokens = crate::core::scan::tokenize(&bytes)
        .map_err(|error| AuthoringError::Unreadable(format!("Menu {menu}: {error}")))?;
    // Only the Menu's own items: MenuItem elements directly under its MenuItems.
    let mut stack: Vec<&[u8]> = Vec::new();
    let mut count = 0;
    for token in &tokens {
        let name = token.name.of(&bytes);
        match token.kind {
            crate::core::scan::Kind::Start | crate::core::scan::Kind::Empty => {
                if name == b"MenuItem" && stack.ends_with(&[b"Menu".as_slice(), b"MenuItems"]) {
                    count += 1;
                }
                if matches!(token.kind, crate::core::scan::Kind::Start) {
                    stack.push(name);
                }
            }
            crate::core::scan::Kind::End => {
                stack.pop();
            }
            _ => {}
        }
    }
    let path = document.find_exact(id).map_err(AuthoringError::Edit)?.path;
    let catalog = widgets::catalog()
        .by_type(type_id)
        .expect("menubar catalogue");
    if let Some((_, value)) = properties
        .iter()
        .find(|(name, _)| name == "PrimaryNavMaxItems")
    {
        super::property_value(catalog, "PrimaryNavMaxItems", value.clone())?;
    }
    let number = |value: &Value| value.as_f64().or_else(|| value.as_str()?.parse().ok());
    let limit = properties
        .iter()
        .find(|(name, _)| name == "PrimaryNavMaxItems")
        .map(|(_, value)| value)
        .or_else(|| document.at(&path)["Properties"].get("PrimaryNavMaxItems"))
        .or_else(|| catalog.defaults.get("PrimaryNavMaxItems"))
        .and_then(number)
        .unwrap_or(5.0);
    if count as f64 <= limit {
        return Ok(Vec::new());
    }
    properties.retain(|(name, _)| name != "PrimaryNavMaxItems");
    properties.push(("PrimaryNavMaxItems".into(), json!(count)));
    Ok(vec![format!(
        "PrimaryNavMaxItems set to {count} so all menu items show"
    )])
}

fn validate_flex_property(name: &str, value: &Value) -> Result<(), AuthoringError> {
    let options: &[&str] = match name {
        "flex-size" => &["default", "fixed", "range"],
        "flex-direction" => &["row", "column", "row-reverse", "column-reverse"],
        "flex-wrap" => &["nowrap", "wrap", "wrap-reverse"],
        "align-items" => &["flex-start", "flex-end", "center", "baseline", "stretch"],
        "align-content" => &[
            "flex-start",
            "flex-end",
            "center",
            "space-between",
            "space-around",
            "stretch",
        ],
        "justify-content" => &[
            "flex-start",
            "flex-end",
            "center",
            "space-between",
            "space-around",
            "space-evenly",
        ],
        _ => &[],
    };
    if !options.is_empty() && !value.as_str().is_some_and(|v| options.contains(&v)) {
        return Err(AuthoringError::Invalid(format!(
            "{name} must be one of {}",
            options.join(", ")
        )));
    }
    if matches!(
        name,
        "flex-min-height" | "flex-max-height" | "flex-min-width" | "flex-max-width"
    ) {
        let valid = value.as_str().is_some_and(|v| {
            v.is_empty()
                || v.strip_suffix("px")
                    .or_else(|| v.strip_suffix('%'))
                    .and_then(|n| n.parse::<f64>().ok())
                    .is_some_and(|n| n.is_finite() && n >= 0.0)
        });
        if !valid {
            return Err(AuthoringError::Invalid(format!(
                "{name} must be empty or a non-negative px or % value, such as 120px"
            )));
        }
    }
    Ok(())
}
