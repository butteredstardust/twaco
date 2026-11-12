//! `mashup set`: checked property changes on one existing widget.

use super::new::validate_theme;
use super::{
    finish_edit, finish_edit_with_css, load_existing, parse_loaded, property_value, AuthoringError,
    EditPlan,
};
use crate::core::config::Solution;
use crate::core::mashup::edit::{insert_sorted, ROOT_ID};
use crate::core::widgets;
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

pub(crate) fn plan_set(
    solution: &Solution,
    request: &SetRequest,
) -> Result<SetPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let found = document
        .find_exact(&request.widget)
        .map_err(AuthoringError::Edit)?;
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
        if !widget.has_contract() && widget.type_id != "flexcontainer" {
            return Err(AuthoringError::Refused(format!(
                "{} is not in the catalogue; twaco cannot check its properties",
                widget.type_id
            )));
        }
    }
    let mut changed = Vec::new();
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
        let settings = request
            .styles
            .iter()
            .map(|text| parse_style(text))
            .collect::<Result<Vec<_>, _>>()?;
        if root {
            return Err(AuthoringError::Refused(
                "--style sets a widget's look; name the widget, not the mashup root".into(),
            ));
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
        let after = (!emptied).then_some(entry);
        if after != original_entry {
            changed.push(Change {
                property,
                before: original_entry.clone(),
                after: after.unwrap_or(Value::Null),
            });
        }
    }
    for (name, supplied) in &request.properties {
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
            "ResponsiveLayout",
        ]
        .contains(&name.as_str())
        {
            return Err(AuthoringError::Refused(format!("twaco sets {name}")));
        }
        if ["_currentParameterDefs", "MashupParameters"].contains(&name.as_str()) {
            return Err(AuthoringError::Refused(format!("{name} is managed by mashup param / add-contained / the function-widget inputs; use those")));
        }
        let known = widget.defaults.contains_key(name)
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
        let value = property_value(widget, name, supplied.clone())?;
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
        change_property(&mut document, &found.path, name, value, &mut changed);
    }
    // Composer shows a fixed-size container with its own palette icon and saves it that way.
    if found.type_id == "flexcontainer" {
        if let Some(size) = changed
            .iter()
            .find(|change| change.property == "flex-size")
            .map(|change| change.after.clone())
        {
            let icon = if size == "fixed" {
                "widgets-flexcontainer-fixed"
            } else {
                "widgets-flexcontainer"
            };
            change_property(
                &mut document,
                &found.path,
                "iconClass",
                Value::String(icon.into()),
                &mut changed,
            );
        }
    }
    if !changed.is_empty() {
        document.reflow_for(&found.id);
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
        value: value.trim().to_string(),
    })
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
    if before.as_ref() != Some(&after) {
        document.set_property(path, name, after.clone());
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
            if sidecars.css_before != sidecars.css_after {
                files.push(solution.relative(&sidecars.css_path));
            }
        }
        files
    });
    json!({"mashup": plan.mashup, "widget": plan.widget, "type": plan.type_id, "changed": plan.changed.iter().map(|change| json!({"property": change.property, "before": change.before, "after": change.after})).collect::<Vec<_>>(), "style_copy": plan.style_copy.as_ref().map(|copy| json!({"mashup": copy.reference_mashup, "widget": copy.reference_widget, "copied": copy.copied})), "files": files, "applied": applied})
}
