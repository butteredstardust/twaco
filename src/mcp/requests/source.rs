use super::common::{default_profile, default_true, Absent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum TypesAction {
    #[serde(rename = "generate")]
    Generate,
    #[serde(rename = "check")]
    Check,
    #[serde(rename = "platform")]
    Platform,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TypesRequest {
    #[serde(default = "default_types_action")]
    pub(crate) action: TypesAction,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

fn default_types_action() -> TypesAction {
    TypesAction::Generate
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckRequest {
    /// Default: the solution's [gates] live.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) live: Absent<bool>,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SyncRequest {
    /// One entity, full name or its last dotted segment. Name one, or pass all: true.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entity: Absent<String>,
    /// Every entity (of the project, if one is named).
    #[serde(default)]
    pub(crate) all: bool,
    /// Narrow to one project of the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    #[serde(default)]
    pub(crate) check: bool,
    /// Permit a service or field to appear or disappear.
    #[serde(default)]
    pub(crate) allow_add_remove: bool,
    /// Rewrite every script payload in the configured layout.
    #[serde(default)]
    pub(crate) relayout: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtractRequest {
    /// One entity, full name or its last dotted segment. Name one, or pass all: true.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entity: Absent<String>,
    /// Every entity (of the project, if one is named).
    #[serde(default)]
    pub(crate) all: bool,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FmtRequest {
    #[serde(default)]
    pub(crate) check: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeployRequest {
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Overwrite entities the server changed since the last sync.
    #[serde(default)]
    pub(crate) force: bool,
    /// With force, save the server's copy of each overwritten entity under .twaco/backups first (default true).
    #[serde(default = "default_true")]
    pub(crate) backup: bool,
    /// Only these entities; post-import services are skipped.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) only: Absent<Vec<String>>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) only_projects: Absent<Vec<String>>,
    #[serde(default)]
    pub(crate) backend_only: bool,
    /// Skip the offline gates. The server's script parse still runs.
    #[serde(default)]
    pub(crate) skip_checks: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}
