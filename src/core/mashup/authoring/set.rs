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
            return Err(AuthoringError::Invalid(format!(
                "{} has no property {name}",
                widget.type_id
            )));
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

/// A missing StyleProperties is allowed; an existing one must be Composer's object/map shape.
fn style_properties<'a>(
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

fn copy_style_entry(
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
