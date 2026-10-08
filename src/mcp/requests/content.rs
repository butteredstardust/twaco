use super::common::{default_profile, default_true, Absent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum RepoAction {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "ls")]
    Ls,
    #[serde(rename = "get")]
    Get,
    #[serde(rename = "status")]
    Status,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepoRequest {
    #[serde(default = "default_repo_action")]
    pub(crate) action: RepoAction,
    /// A FileRepository Thing, such as SystemRepository. Not for list.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) repository: Absent<String>,
    /// ls: a folder (default /); get: a file, such as Thumbnails/a.png.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) path: Absent<String>,
    #[serde(default)]
    pub(crate) recursive: bool,
    /// get: the most text returned.
    #[serde(default = "default_repo_max_chars")]
    #[schemars(range(min = 1000))]
    pub(crate) max_chars: u64,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

fn default_repo_action() -> RepoAction {
    RepoAction::List
}

fn default_repo_max_chars() -> u64 {
    100000
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum RepoWriteAction {
    #[serde(rename = "put")]
    Put,
    #[serde(rename = "mkdir")]
    Mkdir,
    #[serde(rename = "rm")]
    Rm,
    #[serde(rename = "mv")]
    Mv,
    #[serde(rename = "push")]
    Push,
    #[serde(rename = "pull")]
    Pull,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepoWriteRequest {
    /// push/pull: the whole tree filerepository/<repo>/ to or from the server; neither deletes.
    pub(crate) action: RepoWriteAction,
    pub(crate) repository: String,
    /// put/mkdir/rm: the repository path; mv: the source.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) path: Absent<String>,
    /// mv: the file's new path.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) to: Absent<String>,
    /// put: the content, as UTF-8 text.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) text: Absent<String>,
    /// put: a file of the solution to upload instead, relative to the solution root.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) local: Absent<String>,
    #[serde(default)]
    pub(crate) overwrite: bool,
    #[serde(default)]
    pub(crate) recursive: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum ExtensionsAction {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "show")]
    Show,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtensionsRequest {
    #[serde(default = "default_extensions_action")]
    pub(crate) action: ExtensionsAction,
    /// show: the package name.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) package: Absent<String>,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_extensions_action() -> ExtensionsAction {
    ExtensionsAction::List
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum ExtensionWriteAction {
    #[serde(rename = "import")]
    Import,
    #[serde(rename = "remove")]
    Remove,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtensionWriteRequest {
    pub(crate) action: ExtensionWriteAction,
    /// import: the package zip, relative to the solution root.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) zip: Absent<String>,
    /// remove: the package name.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) package: Absent<String>,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum ExportAction {
    #[serde(rename = "entity")]
    Entity,
    #[serde(rename = "collection")]
    Collection,
    #[serde(rename = "project")]
    Project,
    #[serde(rename = "source_control")]
    SourceControl,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchRequest {
    /// Text found anywhere in an entity's name or description, in any case; with a * it is a pattern instead (*_DS). Absent: every entity.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) text: Absent<String>,
    /// Entity types, singular or as collections (Thing, ThingTemplates, Mashup, ...). An unknown one is refused.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) types: Absent<Vec<String>>,
    /// Only this project's entities.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1))]
    pub(crate) limit: u64,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_search_limit() -> u64 {
    100
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntityGetRequest {
    /// Collection/Name for any entity on the server (Things/My.Thing, Resources/EntityServices), or a bare name for one in the repository.
    pub(crate) entity: String,
    /// The most XML characters returned; a longer entity is cut, and export writes it whole to a file.
    #[serde(default = "default_entity_get_max_chars")]
    #[schemars(range(min = 1))]
    pub(crate) max_chars: u64,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_entity_get_max_chars() -> u64 {
    200_000
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExportRequest {
    pub(crate) action: ExportAction,
    /// entity: Collection/Name, such as Things/My.Thing.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) entity: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) collection: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    /// entity/collection/project: the file to write, relative to the solution root.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) out: Absent<String>,
    #[serde(default)]
    pub(crate) overwrite: bool,
    /// source_control: the file repository.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) repository: Absent<String>,
    /// source_control: the folder in it.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) path: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) tags: Absent<String>,
    /// source_control: write a zip of this name instead of a folder.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) zip: Absent<String>,
    #[serde(default)]
    pub(crate) with_dependents: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum PackageAction {
    #[serde(rename = "bundle")]
    Bundle,
    #[serde(rename = "source_control")]
    SourceControl,
    #[serde(rename = "extension")]
    Extension,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum PackagePart {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "backend")]
    Backend,
    #[serde(rename = "frontend")]
    Frontend,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackageRequest {
    pub(crate) action: PackageAction,
    /// Only this project; the whole solution when omitted.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    /// bundle: which collections.
    #[serde(default = "default_package_part")]
    pub(crate) part: PackagePart,
    /// extension: whether the entities stay editable once installed.
    #[serde(default)]
    pub(crate) editable: bool,
    /// The file to write, relative to the solution root.
    pub(crate) out: String,
    #[serde(default)]
    pub(crate) overwrite: bool,
}

fn default_package_part() -> PackagePart {
    PackagePart::All
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum ImportAction {
    #[serde(rename = "file")]
    File,
    #[serde(rename = "source_control")]
    SourceControl,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportRequest {
    pub(crate) action: ImportAction,
    /// file: relative to the solution root.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) file: Absent<String>,
    /// source_control: the file repository.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) repository: Absent<String>,
    /// source_control: the tree's folder in it.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) path: Absent<String>,
    #[serde(default)]
    pub(crate) overwrite_properties: bool,
    #[serde(default)]
    pub(crate) overwrite_tables: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}
