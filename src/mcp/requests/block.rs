use super::common::{default_profile, default_true, Absent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum BlockAction {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "show")]
    Show,
}

/// `block`: the solution's building blocks, or one of them.
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BlockRequest {
    pub(crate) action: BlockAction,
    /// For show: the block (its project), or the end of its name.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) block: Absent<String>,
    /// Add the framework's view from the server: version, state, deploy order and, for show,
    /// the parameters DeployComponent takes.
    #[serde(default)]
    pub(crate) server: bool,
    /// The server profile, with server true.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum BlockBump {
    #[serde(rename = "patch")]
    Patch,
    #[serde(rename = "minor")]
    Minor,
    #[serde(rename = "major")]
    Major,
}

/// `block_version`: raise or set a block's version.
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BlockVersionRequest {
    /// The block (its project), or the end of its name.
    pub(crate) block: String,
    /// Raise the version; give this or set.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) bump: Absent<BlockBump>,
    /// The version to write, major.minor.patch; give this or bump.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) set: Absent<String>,
    /// Plan only when true (the default); false writes both files.
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}
