use super::super::profile::Profile;
use super::{DeployError, ProjectBundle, ServiceCall};
use serde_json::Value;

pub(super) fn resolve_call(
    call: &ServiceCall,
    profile: &Profile,
    project: &str,
) -> Result<ServiceCall, DeployError> {
    Ok(ServiceCall {
        target: call.target.clone(),
        service: call.service.clone(),
        parameters: resolve_value(&call.parameters, profile, project)?,
    })
}

fn resolve_value(value: &Value, profile: &Profile, project: &str) -> Result<Value, DeployError> {
    match value {
        Value::String(text) => {
            let lookup = |key: &str| {
                profile
                    .value(key)
                    .ok_or_else(|| DeployError::UnknownPlaceholder {
                        project: project.to_string(),
                        key: key.to_string(),
                    })
            };
            let found = placeholders(text);
            match found.as_slice() {
                [] => Ok(value.clone()),
                // The whole string is one placeholder: the value keeps its type.
                [(range, key)] if range.start == 0 && range.end == text.len() => {
                    Ok(serde_json::to_value(lookup(key)?).expect("TOML values serialize as JSON"))
                }
                _ => {
                    let mut resolved = String::with_capacity(text.len());
                    let mut copied = 0;
                    for (range, key) in &found {
                        resolved.push_str(&text[copied..range.start]);
                        let value = lookup(key)?;
                        let embedded = embedded_text(&value).ok_or_else(|| {
                            DeployError::PlaceholderNotText {
                                project: project.to_string(),
                                key: key.to_string(),
                            }
                        })?;
                        resolved.push_str(&embedded);
                        copied = range.end;
                    }
                    resolved.push_str(&text[copied..]);
                    Ok(Value::String(resolved))
                }
            }
        }
        Value::Array(values) => values
            .iter()
            .map(|value| resolve_value(value, profile, project))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), resolve_value(value, profile, project)?)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Value::Object),
        _ => Ok(value.clone()),
    }
}

pub(super) fn redact_placeholder_values(
    message: &str,
    profile: &Profile,
    projects: &[ProjectBundle],
) -> String {
    let mut keys = Vec::new();
    for call in projects
        .iter()
        .flat_map(|project| project.deploy.iter().chain(&project.post_import))
    {
        collect_placeholder_keys(&call.parameters, &mut keys);
    }
    keys.sort();
    keys.dedup();
    keys.into_iter().fold(message.to_string(), |redacted, key| {
        let Some(value) = profile.value(&key) else {
            return redacted;
        };
        let json = serde_json::to_value(&value).expect("TOML values serialize as JSON");
        let mut renderings = vec![json.to_string()];
        if let Value::String(text) = &json {
            renderings.push(text.clone());
        }
        // What a placeholder inside a longer string put there.
        renderings.extend(embedded_text(&value));
        renderings.sort_by_key(|value| std::cmp::Reverse(value.len()));
        renderings.dedup();
        renderings.into_iter().fold(redacted, |text, rendered| {
            if rendered.is_empty() {
                text
            } else {
                text.replace(&rendered, &format!("${{profile:{key}}}"))
            }
        })
    })
}

fn collect_placeholder_keys(value: &Value, keys: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            keys.extend(
                placeholders(text)
                    .into_iter()
                    .map(|(_, key)| key.to_string()),
            );
        }
        Value::Array(values) => {
            for value in values {
                collect_placeholder_keys(value, keys);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_placeholder_keys(value, keys);
            }
        }
        _ => {}
    }
}

/// The text a value takes inside a longer string: a string as it is, a number, boolean or date
/// as written. An array or a table has no one text form, so it cannot be embedded.
fn embedded_text(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(text) => Some(text.clone()),
        toml::Value::Integer(_)
        | toml::Value::Float(_)
        | toml::Value::Boolean(_)
        | toml::Value::Datetime(_) => Some(value.to_string()),
        toml::Value::Array(_) | toml::Value::Table(_) => None,
    }
}

/// Every `${profile:key}` in `text`, anywhere in it, with its byte range. An opening
/// `${profile:` with no closing `}` is not a placeholder and stays as it is.
fn placeholders(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    const OPEN: &str = "${profile:";
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find(OPEN) {
        let start = from + offset;
        let key_start = start + OPEN.len();
        let Some(length) = text[key_start..].find('}') else {
            break;
        };
        let end = key_start + length + 1;
        found.push((start..end, &text[key_start..end - 1]));
        from = end;
    }
    found
}
