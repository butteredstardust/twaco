use super::{compile::Artifacts, ids::Pins};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Baseline {
    pub format: u32,
    pub compiler: String,
    pub rules: String,
    #[serde(default)]
    pub id_policy: String,
    pub ids: BTreeMap<String, String>,
    /// Widget ids an adopted mashup keeps, as `[node, role, id]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_widgets: Vec<(String, String, String)>,
    /// Binding and event ids an adopted mashup keeps, by the id generated for them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pinned_wiring: BTreeMap<String, String>,
    pub ownership: Vec<String>,
    pub content: Value,
    /// Expected content including values retained at adoption, never subsequent hand edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Value>,
    pub css: String,
    /// Digest of the content as last written, so a later build can tell a fresh hand edit
    /// from differences the mashup already had when twaco last wrote it (an adoption keeps
    /// what the spec cannot say).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub written: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub generated_cells: BTreeMap<String, BTreeMap<String, String>>,
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
            id_policy: super::ids::VERSION.into(),
            ids: artifacts.ids.clone(),
            pinned_widgets: artifacts.pins.widgets.clone(),
            pinned_wiring: artifacts.pins.wiring.clone(),
            ownership,
            content,
            comparison: None,
            css: artifacts.css.clone(),
            written: String::new(),
            generated_cells: BTreeMap::new(),
            managed_xml: [
                ("name".into(), name.into()),
                ("projectName".into(), project.into()),
            ]
            .into(),
        }
    }

    /// The digest `written` records for a content value.
    pub fn digest(content: &Value) -> String {
        crate::core::repo::sha256_hex(&serde_json::to_vec(content).expect("JSON value"))
    }

    pub(super) fn content_edits(&self, current: &Value) -> Vec<String> {
        if !self.written.is_empty() && self.written == Self::digest(current) {
            return vec![];
        }
        let expected = self.comparison.as_ref().unwrap_or(&self.content);
        let mut report = super::merge::Report::default();
        super::merge::content(expected, current, expected, &mut report);
        report.kept
    }

    pub(super) fn css_matches(&self, current: &Value, css: &str) -> bool {
        let written = !self.written.is_empty() && self.written == Self::digest(current);
        let expected = if written {
            Some(current)
        } else {
            self.comparison.as_ref()
        };
        let expected = expected
            .and_then(|v| v["CustomMashupCss"].as_str())
            .unwrap_or(&self.css);
        crate::core::mashup::same_css(expected, css)
    }

    pub fn pins(&self) -> Pins {
        Pins {
            widgets: self.pinned_widgets.clone(),
            wiring: self.pinned_wiring.clone(),
        }
    }

    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        let baseline: Self =
            serde_json::from_slice(bytes).map_err(|e| format!(".page-baseline.json: {e}"))?;
        if baseline.format != 1 || baseline.compiler != "page-v1" || baseline.rules != "v1" {
            return Err(".page-baseline.json: unsupported format/compiler/rule version; use a compatible twaco".into());
        }
        if baseline.id_policy != super::ids::VERSION {
            return Err(".page-baseline.json: unsupported id policy; use the twaco version that generated this baseline".into());
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
