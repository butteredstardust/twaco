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
            let key = text
                .strip_prefix("${profile:")
                .and_then(|rest| rest.strip_suffix('}'));
            if let Some(key) = key {
                let value = profile
                    .value(key)
                    .ok_or_else(|| DeployError::UnknownPlaceholder {
                        project: project.to_string(),
                        key: key.to_string(),
                    })?;
                Ok(serde_json::to_value(value).expect("TOML values serialize as JSON"))
            } else {
                Ok(value.clone())
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
        let json = serde_json::to_value(value).expect("TOML values serialize as JSON");
        let mut renderings = vec![json.to_string()];
        if let Value::String(text) = &json {
            renderings.push(text.clone());
        }
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
            if let Some(key) = text
                .strip_prefix("${profile:")
                .and_then(|rest| rest.strip_suffix('}'))
            {
                keys.push(key.to_string());
            }
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
