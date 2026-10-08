use super::common::{default_profile, Absent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DoctorRequest {
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum SettingsAction {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "show")]
    Show,
    #[serde(rename = "search")]
    Search,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettingsRequest {
    #[serde(default = "default_settings_action")]
    pub(crate) action: SettingsAction,
    /// show: a subsystem, such as Logging or LoggingSubsystem.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) subsystem: Absent<String>,
    /// show: only this table of the subsystem, such as Settings (any case).
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) table: Absent<String>,
    /// search: part of a setting's name or description.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) text: Absent<String>,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_settings_action() -> SettingsAction {
    SettingsAction::List
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogRequest {
    /// One entity, full name or its last dotted segment.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entity: Absent<String>,
    /// Narrow to one project of the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    /// Case-insensitive match over service names, parameter names and descriptions; use entity to narrow to one entity.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) text: Absent<String>,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum ImpactMinConfidence {
    #[serde(rename = "structural")]
    Structural,
    #[serde(rename = "resolved")]
    Resolved,
    #[serde(rename = "review")]
    Review,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum ImpactFormat {
    #[serde(rename = "json")]
    Json,
    #[serde(rename = "dot")]
    Dot,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImpactRequest {
    /// Collection/Name, a full name, or its last dotted segment.
    pub(crate) entity: String,
    /// A service, property or field of the entity: ask only about what names it.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) member: Absent<String>,
    /// The weakest reference to follow.
    #[serde(default = "default_impact_min_confidence")]
    pub(crate) min_confidence: ImpactMinConfidence,
    /// How many references away to look; omit for no limit.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    #[schemars(range(min = 1))]
    pub(crate) depth: Absent<u64>,
    /// dot returns the dependents as a Graphviz graph in `dot`.
    #[serde(default = "default_impact_format")]
    pub(crate) format: ImpactFormat,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

fn default_impact_min_confidence() -> ImpactMinConfidence {
    ImpactMinConfidence::Review
}

fn default_impact_format() -> ImpactFormat {
    ImpactFormat::Json
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum UnusedMinConfidence {
    #[serde(rename = "structural")]
    Structural,
    #[serde(rename = "resolved")]
    Resolved,
    #[serde(rename = "review")]
    Review,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum UnusedCollection {
    #[serde(rename = "Things")]
    Things,
    #[serde(rename = "ThingTemplates")]
    ThingTemplates,
    #[serde(rename = "ThingShapes")]
    ThingShapes,
    #[serde(rename = "DataShapes")]
    DataShapes,
}

impl UnusedCollection {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Things => "Things",
            Self::ThingTemplates => "ThingTemplates",
            Self::ThingShapes => "ThingShapes",
            Self::DataShapes => "DataShapes",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnusedRequest {
    /// The weakest reference that counts as use; a stronger minimum reports more entities.
    #[serde(default = "default_unused_min_confidence")]
    pub(crate) min_confidence: UnusedMinConfidence,
    /// Judge one collection only.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) collection: Absent<UnusedCollection>,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

fn default_unused_min_confidence() -> UnusedMinConfidence {
    UnusedMinConfidence::Review
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocsRequest {
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum GuideAction {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "search")]
    Search,
    #[serde(rename = "read")]
    Read,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GuideRequest {
    /// Default: search when text is given, read when a topic is, else list (as `twaco guide`).
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) action: Absent<GuideAction>,
    /// search: words, such as "AddMember group" or "configuration table row replaced".
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) text: Absent<String>,
    /// read: a topic id from list or search, such as quirks or docs/DEVELOPER_GUIDE.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) topic: Absent<String>,
    /// read: a section heading, or a part of it that only one heading has.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) section: Absent<String>,
    #[serde(default = "default_guide_limit")]
    #[schemars(range(min = 1))]
    pub(crate) limit: u64,
}

fn default_guide_limit() -> u64 {
    8
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelpSearchRequest {
    /// Words, as in the help's own search box. Code such as Resources["InfoTableFunctions"] is split into its words.
    pub(crate) query: String,
    #[serde(default = "default_help_search_limit")]
    #[schemars(range(min = 1))]
    pub(crate) limit: u64,
    /// A help release such as 10.1; default: the server's own.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) version: Absent<String>,
    /// Download the index again instead of using the cache.
    #[serde(default)]
    pub(crate) refresh: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_help_search_limit() -> u64 {
    10
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelpPageRequest {
    /// A path such as ThingWorx/Help/Composer/Things/ThingServices/QueryParameterforQueryServices.html, or its address.
    pub(crate) page: String,
    /// Only the part under the first heading containing this text.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) section: Absent<String>,
    #[serde(default = "default_help_page_max_chars")]
    #[schemars(range(min = 1000))]
    pub(crate) max_chars: u64,
    /// A help release such as 10.1; default: the one in the address, else the server's own.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) version: Absent<String>,
    #[serde(default)]
    pub(crate) refresh: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_help_page_max_chars() -> u64 {
    20000
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum JavadocAction {
    #[serde(rename = "search")]
    Search,
    #[serde(rename = "class")]
    Class,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JavadocRequest {
    pub(crate) action: JavadocAction,
    /// A search name, or a simple/qualified class name.
    pub(crate) name: String,
    /// class: return full details for every overload of this member.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) member: Absent<String>,
    #[serde(default = "default_javadoc_limit")]
    #[schemars(range(min = 1))]
    pub(crate) limit: u64,
    #[serde(default)]
    pub(crate) refresh: bool,
}

fn default_javadoc_limit() -> u64 {
    10
}
