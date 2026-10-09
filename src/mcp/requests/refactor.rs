use super::common::{default_true, Absent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdoptReportRequest {
    /// A designer's or backend collaborator's export, absolute or relative to the solution root.
    pub(crate) export: String,
    /// The collaborator's starting export, recorded handoff, or git revision.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) base: Absent<String>,
    /// Limit comparison to designer-owned UI or backend collections.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) only: Absent<AdoptKind>,
    /// Only entities whose name contains one of these.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entity: Absent<Vec<String>>,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdoptApplyRequest {
    /// A designer's or backend collaborator's export, absolute or relative to the solution root.
    pub(crate) export: String,
    /// The collaborator's starting export, recorded handoff, or git revision.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) base: Absent<String>,
    /// Limit comparison to designer-owned UI or backend collections.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) only: Absent<AdoptKind>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entity: Absent<Vec<String>>,
    /// Resolve named conflicts by taking the collaborator's or repository's version.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) take: Absent<Vec<AdoptTakeRequest>>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum AdoptKind {
    #[serde(rename = "ui")]
    Ui,
    #[serde(rename = "backend")]
    Backend,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdoptTakeRequest {
    pub(crate) side: AdoptTakeSide,
    /// An entity or Entity.Service.
    pub(crate) target: String,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum AdoptTakeSide {
    #[serde(rename = "theirs")]
    Theirs,
    #[serde(rename = "ours")]
    Ours,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum HandoffAction {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "record")]
    Record,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HandoffRequest {
    pub(crate) action: HandoffAction,
    /// Required for action record.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) name: Absent<String>,
    /// Export paths required for action record, absolute or relative to the solution root.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) files: Absent<Vec<String>>,
    /// Record only when false; record plans by default.
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum RenameKind {
    #[serde(rename = "entity")]
    Entity,
    #[serde(rename = "prefix")]
    Prefix,
    #[serde(rename = "field")]
    Field,
    #[serde(rename = "service")]
    Service,
    #[serde(rename = "param")]
    Param,
    #[serde(rename = "table")]
    Table,
    #[serde(rename = "property")]
    Property,
}

impl RenameKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Prefix => "prefix",
            Self::Field => "field",
            Self::Service => "service",
            Self::Param => "param",
            Self::Table => "table",
            Self::Property => "property",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenameRequest {
    pub(crate) kind: RenameKind,
    /// Entity full name; required for kind field, service or table.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) scope: Absent<String>,
    /// Service name; required for kind param and refused otherwise.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) service: Absent<String>,
    pub(crate) old: String,
    pub(crate) new: String,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    #[serde(default)]
    pub(crate) include_outside: bool,
    /// Write the database migration script when the rename touches DBConnection tables (into sql/ or sql_dir). Such a rename is refused unless sql or no_sql is given.
    #[serde(default)]
    pub(crate) sql: bool,
    /// Folder for the migration script, relative to the solution; implies sql.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) sql_dir: Absent<String>,
    /// The DBConnection tables are not in use: write no script.
    #[serde(default)]
    pub(crate) no_sql: bool,
    #[serde(default)]
    pub(crate) skip_checks: bool,
    /// The plan_digest a dry run returned. With dry_run false, the rename is refused (nothing written) unless the plan is still exactly that one.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) plan_digest: Absent<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum MoveMemberAction {
    #[serde(rename = "move")]
    Move,
    #[serde(rename = "copy")]
    Copy,
}

impl MoveMemberAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Copy => "copy",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum MoveMemberKind {
    #[serde(rename = "service")]
    Service,
    #[serde(rename = "property")]
    Property,
}

impl MoveMemberKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Property => "property",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MoveMemberRequest {
    pub(crate) action: MoveMemberAction,
    pub(crate) kind: MoveMemberKind,
    /// The entity the member is on.
    pub(crate) from: String,
    /// The entity it goes to.
    pub(crate) to: String,
    pub(crate) name: String,
    /// A new name on the target.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) new_name: Absent<String>,
    #[serde(default)]
    pub(crate) leave_delegate: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum NewBuildingBlockType {
    #[serde(rename = "standard")]
    Standard,
    #[serde(rename = "abstract")]
    Abstract,
    #[serde(rename = "implementation")]
    Implementation,
}

impl NewBuildingBlockType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Abstract => "abstract",
            Self::Implementation => "implementation",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewBuildingBlockRequest {
    /// The block's name with its namespace, such as Acme.Orders.
    pub(crate) name: String,
    #[serde(default = "default_new_building_block_type")]
    pub(crate) r#type: NewBuildingBlockType,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) display_name: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) description: Absent<String>,
    /// The abstract block an implementation implements.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) parent: Absent<String>,
    #[serde(default)]
    pub(crate) model_logic: bool,
    /// An implementation may leave out its Management_TS.
    #[serde(default = "default_true")]
    pub(crate) management_shape: bool,
    /// The project's folder, relative to the solution; default the block's name.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) root: Absent<String>,
    /// PTC.Base:<version>; default what another project of the solution declares.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) base_extension: Absent<String>,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}

fn default_new_building_block_type() -> NewBuildingBlockType {
    NewBuildingBlockType::Standard
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetemplateRequest {
    /// The Thing or template to change.
    pub(crate) entity: String,
    /// The new template (a Thing) or base template (a template).
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) template: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) add_shapes: Absent<Vec<String>>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) remove_shapes: Absent<Vec<String>>,
    #[serde(default)]
    pub(crate) accept_loss: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}
