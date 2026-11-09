//! Composer's inline StateDefinition snapshots, checked before a single mashup edit.

use super::{load_existing, parse_loaded, plan_set, AuthoringError, SetPlan, SetRequest};
use crate::core::{bindings, config::Solution, scan, types, widgets, workspace};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct StateFormatSetRequest {
    pub mashup: String,
    pub widget: String,
    pub state: String,
    pub property: Option<String>,
    pub column: Option<String>,
    pub field: Option<String>,
    pub icon_only: bool,
}

fn invalid(message: impl Into<String>) -> AuthoringError {
    AuthoringError::Refused(message.into())
}

pub(crate) fn read_definition(bytes: &[u8]) -> Result<Value, AuthoringError> {
    let tokens = scan::scan(bytes)
        .map_err(|e| invalid(e.to_string()))?
        .tokens;
    let start = scan::find_tag(&tokens, bytes, "content", 0, tokens.len())
        .ok_or_else(|| invalid("StateDefinition has no content"))?;
    let end = scan::element_end(&tokens, start)
        .ok_or_else(|| invalid("StateDefinition content is not closed"))?;
    if end == start {
        return Err(invalid("StateDefinition content must be JSON text"));
    }
    let text = scan::content_text(&bytes[tokens[start].span.end..tokens[end].span.start])
        .ok_or_else(|| invalid("StateDefinition content must be JSON text"))?;
    serde_json::from_str(&text)
        .map_err(|e| invalid(format!("invalid StateDefinition content: {e}")))
}

fn empty_style() -> Value {
    json!({"backgroundColor":"", "displayString":"", "fontEmphasisBold":false,
        "fontEmphasisItalic":false, "fontEmphasisUnderline":false, "foregroundColor":"",
        "image":"", "lineColor":"", "lineStyle":"solid", "lineThickness":1,
        "secondaryBackgroundColor":"", "textSize":"normal"})
}

pub(crate) fn inline(definition: &Value, name: &str, field: &str) -> Result<Value, AuthoringError> {
    let kind = definition["stateType"]
        .as_str()
        .filter(|kind| matches!(*kind, "numeric" | "string" | "fixed"))
        .ok_or_else(|| invalid(format!("StateDefinition {name} has no supported stateType")))?;
    let entries = definition["stateDefinitions"].as_array().ok_or_else(|| {
        invalid(format!(
            "StateDefinition {name} has no stateDefinitions array"
        ))
    })?;
    let mut states = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        // Composer saves the final catch-all state without a defaultValue (9 of 10 kit
        // StateDefinitions); any other state must name the value it matches.
        let last = index + 1 == entries.len();
        let value = match (entry.get("defaultValue"), last) {
            (Some(Value::String(value)), _) => value.as_str(),
            (None, true) => "",
            _ => {
                return Err(invalid(format!(
                    "StateDefinition {name} has a state without defaultValue"
                )))
            }
        };
        let style = match &entry["defaultStyleDefinition"] {
            Value::String(style) if style.is_empty() => empty_style(),
            style @ (Value::Object(_) | Value::String(_)) => style.clone(),
            _ => {
                return Err(invalid(format!(
                    "StateDefinition {name} has an invalid defaultStyleDefinition"
                )))
            }
        };
        let label = entry["displayString"].as_str().ok_or_else(|| {
            invalid(format!(
                "StateDefinition {name} has a state without displayString"
            ))
        })?;
        let mut state = json!({"defaultValue":value, "stateName":label, "state":style});
        if !last || !value.is_empty() {
            state["value"] = json!(value);
            state["comparator"] = if kind == "string" {
                json!("==")
            } else {
                json!(entry["comparator"].as_str().ok_or_else(|| invalid(format!(
                    "StateDefinition {name} has a state without comparator"
                )))?)
            };
        }
        states.push(state);
    }
    Ok(
        json!({"FieldName":field,"StateDefinition":name,"StateDefinitionType":kind,"StateFormats":states}),
    )
}

pub(crate) fn plan_state_format_set(
    solution: &Solution,
    request: &StateFormatSetRequest,
) -> Result<SetPlan, AuthoringError> {
    if request.property.is_some() == request.column.is_some() {
        return Err(invalid("give exactly one of --property and --column"));
    }
    if request.icon_only && request.column.is_none() {
        return Err(invalid("--icon-only requires --column"));
    }
    let file = workspace::entities(solution)
        .into_iter()
        .find(|file| file.info.collection == "StateDefinitions" && file.info.name == request.state)
        .ok_or_else(|| {
            invalid(format!(
                "no StateDefinition {}; add it to the solution first",
                request.state
            ))
        })?;
    let bytes =
        std::fs::read(&file.path).map_err(|e| invalid(format!("{}: {e}", file.path.display())))?;
    let definition = read_definition(&bytes).map_err(|error| {
        invalid(format!(
            "StateDefinition {}: {error}; repair its content with state set",
            request.state
        ))
    })?;
    let loaded = load_existing(solution, &request.mashup)?;
    let document = parse_loaded(&loaded)?;
    let found = document
        .find_exact(&request.widget)
        .map_err(AuthoringError::Edit)?;
    let widget = widgets::catalog()
        .by_type(&found.type_id)
        .ok_or_else(|| invalid(format!("{} is not in the widget catalogue", found.type_id)))?;
    if let Some(property) = &request.property {
        let allowed: Vec<_> = widget
            .properties
            .iter()
            .flat_map(|properties| properties.iter())
            .filter(|(_, contract)| contract.base_type.as_deref() == Some("STATEFORMATTING"))
            .map(|(name, _)| name.clone())
            .collect();
        if !allowed.contains(property) {
            return Err(invalid(format!(
                "{} property {property} is not STATEFORMATTING; STATEFORMATTING properties: {}",
                found.type_id,
                allowed.join(", ")
            )));
        }
    }
    let field = request
        .field
        .as_deref()
        .or(request.column.as_deref())
        .or_else(|| {
            // The KPI dial's formatter judges its Value; other widgets need an explicit field.
            (found.type_id == "ptcskpidial").then_some("Value")
        })
        .filter(|field| !field.is_empty())
        .ok_or_else(|| {
            invalid(format!(
                "give --field for {}; {} has no unambiguous default field",
                found.id, found.type_id
            ))
        })?;
    let snapshot = inline(&definition, &request.state, field)?;
    let properties = &document.at(&found.path)["Properties"];
    let changes = if let Some(property) = &request.property {
        vec![
            (property.clone(), snapshot),
            (format!("{property}_StateParameters"), json!([])),
        ]
    } else {
        let column = request.column.as_ref().expect("one target checked");
        if found.type_id != "ptcsgrid" {
            return Err(invalid(format!(
                "{} is not a ptcsgrid; --column requires a grid",
                found.id
            )));
        }
        let mut format = properties["ColumnFormat"].clone();
        let target = format
            .get_mut("formatInfo")
            .and_then(Value::as_array_mut)
            .and_then(|columns| {
                columns
                    .iter_mut()
                    .find(|entry| entry["FieldName"] == *column)
            })
            .ok_or_else(|| {
                invalid(format!(
                    "{} has no column {column}; use mashup recipe columns first",
                    found.id
                ))
            })?;
        if !target["FormatOptions"].is_object() {
            return Err(invalid(format!(
                "{} column {column} has invalid FormatOptions",
                found.id
            )));
        }
        target["FormatOptions"]["formatInfo"] = snapshot;
        if request.icon_only {
            target["FormatOptions"]["FormatString"] = json!("notext");
        }
        vec![("ColumnFormat".into(), format)]
    };
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let resolver = bindings::Resolver::new(&model, platform.as_ref());
    let sections = bindings::data_sections(document.value());
    let shape = bindings::whole_data_bindings(document.value())
        .into_iter()
        .find(|binding| binding.widget == found.id)
        .and_then(|binding| {
            let (section, call) = bindings::service_at(&sections, &binding.section, &binding.call)?;
            let service = resolver.service(section, &call.target).service?;
            service
                .result
                .base_type
                .eq_ignore_ascii_case("INFOTABLE")
                .then(|| service.result.data_shape.clone())
                .flatten()
        })
        .and_then(|name| resolver.checkable_shape(&name));
    if let Some(shape) = shape {
        let known = shape
            .fields
            .iter()
            .find(|known| known.name == field)
            .ok_or_else(|| {
                invalid(format!(
                    "{} DataShape {} has no field {field}; choose an existing --field",
                    found.id, shape.name
                ))
            })?;
        if definition["stateType"] == "numeric"
            && !matches!(known.base_type.as_str(), "NUMBER" | "INTEGER" | "LONG")
        {
            return Err(invalid(format!(
                "field {field} is {}; a numeric StateDefinition requires NUMBER, INTEGER or LONG",
                known.base_type
            )));
        }
    }
    let mut plan = plan_set(
        solution,
        &SetRequest {
            mashup: request.mashup.clone(),
            widget: found.id,
            properties: changes,
            style_from: None,
            styles: Vec::new(),
        },
    )?;
    if let Some(edit) = &mut plan.edit {
        edit.command = "mashup state-format set";
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_content_uses_scanned_text_and_names_invalid_contracts() {
        let bytes = br#"<StateDefinition><content><![CDATA[{"stateType":"str]]><![CDATA[ing","stateDefinitions":[]}]]></content></StateDefinition>"#;
        assert_eq!(
            read_definition(bytes).unwrap(),
            json!({"stateType":"string","stateDefinitions":[]})
        );
        for bytes in [
            b"<broken>".as_slice(),
            b"<StateDefinition/>",
            b"<content/>",
            b"<content><child/></content>",
            b"<content>invalid JSON</content>",
        ] {
            assert!(read_definition(bytes).is_err());
        }
        let valid = json!({"stateType":"fixed", "stateDefinitions":[{"defaultValue":"", "displayString":"Fallback", "defaultStyleDefinition":"Acme.Style"}]});
        assert_eq!(
            inline(&valid, "Acme.State", "field").unwrap()["StateDefinitionType"],
            "fixed"
        );
        for (key, value, message) in [
            ("stateType", json!("unexpected"), "stateType"),
            ("stateDefinitions", json!(null), "stateDefinitions"),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            let error = inline(&invalid, "Acme.State", "field")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(message) && error.contains("Acme.State"),
                "{error}"
            );
        }
        // A missing defaultValue is the final catch-all; see the test below.
        for key in ["displayString", "defaultStyleDefinition"] {
            let mut invalid = valid.clone();
            invalid["stateDefinitions"][0]
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(inline(&invalid, "Acme.State", "field")
                .unwrap_err()
                .to_string()
                .contains(key));
        }
        let mut invalid = valid;
        invalid["stateDefinitions"][0]["defaultValue"] = json!("25");
        assert!(inline(&invalid, "Acme.State", "field")
            .unwrap_err()
            .to_string()
            .contains("comparator"));
    }

    #[test]
    fn a_final_catch_all_without_default_value_is_how_composer_saves_it() {
        let definition = json!({"stateType":"string","stateDefinitions":[
            {"defaultValue":"Finished","displayString":"Finished","defaultStyleDefinition":"Acme.Good"},
            {"displayString":"Other","defaultStyleDefinition":"Acme.Other"}
        ]});
        let result = inline(&definition, "Acme.Status_ST", "Status").unwrap();
        assert_eq!(result["StateFormats"][0]["value"], "Finished");
        assert_eq!(result["StateFormats"][1]["defaultValue"], "");
        assert!(result["StateFormats"][1].get("comparator").is_none());
        let mut middle = definition.clone();
        middle["stateDefinitions"].as_array_mut().unwrap().reverse();
        assert!(inline(&middle, "Acme.Status_ST", "Status")
            .unwrap_err()
            .to_string()
            .contains("defaultValue"));
    }

    #[test]
    fn empty_string_equality_is_distinct_from_the_final_catch_all() {
        let definition = json!({"stateType":"string","stateDefinitions":[
            {"defaultValue":"","displayString":"Empty","defaultStyleDefinition":"Acme.Empty"},
            {"defaultValue":"","displayString":"Other","defaultStyleDefinition":"Acme.Other"}
        ]});
        let result = inline(&definition, "Acme.State", "field").unwrap();
        assert_eq!(result["StateFormats"][0]["comparator"], "==");
        assert_eq!(result["StateFormats"][0]["value"], "");
        assert!(result["StateFormats"][1].get("comparator").is_none());
        assert!(result["StateFormats"][1].get("value").is_none());
    }
}
