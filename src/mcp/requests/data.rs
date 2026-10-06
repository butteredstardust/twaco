use super::common::Absent;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DbRunRequest {
    /// A UTF-8 SQL file relative to the solution; give file or sql.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) file: Absent<String>,
    /// Inline SQL; give sql or file.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) sql: Absent<String>,
    /// Database Thing; otherwise resolve the one in the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) thing: Absent<String>,
    /// Prefix COMMIT; for statements such as CREATE DATABASE.
    #[serde(default)]
    pub(crate) no_transaction: bool,
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 1))]
    pub(crate) timeout: u64,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_timeout() -> u64 {
    120
}

fn default_true() -> bool {
    true
}

fn default_profile() -> String {
    "default".to_string()
}
