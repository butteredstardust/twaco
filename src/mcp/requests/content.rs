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
    pub action: RepoAction,
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
    pub profile: String,
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
pub(crate) enum LocalizationWriteAction {
    #[serde(rename = "pull")]
    Pull,
    #[serde(rename = "push")]
    Push,
    #[serde(rename = "new")]
    New,
    #[serde(rename = "set")]
    Set,
    #[serde(rename = "remove")]
    Remove,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalizationRequest {
    /// Only one localization table, by its table name.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) table: Absent<String>,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Include each differing token and problem rather than only counts.
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalizationWriteRequest {
    pub(crate) action: LocalizationWriteAction,
    /// new: table name; set/remove: optionally limit the action to this table.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) table: Absent<String>,
    /// set/remove: the localization token name.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) token: Absent<String>,
    /// set: the localized text.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) value: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) usage: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) context: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    #[serde(default)]
    pub(crate) prune: bool,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) language_common: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) language_native: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) description: Absent<String>,
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    #[serde(default)]
    pub(crate) detail: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsersRequest {
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub profile: String,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum UsersWriteAction {
    #[serde(rename = "seed")]
    Seed,
    #[serde(rename = "delete")]
    Delete,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsersWriteRequest {
    pub(crate) action: UsersWriteAction,
    /// Server profile name. It must say `allow_test_users = true`.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum AppKeyAction {
    #[serde(rename = "create")]
    Create,
    #[serde(rename = "show")]
    Show,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppKeyRequest {
    pub(crate) action: AppKeyAction,
    /// The application key's name.
    pub(crate) name: String,
    /// create: the user the key acts as.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) user: Absent<String>,
    /// create: when it expires: 30d (the default), 12h, a date (midnight UTC) or an RFC 3339 timestamp.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) expires: Absent<String>,
    /// create: the project the key belongs to.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) project: Absent<String>,
    /// create: the key's description.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) description: Absent<String>,
    /// create: addresses or ranges the key may be used from; empty means any.
    #[serde(default)]
    pub(crate) whitelist: Vec<String>,
    /// Write the key as app_key of this profile file, which must exist.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) save: Absent<String>,
    /// Return the key itself in the result.
    #[serde(default)]
    pub(crate) show: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// create: true plans and sends nothing. show reads the server whatever it says.
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestRequest {
    /// Keep the cases whose name, service or file contains one of these.
    #[serde(default)]
    pub filters: Vec<String>,
    /// Server profile name. It must say `allow_test_users = true` to run the cases.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// True lists and checks the cases; false calls them, which may change the server.
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum ExtensionsAction {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "show")]
    Show,
}

/// `ext_check`: a Java extension project's folder.
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtCheckRequest {
    /// The extension project's folder, absolute or relative to the solution; omit for the
    /// solution folder itself.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) root: Absent<String>,
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

/// `ext_new`: a new Java extension project.
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtNewRequest {
    /// The extension package's name.
    pub(crate) name: String,
    /// The folder to create, relative to the solution and inside it; default: the name.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) folder: Absent<String>,
    /// The Java package; default com.<vendor>.<name>.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) package: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) vendor: Absent<String>,
    /// Default 1.0.0.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) package_version: Absent<String>,
    /// Default 10.0.0.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) min_thingworx: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) description: Absent<String>,
    /// The Extension SDK zip or folder whose jars go into twx-lib/; default TWACO_SDK.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) sdk: Absent<String>,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum ExtAddKind {
    #[serde(rename = "thing-template")]
    ThingTemplate,
    #[serde(rename = "thing-shape")]
    ThingShape,
    #[serde(rename = "resource")]
    Resource,
    #[serde(rename = "widget")]
    Widget,
    #[serde(rename = "jar")]
    Jar,
}

/// `ext_add`: one thing added to a Java extension project.
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtAddRequest {
    pub(crate) kind: ExtAddKind,
    /// The entity or widget name; for a jar, the file to copy into lib/, relative to the
    /// solution and inside it.
    pub(crate) name: String,
    /// The extension project's folder, relative to the solution and inside it; omit for the
    /// solution folder itself.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) root: Absent<String>,
    /// thing-template, thing-shape, resource: the Java package; default the one metadata.xml's
    /// classes use.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) package: Absent<String>,
    /// thing-template: its base template; default GenericThing.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) base: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) description: Absent<String>,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
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
pub(crate) struct BundleRequest {
    /// Leave out the collections [bundle] ui_collections names (a designer's mashups and media).
    #[serde(default)]
    pub backend_only: bool,
    /// Select entities whose source file or sidecars changed since this Git revision.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) changed_since: Absent<String>,
    /// Remove matching Collection/Name selectors from the selected entities.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) exclude: Absent<Vec<String>>,
    /// Write or check this bundle path inside the solution instead of [bundle]'s dist target.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) out: Absent<String>,
    /// Apply this profile's mashup patches from twaco.toml; without it, none apply.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) profile: Absent<String>,
    /// Record the rebuilt bundle as this collaborator handoff; only with dry_run false.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) handoff: Absent<String>,
    /// Only say whether the configured bundle is current, as `twaco bundle --check` does.
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
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
    /// Only entities changed at or after this: a duration back from now (1d, 12h, 30m), a date (midnight UTC) or an RFC 3339 timestamp.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) modified_since: Absent<String>,
    /// name: A to Z; modified: newest change first. Absent: modified with modified_since, name without. With modified_since the limit keeps the newest, whatever the order.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) sort: Absent<SearchSort>,
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1))]
    pub(crate) limit: u64,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum SearchSort {
    #[serde(rename = "name")]
    Name,
    #[serde(rename = "modified")]
    Modified,
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
    pub action: ExportAction,
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
    pub profile: String,
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
    pub action: PackageAction,
    /// Only this project; the whole solution when omitted.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub project: Absent<String>,
    /// bundle: which collections.
    #[serde(default = "default_package_part")]
    pub(crate) part: PackagePart,
    /// extension: whether the entities stay editable once installed.
    #[serde(default)]
    pub(crate) editable: bool,
    /// The file to write, relative to the solution root.
    pub out: String,
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
    pub action: ImportAction,
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
    pub overwrite_properties: bool,
    #[serde(default)]
    pub overwrite_tables: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub profile: String,
}
