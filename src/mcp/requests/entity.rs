use super::common::{default_profile, default_true, Absent};
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

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StatusRequest {
    /// One entity name, full or its last dotted segment. Name one, or pass all: true.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entity: Absent<String>,
    /// Every entity (of the project, if one is named).
    #[serde(default)]
    pub(crate) all: bool,
    /// Narrow to one project of the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    /// Record a baseline for every entity whose two sides agree. Writes .twaco/baseline.json under the workspace lock.
    #[serde(default)]
    pub(crate) record: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntityDeleteRequest {
    /// Entities to delete; may be empty when renamed is true.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entities: Absent<Vec<String>>,
    #[serde(default)]
    pub(crate) renamed: bool,
    #[serde(default)]
    pub(crate) allow_repository_defined: bool,
    #[serde(default)]
    pub(crate) allow_outside_dependents: bool,
    #[serde(default)]
    pub(crate) allow_file_repository_data_loss: bool,
    /// Deprecated: means allow_repository_defined and allow_outside_dependents; never accepts FileRepository data loss.
    #[serde(default)]
    pub(crate) force: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Save the server's copy of every entity under .twaco/backups before deleting (default true); a failed backup deletes nothing.
    #[serde(default = "default_true")]
    pub(crate) backup: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntityRestoreRequest {
    /// A set id from the listing; omit to list sets.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) set: Absent<String>,
    /// Only these (Collection/Name or a bare name).
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entities: Absent<Vec<String>>,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntityCarryRequest {
    /// Collection/Old, Collection/New, in pairs; may be empty when renamed is true.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) pairs: Absent<Vec<String>>,
    #[serde(default)]
    pub(crate) renamed: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Also ask the platform for its own difference count.
    #[serde(default)]
    pub(crate) detail: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PermissionsRequest {
    /// Entity names, full or their last dotted segment. Name some, or pass all: true.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entities: Absent<Vec<String>>,
    /// Every entity (of the project, if one is named).
    #[serde(default)]
    pub(crate) all: bool,
    /// Narrow to one project of the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PermissionsAuditRequest {
    /// Narrow to one project of the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    /// Also compare the server (read-only): entity permissions, helper tables, platform grants,
    /// memberships and role units.
    #[serde(default)]
    pub(crate) server: bool,
    /// Server profile name, with server.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Every grant behind each finding.
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PermissionsInitRequest {
    /// Narrow to one project of the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    /// Take the grants from the permission helper's tables, not the entity XML.
    #[serde(default)]
    pub(crate) from_helper: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PermissionsApplyRequest {
    /// Narrow to one project of the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Every grant each file gains or loses.
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PermissionsPushRequest {
    /// Entity names, full or their last dotted segment. Name some, or pass all: true.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entities: Absent<Vec<String>>,
    /// Every entity (of the project, if one is named).
    #[serde(default)]
    pub(crate) all: bool,
    /// Instead of entities, add the policies' [[platform]] grants and memberships the server
    /// lacks; nothing is removed.
    #[serde(default)]
    pub(crate) platform: bool,
    /// Narrow to one project of the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}
