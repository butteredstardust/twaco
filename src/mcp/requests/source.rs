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
    pub action: TypesAction,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub profile: String,
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
    /// One entity, full name or its last dotted segment. Name one or more (entity, entities), or pass all: true.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entity: Absent<String>,
    /// Several entities, each as entity takes one.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entities: Absent<Vec<String>>,
    /// Every entity (of the project, if one is named).
    #[serde(default)]
    pub(crate) all: bool,
    /// Narrow to one project of the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    #[serde(default)]
    pub(crate) check: bool,
    /// Add the services and fields only the sidecars have, and remove those only the entity has.
    #[serde(default)]
    pub(crate) allow_add_remove: bool,
    /// Rewrite every script payload in the configured layout.
    #[serde(default)]
    pub relayout: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtractRequest {
    /// One entity, full name or its last dotted segment. Name one or more (entity, entities), or pass all: true.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entity: Absent<String>,
    /// Several entities, each as entity takes one.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entities: Absent<Vec<String>>,
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
    pub force: bool,
    /// With force, save the server's copy of each overwritten entity under .twaco/backups first (default true).
    #[serde(default = "default_true")]
    pub backup: bool,
    /// Only these entities; post-import services are skipped.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub only: Absent<Vec<String>>,
    /// Select entities whose source file or sidecars changed since this Git revision.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub changed_since: Absent<String>,
    /// Remove matching Collection/Name selectors from the selected entities.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub exclude: Absent<Vec<String>>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub only_projects: Absent<Vec<String>>,
    #[serde(default)]
    pub backend_only: bool,
    /// Skip the gates, the server requirements check among them. The server's script parse
    /// during the deploy still runs.
    #[serde(default)]
    pub skip_checks: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}
