use super::*;

pub(crate) fn validate_arguments(schema: &Value, arguments: &Value) -> Result<(), String> {
    if !arguments.is_object() {
        return Err("`arguments` must be a JSON object".to_string());
    }
    validate_value(schema, arguments, "")
}

/// Validate one value against the small JSON Schema subset advertised by MCP tools.
fn validate_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let fits = match schema["type"].as_str() {
        Some("string") => value.is_string(),
        Some("boolean") => value.is_boolean(),
        Some("integer") => value
            .as_u64()
            .is_some_and(|n| schema["minimum"].as_u64().is_none_or(|min| n >= min)),
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        _ => true,
    };
    if !fits {
        let wanted = match schema["type"].as_str() {
            Some("array") if array_of_strings(schema) => "an array of strings".to_string(),
            Some("array") => "an array".to_string(),
            Some("integer") => format!(
                "an integer of at least {}",
                schema["minimum"].as_u64().unwrap_or(0)
            ),
            Some(other) => format!("a {other}"),
            None => "something else".to_string(),
        };
        return Err(format!("`{path}` must be {wanted}, not {value}"));
    }
    if let Some(allowed) = schema["enum"].as_array() {
        if !allowed.contains(value) {
            return Err(format!(
                "`{path}` must be one of {}, not {value}",
                Value::Array(allowed.clone())
            ));
        }
    }
    if let Some(given) = value.as_object() {
        let empty = Map::new();
        let properties = schema["properties"].as_object().unwrap_or(&empty);
        for name in schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !given.contains_key(name) {
                let path = property_path(path, name);
                return Err(format!("`{path}` is required"));
            }
        }
        for (name, value) in given {
            let name_path = property_path(path, name);
            match properties.get(name) {
                Some(property) => validate_value(property, value, &name_path)?,
                None if schema["additionalProperties"] == false => {
                    if path.is_empty() {
                        let mut known: Vec<&String> = properties.keys().collect();
                        known.sort();
                        let known: Vec<&str> = known.into_iter().map(String::as_str).collect();
                        return Err(format!(
                            "this tool takes no argument `{name}` (it takes: {})",
                            known.join(", ")
                        ));
                    }
                    return Err(format!("`{name_path}` is not allowed"));
                }
                None if schema["additionalProperties"].is_object() => {
                    validate_value(&schema["additionalProperties"], value, &name_path)?
                }
                None => {}
            }
        }
    }
    if let Some(items) = value.as_array() {
        if schema["items"].is_object() {
            for (index, value) in items.iter().enumerate() {
                validate_value(&schema["items"], value, &format!("{path}[{index}]"))?;
            }
        }
    }
    Ok(())
}

fn property_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}

fn array_of_strings(schema: &Value) -> bool {
    schema["items"].as_object().is_some_and(|items| {
        items.len() == 1 && items.get("type").and_then(Value::as_str) == Some("string")
    })
}
