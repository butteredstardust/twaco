use super::common::{default_profile, default_true};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InitRequest {
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckRequest {
    #[serde(default)]
    pub(crate) server: bool,
    #[serde(default)]
    pub(crate) detail: bool,
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}
