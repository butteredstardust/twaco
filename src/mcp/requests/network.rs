use super::common::{default_true, Absent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum NetworkAction {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "show")]
    Show,
}
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NetworkRequest {
    pub(crate) action: NetworkAction,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) network: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) top: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) depth: Absent<u64>,
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum NetworkWriteAction {
    #[serde(rename = "add")]
    Add,
    #[serde(rename = "move")]
    Move,
    #[serde(rename = "remove")]
    Remove,
    #[serde(rename = "import")]
    Import,
}
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NetworkWriteRequest {
    pub(crate) action: NetworkWriteAction,
    pub(crate) network: String,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) thing: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) under: Absent<String>,
    #[serde(default)]
    pub(crate) as_root: bool,
    #[serde(default, rename = "type", skip_serializing_if = "Absent::is_absent")]
    pub(crate) connection_type: Absent<String>,
    #[serde(default)]
    pub(crate) outside: bool,
    #[serde(default)]
    pub(crate) subtree: bool,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) file: Absent<String>,
    #[serde(default)]
    pub(crate) create_things: bool,
    #[serde(default)]
    pub(crate) r#move: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}
