//! Profile-specific mashup values applied only to copies sent to or compared with a server.

use super::config::Patch;
use serde_json::Value;

/// `bytes` with each of `patches` for this entity set in its `mashupContent`; the same bytes when
/// none applies or nothing changes.
pub(crate) fn apply(
    bytes: &[u8],
    collection: &str,
    name: &str,
    patches: &[Patch],
) -> Result<Vec<u8>, String> {
    let selected: Vec<&Patch> = patches
        .iter()
        .filter(|patch| patch.entity == format!("{collection}/{name}"))
        .collect();
    if selected.is_empty() {
        return Ok(bytes.to_vec());
    }
    if collection != "Mashups" {
        return Err(format!("patch for {collection}/{name} is not a Mashup"));
    }
    let assets = super::mashup::extract(bytes)
        .map_err(|error| format!("patch for Mashups/{name}: {error}"))?;
    let current: Value = serde_json::from_str(&assets.content).map_err(|error| {
        format!("patch for Mashups/{name}: mashupContent will not parse: {error}")
    })?;
    let mut content = current.clone();
    for patch in selected {
        set(&mut content, patch)
            .map_err(|error| format!("patch {} {}: {error}", patch.entity, patch.path))?;
    }
    if content == current {
        return Ok(bytes.to_vec());
    }
    super::mashup::replace_content(bytes, &content)
        .map_err(|error| format!("patch for Mashups/{name}: {error}"))
}

/// A copy pulled from the server with each patched path put back to the working copy's value, or
/// removed where the working copy has none, so a pull never writes one server's value into the
/// repository.
pub(crate) fn restore(
    bytes: &[u8],
    working: &[u8],
    collection: &str,
    name: &str,
    patches: &[Patch],
) -> Result<Vec<u8>, String> {
    let selected: Vec<&Patch> = patches
        .iter()
        .filter(|patch| patch.entity == format!("{collection}/{name}"))
        .collect();
    if selected.is_empty() {
        return Ok(bytes.to_vec());
    }
    let working_value = content(working, name)?;
    let current = content(bytes, name)?;
    let mut pulled = current.clone();
    for patch in selected {
        let value = get(&working_value, &patch.path);
        restore_path(&mut pulled, &patch.path, value)
            .map_err(|error| format!("patch {} {}: {error}", patch.entity, patch.path))?;
    }
    if pulled == current {
        return Ok(bytes.to_vec());
    }
    super::mashup::replace_content(bytes, &pulled)
        .map_err(|error| format!("patch for Mashups/{name}: {error}"))
}

fn content(bytes: &[u8], name: &str) -> Result<Value, String> {
    let assets = super::mashup::extract(bytes)
        .map_err(|error| format!("patch for Mashups/{name}: {error}"))?;
    serde_json::from_str(&assets.content).map_err(|error| error.to_string())
}

fn set(value: &mut Value, patch: &Patch) -> Result<(), String> {
    let parts: Vec<_> = patch.path.split('.').collect();
    let mut here = value;
    for key in &parts[..parts.len() - 1] {
        here = here
            .get_mut(*key)
            .ok_or_else(|| format!("missing object {key:?}"))?;
        if !here.is_object() {
            return Err(format!("missing object {key:?}"));
        }
    }
    let object = here
        .as_object_mut()
        .ok_or_else(|| "mashupContent is not an object".to_string())?;
    object.insert(
        parts.last().unwrap().to_string(),
        serde_json::to_value(&patch.value).map_err(|error| error.to_string())?,
    );
    Ok(())
}

fn get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |value, key| value.get(key))
}

fn restore_path(value: &mut Value, path: &str, wanted: Option<&Value>) -> Result<(), String> {
    let parts: Vec<_> = path.split('.').collect();
    let mut here = value;
    for key in &parts[..parts.len() - 1] {
        here = here
            .get_mut(*key)
            .ok_or_else(|| format!("missing object {key:?}"))?;
    }
    let object = here
        .as_object_mut()
        .ok_or_else(|| "missing parent object".to_string())?;
    match wanted {
        Some(value) => {
            object.insert(parts.last().unwrap().to_string(), value.clone());
        }
        None => {
            object.remove(*parts.last().unwrap());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(path: &str, value: toml::Value) -> Patch {
        Patch {
            entity: "Mashups/P.Main".into(),
            path: path.into(),
            value,
        }
    }

    fn document(content: &str) -> Vec<u8> {
        format!("<Entities><Mashups><Mashup name=\"P.Main\"><mashupContent><![CDATA[{content}]]></mashupContent></Mashup></Mashups></Entities>").into_bytes()
    }

    #[test]
    fn applies_a_leaf_and_keeps_unpatched_bytes_exact() {
        let bytes = document(r#"{"UI":{"Properties":{"Master":"P.Dev","Theme":"Base"}}}"#);
        let changed = apply(
            &bytes,
            "Mashups",
            "P.Main",
            &[patch(
                "UI.Properties.Master",
                toml::Value::String("P.Qa".into()),
            )],
        )
        .unwrap();
        let content = mashup_content(&changed);
        assert_eq!(content["UI"]["Properties"]["Master"], "P.Qa");
        assert_eq!(content["UI"]["Properties"]["Theme"], "Base");
        assert_eq!(
            apply(
                &bytes,
                "Mashups",
                "P.Other",
                &[patch(
                    "UI.Properties.Master",
                    toml::Value::String("P.Qa".into())
                )]
            )
            .unwrap(),
            bytes
        );
    }

    #[test]
    fn creates_a_leaf_but_refuses_a_missing_parent() {
        let bytes = document(r#"{"UI":{"Properties":{}}}"#);
        let changed = apply(
            &bytes,
            "Mashups",
            "P.Main",
            &[patch(
                "UI.Properties.Master",
                toml::Value::String("P.Qa".into()),
            )],
        )
        .unwrap();
        assert_eq!(
            mashup_content(&changed)["UI"]["Properties"]["Master"],
            "P.Qa"
        );
        let error = apply(
            &bytes,
            "Mashups",
            "P.Main",
            &[patch(
                "UI.Missing.Master",
                toml::Value::String("P.Qa".into()),
            )],
        )
        .unwrap_err();
        assert!(error.contains("missing object"), "{error}");
    }

    #[test]
    fn pull_restores_the_working_value() {
        let working = document(r#"{"UI":{"Properties":{"Master":"P.Dev"}}}"#);
        let server = document(r#"{"UI":{"Properties":{"Master":"P.Qa"}}}"#);
        let restored = restore(
            &server,
            &working,
            "Mashups",
            "P.Main",
            &[patch(
                "UI.Properties.Master",
                toml::Value::String("P.Qa".into()),
            )],
        )
        .unwrap();
        assert_eq!(
            mashup_content(&restored)["UI"]["Properties"]["Master"],
            "P.Dev"
        );
    }

    fn mashup_content(bytes: &[u8]) -> Value {
        serde_json::from_str(&super::super::mashup::extract(bytes).unwrap().content).unwrap()
    }
}
