use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PushRequest {
    pub(crate) entity: String,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    #[serde(default)]
    pub(crate) force: bool,
    /// With force, save the server's copy under .twaco/backups first (default true); false skips it.
    #[serde(default = "default_true")]
    pub(crate) backup: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_true() -> bool {
    true
}

fn default_profile() -> String {
    "default".to_string()
}
