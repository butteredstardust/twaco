use super::compile::Artifacts;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Baseline {
    pub format: u32,
    pub compiler: String,
    pub rules: String,
    pub ids: BTreeMap<String, String>,
    pub ownership: Vec<String>,
    pub content: Value,
    pub css: String,
    pub managed_xml: BTreeMap<String, String>,
}

impl Baseline {
    pub fn generated(artifacts: &Artifacts, name: &str, project: &str) -> Self {
        let content: Value = serde_json::from_str(&artifacts.content).expect("compiled JSON");
        let mut ownership = Vec::new();
        fn paths(value: &Value, path: &str, out: &mut Vec<String>) {
            match value {
                Value::Object(map) if !map.is_empty() => {
                    for (key, value) in map {
                        paths(
                            value,
                            &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                            out,
                        );
                    }
                }
                Value::Array(items) if !items.is_empty() => {
                    for (index, value) in items.iter().enumerate() {
                        let key = super::merge::key(value).unwrap_or_else(|| index.to_string());
                        paths(value, &format!("{path}/{key}"), out);
                    }
                }
                _ => out.push(path.into()),
            }
        }
        paths(&content, "", &mut ownership);
        Self {
            format: 1,
            compiler: "page-v1".into(),
            rules: "v1".into(),
            ids: artifacts.ids.clone(),
            ownership,
            content,
            css: artifacts.css.clone(),
            managed_xml: [
                ("name".into(), name.into()),
                ("projectName".into(), project.into()),
            ]
            .into(),
        }
    }

    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        let baseline: Self =
            serde_json::from_slice(bytes).map_err(|e| format!(".page-baseline.json: {e}"))?;
        if baseline.format != 1 || baseline.compiler != "page-v1" || baseline.rules != "v1" {
            return Err(".page-baseline.json: unsupported format/compiler/rule version; use a compatible twaco".into());
        }
        crate::core::mashup::edit::parse(&crate::core::mashup::to_sidecar(&baseline.content))
            .map_err(|e| format!(".page-baseline.json: invalid generated content: {e}"))?;
        Ok(baseline)
    }

    pub fn bytes(&self) -> Vec<u8> {
        format!(
            "{}\n",
            serde_json::to_string_pretty(self).expect("baseline serialises")
        )
        .into_bytes()
    }
}
