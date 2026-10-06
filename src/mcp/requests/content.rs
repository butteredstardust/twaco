use super::common::Absent;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RepoAction {
    List,
    Ls,
    Get,
    Status,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepoRequest {
    #[serde(default = "default_action")]
    pub(crate) action: RepoAction,
    /// A FileRepository Thing, such as SystemRepository. Not for list.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) repository: Absent<String>,
    /// ls: a folder (default /); get: a file, such as Thumbnails/a.png.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) path: Absent<String>,
    #[serde(default)]
    pub(crate) recursive: bool,
    /// get: the most text returned.
    #[serde(default = "default_max_chars")]
    #[schemars(range(min = 1000))]
    pub(crate) max_chars: u64,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

fn default_action() -> RepoAction {
    RepoAction::List
}

fn default_max_chars() -> u64 {
    100_000
}

fn default_profile() -> String {
    "default".to_string()
}
