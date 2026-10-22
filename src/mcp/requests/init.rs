use super::common::default_profile;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InitFromServerRequest {
    /// Server project names to create or add; at least one is required.
    #[schemars(length(min = 1))]
    pub(crate) projects: Vec<String>,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Write the planned files. The default only returns the plan.
    #[serde(default = "default_false")]
    pub(crate) apply: bool,
}

fn default_false() -> bool {
    false
}
