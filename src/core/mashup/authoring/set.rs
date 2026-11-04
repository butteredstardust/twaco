//! `mashup set`: checked property changes on one existing widget.

use super::{
    finish_edit, finish_edit_with_css, load_existing, parse_loaded, property_value, AuthoringError,
    EditPlan,
};
use crate::core::config::Solution;
use crate::core::mashup::edit::ROOT_ID;
use crate::core::widgets;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct SetRequest {
    pub mashup: String,
    pub widget: String,
    pub properties: Vec<(String, Value)>,
}

#[derive(Debug)]
pub struct SetPlan {
    pub(crate) edit: Option<EditPlan>,
    pub mashup: String,
    pub widget: String,
    pub type_id: String,
    pub changed: Vec<Change>,
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
            if ![
                "StyleProperties",
                "CustomClass",
                "DisableLegacyCustomClassStyles",
            ]
            .contains(&name.as_str())
            {
                return Err(AuthoringError::Refused("mashup-root properties are StyleProperties, CustomClass, DisableLegacyCustomClassStyles and CustomMashupCss".into()));
            }
            let root_widget = root_contract(name)?;
            let value = property_value(&root_widget, name, supplied.clone())?;
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
    })
}

fn root_contract(name: &str) -> Result<widgets::Widget, AuthoringError> {
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
    if !widget.defaults.contains_key(name) {
        return Err(AuthoringError::Invalid(format!(
            "mashup-root has no property {name}"
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
    json!({"mashup": plan.mashup, "widget": plan.widget, "type": plan.type_id, "changed": plan.changed.iter().map(|change| json!({"property": change.property, "before": change.before, "after": change.after})).collect::<Vec<_>>(), "files": files, "applied": applied})
}
