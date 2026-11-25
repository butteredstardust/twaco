//! The widget cache: the contracts of the legacy and extension widgets an installation ships,
//! read from its widget files without running them (`twaco widgets --platform` or `--from`).
//!
//! The embedded catalogue stays the offline default and always wins for the widgets it has. The
//! cache, `.twaco/widgets.json`, adds the rest: legacy platform widgets (`tree`, `collection`,
//! `gauge`, ...) from `Common/mashup-common-widgets-shared.js` and `-builder.js`, and each
//! installed extension's widgets from its `*.ide.js` and `*.config.js`. A solution that loads
//! with a cache lets `widgets`, `check`, `mashup validate` and the page specs judge those widgets
//! by their contracts instead of carrying them as unknown.
//!
//! The reading is static ([`super::extension_widgets::read_report`]): a widget whose contract is
//! built at run time is listed as unknown with the reason and is never guessed.

use super::codes::{Coded, ErrorCode};
use super::config::Solution;
use super::entity::key::ServiceTarget;
use super::extension_widgets::{read_report, Found};
use super::lock::{self, WorkspaceLock};
use super::types::{platform, Remote};
use super::widgets::{self, Ambient, PropertyContract, Widget};
use super::workspace;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Where the cache lives, relative to the solution root.
pub const FILE: &str = ".twaco/widgets.json";

const SCHEMA: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum WidgetCacheError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Source(String),
    #[error("{0}")]
    Write(String),
}

impl Coded for WidgetCacheError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
            Self::Source(_) => ErrorCode::InvalidArguments,
            Self::Write(_) => ErrorCode::IoError,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    schema: u32,
    /// What the server reported, when it was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    platform_version: Option<String>,
    fetched_at: String,
    /// `server` or `folder`.
    source: String,
    widgets: Vec<Cached>,
    #[serde(default)]
    unknown: Vec<Unknown>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Cached {
    #[serde(rename = "type")]
    type_id: String,
    /// `legacy` (a platform widget the embedded catalogue lacks) or `extension`.
    kind: String,
    label: String,
    #[serde(default)]
    auto_resize: bool,
    defaults: Map<String, Value>,
    properties: BTreeMap<String, PropertyContract>,
    events: Vec<String>,
    services: Vec<String>,
    /// What the reading left out of this widget, one line each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
    /// The file it was read from.
    origin: String,
}

/// A widget whose contract could not be read statically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unknown {
    #[serde(rename = "type")]
    pub type_id: String,
    pub origin: String,
    pub why: String,
}

/// What a refresh found and where it put it.
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub cache: String,
    pub source: String,
    pub platform_version: Option<String>,
    pub fetched_at: String,
    /// Widgets the embedded catalogue already has (it wins; nothing is cached for them).
    pub platform: Vec<String>,
    pub legacy: Vec<String>,
    pub extension: Vec<String>,
    pub unknown: Vec<Unknown>,
    /// Files the reading could not take, and what could not be fetched.
    pub skipped: Vec<String>,
}

/// The widget files of one installation, as text.
#[derive(Default)]
struct Inputs {
    version: Option<String>,
    source: &'static str,
    meta: Option<String>,
    /// `(origin, text)` of the platform widget bundles.
    platform: Vec<(String, String)>,
    /// `(origin, text)` of extension code.
    extensions: Vec<(String, String)>,
    skipped: Vec<String>,
}

const META: &str = "Common/widget-meta-values.json";
const SHARED: &str = "Common/mashup-common-widgets-shared.js";
const BUILDER: &str = "Common/mashup-common-widgets-builder.js";

/// Read a Thingworx webapp folder (the one holding `Common/`).
fn from_folder(root: &Path) -> Result<Inputs, WidgetCacheError> {
    let read = |relative: &str| {
        std::fs::read_to_string(root.join(relative)).map_err(|e| {
            WidgetCacheError::Source(format!(
                "cannot read {relative} under {}: {e}; give the Thingworx webapp folder that holds Common/",
                root.display()
            ))
        })
    };
    let mut inputs = Inputs {
        source: "folder",
        ..Inputs::default()
    };
    inputs.platform.push((SHARED.into(), read(SHARED)?));
    inputs.platform.push((BUILDER.into(), read(BUILDER)?));
    match read(META) {
        Ok(text) => inputs.meta = Some(text),
        Err(error) => inputs.skipped.push(error.to_string()),
    }
    let extensions = root.join("Common").join("extensions");
    let mut files = Vec::new();
    collect_extension_files(&extensions, &mut files, 0);
    files.sort();
    for file in files {
        let origin = file
            .strip_prefix(root.join("Common"))
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        match std::fs::read_to_string(&file) {
            Ok(text) => inputs.extensions.push((origin, text)),
            Err(error) => inputs.skipped.push(format!("{origin}: {error}")),
        }
    }
    Ok(inputs)
}

/// `*.ide.js` and `*.config.js` under `extensions/<name>/ui/<widget>/`.
fn collect_extension_files(dir: &Path, out: &mut Vec<PathBuf>, depth: u8) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_extension_files(&path, out, depth + 1);
        } else if kind.is_file() && (name.ends_with(".ide.js") || name.ends_with(".config.js")) {
            out.push(path);
        }
    }
}

/// Fetch the same files from the server by GET: the platform bundles and the combined code of
/// the installed extensions, as the mashup builder loads it.
fn from_remote(remote: &dyn Remote) -> Result<Inputs, WidgetCacheError> {
    let fetch = |path: &str| -> Result<String, WidgetCacheError> {
        match remote.web_file(path) {
            Ok(Some(bytes)) => String::from_utf8(bytes)
                .map_err(|_| WidgetCacheError::Source(format!("{path} is not UTF-8"))),
            Ok(None) => Err(WidgetCacheError::Source(format!(
                "this server connection cannot fetch {path}; use --from <webapp folder>"
            ))),
            Err(error) => Err(WidgetCacheError::Source(format!(
                "cannot fetch {path}: {error}; use --from <webapp folder> for an installation on disk"
            ))),
        }
    };
    let mut inputs = Inputs {
        source: "server",
        ..Inputs::default()
    };
    inputs.platform.push((SHARED.into(), fetch(SHARED)?));
    inputs.platform.push((BUILDER.into(), fetch(BUILDER)?));
    match fetch(META) {
        Ok(text) => inputs.meta = Some(text),
        Err(error) => inputs.skipped.push(error.to_string()),
    }
    match platform::extension_widget_code(remote) {
        Ok(Some(code)) => inputs
            .extensions
            .push(("Builder extension bundle".into(), code)),
        Ok(None) => {}
        Err(why) => inputs.skipped.push(format!("extension widgets: {why}")),
    }
    inputs.version = remote
        .call(
            &ServiceTarget::platform("Subsystems", "PlatformSubsystem"),
            "GetPlatformStats",
            &serde_json::json!({}),
        )
        .ok()
        .flatten()
        .and_then(|reply| {
            reply
                .pointer("/rows/0/thingworxSoftwareVersion")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Ok(inputs)
}

/// Read what the widget files say and sort it: the embedded catalogue's widgets, legacy platform
/// widgets, extension widgets, and the unknown.
fn build(inputs: Inputs) -> (CacheFile, Outcome) {
    let embedded: BTreeSet<&str> = widgets::catalog()
        .widgets
        .iter()
        .map(|widget| widget.type_id.as_str())
        .collect();
    let meta: Map<String, Value> = inputs
        .meta
        .as_deref()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default();
    let mut skipped = inputs.skipped;
    let mut platform_seen = BTreeSet::new();
    let mut cached: BTreeMap<String, Cached> = BTreeMap::new();
    let mut unknown: BTreeMap<String, Unknown> = BTreeMap::new();

    let groups = [
        (&inputs.platform, "legacy"),
        (&inputs.extensions, "extension"),
    ];
    for (files, kind) in groups {
        for (origin, text) in files {
            let report = match read_report(text) {
                Ok(report) => report,
                Err(why) => {
                    skipped.push(format!("{origin}: {why}"));
                    continue;
                }
            };
            let mut found = report.found;
            for (type_id, why) in report.unnamed {
                found.push(Found {
                    widget: Widget::extension(
                        type_id,
                        String::new(),
                        false,
                        Map::new(),
                        BTreeMap::new(),
                        Vec::new(),
                        None,
                    ),
                    notes: Vec::new(),
                    unknown: Some(why),
                });
            }
            for item in found {
                let type_id = item.widget.type_id.clone();
                if kind == "legacy" && embedded.contains(type_id.as_str()) {
                    platform_seen.insert(type_id);
                    continue;
                }
                if let Some(why) = item.unknown {
                    if !cached.contains_key(&type_id) {
                        unknown.entry(type_id.clone()).or_insert(Unknown {
                            type_id,
                            origin: origin.clone(),
                            why,
                        });
                    }
                    continue;
                }
                unknown.remove(&type_id);
                cached
                    .entry(type_id.clone())
                    .or_insert_with(|| cached_widget(item, kind, origin, &meta));
            }
        }
    }

    let legacy: Vec<String> = cached
        .values()
        .filter(|widget| widget.kind == "legacy")
        .map(|widget| widget.type_id.clone())
        .collect();
    let extension: Vec<String> = cached
        .values()
        .filter(|widget| widget.kind == "extension")
        .map(|widget| widget.type_id.clone())
        .collect();
    let unknown: Vec<Unknown> = unknown.into_values().collect();
    let fetched_at = jiff::Timestamp::now().to_string();
    let file = CacheFile {
        schema: SCHEMA,
        platform_version: inputs.version.clone(),
        fetched_at: fetched_at.clone(),
        source: inputs.source.to_string(),
        widgets: cached.into_values().collect(),
        unknown: unknown.clone(),
    };
    let outcome = Outcome {
        cache: FILE.to_string(),
        source: inputs.source.to_string(),
        platform_version: inputs.version,
        fetched_at,
        platform: platform_seen.into_iter().collect(),
        legacy,
        extension,
        unknown,
        skipped,
    };
    (file, outcome)
}

fn cached_widget(item: Found, kind: &str, origin: &str, meta: &Map<String, Value>) -> Cached {
    let widget = item.widget;
    let mut notes = item.notes;
    let mut defaults = widget.defaults.clone();
    if kind == "legacy" {
        // Composer's own defaults for a legacy widget are in its meta values file.
        if let Some(Value::Object(own)) =
            meta.get(&widget.type_id).and_then(|m| m.get("Properties"))
        {
            defaults = own.clone();
        }
        let before = defaults.len();
        // `__tw.*` and `$$...` values are translated or resolved by Composer when it drops a widget.
        defaults.retain(|_, value| {
            !value
                .as_str()
                .is_some_and(|text| text.starts_with("__tw.") || text.starts_with("$$"))
        });
        if defaults.len() != before {
            notes.push(format!(
                "{} defaults need Composer's translation and are left out",
                before - defaults.len()
            ));
        }
    }
    let label = if widget.type_display_name.starts_with("tw.") {
        widget.type_id.clone()
    } else {
        widget.type_display_name.clone()
    };
    Cached {
        type_id: widget.type_id,
        kind: kind.to_string(),
        label,
        auto_resize: widget.auto_resize,
        defaults,
        properties: widget.properties.unwrap_or_default(),
        events: widget.events.unwrap_or_default(),
        services: widget.services.unwrap_or_default(),
        notes,
        origin: origin.to_string(),
    }
}

/// Fetch (`from` is `None`) or read a folder, and cache the result. The cache is the workspace's:
/// `_held` proves the caller holds the workspace lock.
pub(crate) fn refresh(
    solution: &Solution,
    remote: Option<&dyn Remote>,
    from: Option<&Path>,
    _held: &WorkspaceLock,
) -> Result<Outcome, WidgetCacheError> {
    lock::reject_unlocked_write_links(solution).map_err(WidgetCacheError::Lock)?;
    let inputs = match (from, remote) {
        (Some(folder), _) => from_folder(folder)?,
        (None, Some(remote)) => from_remote(remote)?,
        (None, None) => {
            return Err(WidgetCacheError::Source(
                "give a server profile or --from <webapp folder>".into(),
            ))
        }
    };
    let (file, outcome) = build(inputs);
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&file).expect("the widget cache is JSON encodable")
    );
    workspace::write_lf_if_changed(&solution.root.join(FILE), &text)
        .map_err(|e| WidgetCacheError::Write(e.to_string()))?;
    install(&solution.root);
    Ok(outcome)
}

/// What a cache holds, for a human: its source and age.
fn provenance(file: &CacheFile, kind: &str) -> String {
    let version = file
        .platform_version
        .as_deref()
        .map(|version| format!(" ThingWorx {version},"))
        .unwrap_or_default();
    format!(
        "{kind} widget: contract read from the widget cache ({}{version} fetched {}); `twaco widgets --platform` refreshes it",
        file.source, file.fetched_at
    )
}

type Loaded = (SystemTime, u64, &'static Ambient);

/// Make the solution's widget cache, if it has one, what this thread's widget lookups consult
/// after the embedded catalogue. No cache, or an unreadable one, leaves the embedded catalogue
/// alone. Called whenever a solution is loaded.
pub(crate) fn install(root: &Path) {
    static LOADED: OnceLock<Mutex<HashMap<PathBuf, Loaded>>> = OnceLock::new();
    let path = root.join(FILE);
    let Ok(meta) = std::fs::metadata(&path) else {
        return widgets::set_ambient(None);
    };
    let stamp = (
        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        meta.len(),
    );
    let loaded = LOADED.get_or_init(Mutex::default);
    let mut loaded = loaded.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((time, len, ambient)) = loaded.get(&path) {
        if (*time, *len) == stamp {
            return widgets::set_ambient(Some(ambient));
        }
    }
    let ambient = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
        .filter(|file| file.schema == SCHEMA)
        .map(ambient_of);
    match ambient {
        Some(ambient) => {
            // One small allocation per change of the file, kept for the life of the process.
            let ambient: &'static Ambient = Box::leak(Box::new(ambient));
            loaded.insert(path, (stamp.0, stamp.1, ambient));
            widgets::set_ambient(Some(ambient));
        }
        None => widgets::set_ambient(None),
    }
}

fn ambient_of(file: CacheFile) -> Ambient {
    let embedded = widgets::catalog();
    let mut widgets_out = Vec::new();
    for item in &file.widgets {
        // The embedded catalogue wins for what it has.
        if embedded
            .widgets
            .iter()
            .any(|widget| widget.type_id == item.type_id)
        {
            continue;
        }
        let mut widget = Widget::extension(
            item.type_id.clone(),
            item.label.clone(),
            item.auto_resize,
            item.defaults.clone(),
            item.properties.clone(),
            item.events.clone(),
            Some(item.services.clone()),
        );
        widget.legacy = item.kind == "legacy";
        widget.extension = item.kind == "extension";
        widget.provenance = Some(provenance(&file, &item.kind));
        widgets_out.push(widget);
    }
    Ambient {
        widgets: widgets_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::server::ServerError;
    use serde_json::json;

    const SHARED_JS: &str = r#"
(function (e) {
e.Widget.widgetWrapper.config("ptcsbutton", { flags: { name: "x" }, properties: { Label: { baseType: "STRING" }, FakeOnly: { baseType: "STRING" } }, events: { Clicked: {}, FakeEvent: {} } });
e.Widget.widgetWrapper.config("acmemodern", { flags: { name: "Acme Modern" }, properties: { Title: { baseType: "STRING", isBindingTarget: !0, defaultValue: "T" } }, events: { Opened: {} } });
}(TW));
"#;
    const BUILDER_JS: &str = r#"
TW.IDE.Widgets.acmetree = function () {
    this.widgetProperties = function () {
        return this.checked = TW.UTIL.check(this), {
            name: TW.IDE.I18NController.translate("tw.acmetree-ide.widget.name"),
            properties: {
                Data: { baseType: "INFOTABLE", isBindingTarget: !0 },
                Picked: { baseType: "STRING", isBindingSource: !0 }
            }
        };
    };
    this.widgetEvents = function () { return { Picked: {}, Changed: {} }; };
    this.widgetServices = function () { return { Collapse: {} }; };
};
TW.IDE.Widgets.acmegauge = function () {
    this.widgetProperties = function () { return TW.IDE.build(); };
};
"#;
    const META_JSON: &str =
        r#"{"acmetree": {"Properties": {"Width": 225, "Label": "__tw.x.label", "Height": 300}}}"#;
    const EXTENSION_JS: &str = r#"
TW.IDE.Widgets.acmeext = function () {
    this.widgetProperties = function () {
        var props = { Value: { baseType: "NUMBER", isBindingTarget: true, defaultValue: 3 } };
        return { name: "Acme Ext", properties: props };
    };
    this.widgetEvents = function () { return { Ready: {} }; };
};
"#;

    fn webapp(root: &Path) -> PathBuf {
        let webapp = root.join("webapp");
        let common = webapp.join("Common");
        let ui = common.join("extensions/acme-ext/ui/acmeext");
        std::fs::create_dir_all(&ui).unwrap();
        std::fs::write(common.join("widget-meta-values.json"), META_JSON).unwrap();
        std::fs::write(common.join("mashup-common-widgets-shared.js"), SHARED_JS).unwrap();
        std::fs::write(common.join("mashup-common-widgets-builder.js"), BUILDER_JS).unwrap();
        std::fs::write(ui.join("acmeext.ide.js"), EXTENSION_JS).unwrap();
        webapp
    }

    fn solution() -> (tempfile::TempDir, Solution) {
        let guard = tempfile::Builder::new()
            .prefix("twaco-widget-cache-")
            .tempdir()
            .unwrap();
        std::fs::write(
            guard.path().join("twaco.toml"),
            "[[project]]\nname = \"P\"\n",
        )
        .unwrap();
        let solution = Solution::load(&guard.path().join("twaco.toml")).unwrap();
        (guard, solution)
    }

    fn refresh_from(solution: &Solution, folder: &Path) -> Outcome {
        let held = lock::acquire(&solution.root, "test", &[]).unwrap();
        refresh(solution, None, Some(folder), &held).unwrap()
    }

    #[test]
    fn a_folder_is_read_into_legacy_extension_and_unknown_widgets() {
        let (guard, solution) = solution();
        let outcome = refresh_from(&solution, &webapp(guard.path()));
        assert_eq!(outcome.platform, ["ptcsbutton"]);
        assert_eq!(outcome.legacy, ["acmemodern", "acmetree"]);
        assert_eq!(outcome.extension, ["acmeext"]);
        assert_eq!(outcome.unknown.len(), 1);
        assert_eq!(outcome.unknown[0].type_id, "acmegauge");
        assert!(outcome.unknown[0].why.contains("object literal"));
        assert_eq!(outcome.source, "folder");
        let text = std::fs::read_to_string(solution.root.join(FILE)).unwrap();
        let file: CacheFile = serde_json::from_str(&text).unwrap();
        assert_eq!(file.schema, SCHEMA);
        assert!(!file.fetched_at.is_empty());
        let tree = file
            .widgets
            .iter()
            .find(|w| w.type_id == "acmetree")
            .unwrap();
        // Composer's defaults come from the meta values file, minus what it translates.
        assert_eq!(tree.defaults.get("Width"), Some(&json!(225)));
        assert!(!tree.defaults.contains_key("Label"));
        assert_eq!(tree.label, "acmetree");
        assert!(tree.notes[0].contains("1 defaults"));
        assert_eq!(tree.events, ["Picked", "Changed"]);
        assert_eq!(tree.services, ["Collapse"]);
        assert!(!text.contains('\r'));
    }

    #[test]
    fn the_cache_adds_widgets_after_the_embedded_catalogue_which_wins() {
        let (guard, solution) = solution();
        assert!(widgets::catalog().by_type("acmetree").is_none());
        refresh_from(&solution, &webapp(guard.path()));
        // Loading the solution again installs the cache that was written.
        Solution::load(&solution.root.join("twaco.toml")).unwrap();
        let catalog = widgets::catalog();
        let tree = catalog.by_type("acmetree").unwrap();
        assert!(tree.has_contract() && tree.legacy);
        assert!(tree.has_event("Changed") && tree.has_service("Collapse"));
        assert!(tree.property("Data").unwrap().target);
        assert_eq!(catalog.resolve("ACMETREE").unwrap().type_id, "acmetree");
        assert!(catalog.by_type("acmeext").unwrap().extension);
        // The folder redefines ptcsbutton with other events; the embedded entry stays.
        let button = catalog.by_type("ptcsbutton").unwrap();
        assert!(button.has_event("Clicked") && !button.has_event("FakeEvent"));
        assert!(button.property("FakeOnly").is_none());
        assert!(button.provenance.is_none());
        // `mashup add` never authors a legacy widget from the cache.
        assert!(catalog.resolve_for_add("acmetree").is_err());
        let listed: Vec<String> = crate::core::widgets_view::list(None)
            .into_iter()
            .map(|widget| widget.type_id)
            .collect();
        assert!(listed.contains(&"acmetree".to_string()));
        // A solution without a cache sees the embedded catalogue alone.
        let (_other, _bare) = self::solution();
        assert!(widgets::catalog().by_type("acmetree").is_none());
    }

    #[test]
    fn a_legacy_widget_is_judged_by_its_contract_only_with_the_cache() {
        let page = json!({"UI": {"Properties": {"Type": "mashup", "Id": "mashup-root"}, "Widgets": [
            {"Properties": {"Type": "acmetree", "Id": "acmetree-1"}}
        ]}, "Events": [
            {"Id": "e1", "EventTriggerArea": "UI", "EventTriggerId": "acmetree-1", "EventTriggerEvent": "DoubleClicked",
             "EventHandlerArea": "UI", "EventHandlerId": "acmetree-1", "EventHandlerService": "Collapse"},
            {"Id": "e2", "EventTriggerArea": "UI", "EventTriggerId": "acmetree-1", "EventTriggerEvent": "Changed",
             "EventHandlerArea": "UI", "EventHandlerId": "acmetree-1", "EventHandlerService": "Explode"}
        ]})
        .to_string();
        let messages = |text: &str| {
            let mut found = Vec::new();
            crate::core::mashup::validate::check_content(text, &mut found);
            found.into_iter().map(|p| p.message).collect::<Vec<_>>()
        };
        let (guard, solution) = solution();
        // Without a cache the widget has no contract: nothing is judged.
        assert!(messages(&page)
            .iter()
            .all(|m| !m.contains("acmetree-1 (acmetree)")));
        refresh_from(&solution, &webapp(guard.path()));
        let found = messages(&page);
        assert!(
            found
                .iter()
                .any(|m| m.contains("DoubleClicked") && m.contains("it has Changed, Picked")),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|m| m.contains("Explode") && m.contains("it has Collapse")),
            "{found:?}"
        );
        // Changed and Collapse exist: they are not reported.
        assert!(found
            .iter()
            .all(|m| !m.contains("Changed, which") && !m.contains("Collapse, which")));
    }

    struct FakeServer {
        files: BTreeMap<String, Vec<u8>>,
        requested: std::cell::RefCell<Vec<String>>,
    }

    impl Remote for FakeServer {
        fn call(
            &self,
            target: &ServiceTarget,
            service: &str,
            _: &Value,
        ) -> Result<Option<Value>, ServerError> {
            assert_eq!(
                (target.to_string().as_str(), service),
                ("Subsystems/PlatformSubsystem", "GetPlatformStats"),
                "only the version is asked of the server"
            );
            Ok(Some(
                json!({"rows": [{"thingworxSoftwareVersion": "10.2.0-b1"}]}),
            ))
        }

        fn web_file(&self, path: &str) -> Result<Option<Vec<u8>>, ServerError> {
            self.requested.borrow_mut().push(path.to_string());
            Ok(self.files.get(path).cloned())
        }
    }

    fn fake_server() -> FakeServer {
        let mut files = BTreeMap::new();
        files.insert(
            "Common/widget-meta-values.json".to_string(),
            META_JSON.as_bytes().to_vec(),
        );
        files.insert(
            "Common/mashup-common-widgets-shared.js".to_string(),
            SHARED_JS.as_bytes().to_vec(),
        );
        files.insert(
            "Common/mashup-common-widgets-builder.js".to_string(),
            BUILDER_JS.as_bytes().to_vec(),
        );
        files.insert(
            "Builder/index.html".to_string(),
            b"<script>var COMBINED_EXT_JS='js/combined.ext.js';</script>".to_vec(),
        );
        files.insert(
            "Builder/js/combined.ext.js".to_string(),
            EXTENSION_JS.as_bytes().to_vec(),
        );
        FakeServer {
            files,
            requested: Default::default(),
        }
    }

    #[test]
    fn a_fake_server_gives_the_same_cache_by_get_alone() {
        let (_guard, solution) = solution();
        let server = fake_server();
        let held = lock::acquire(&solution.root, "test", &[]).unwrap();
        let outcome = refresh(&solution, Some(&server), None, &held).unwrap();
        assert_eq!(outcome.source, "server");
        assert_eq!(outcome.platform_version.as_deref(), Some("10.2.0-b1"));
        assert_eq!(outcome.legacy, ["acmemodern", "acmetree"]);
        assert_eq!(outcome.extension, ["acmeext"]);
        assert_eq!(outcome.unknown.len(), 1);
        assert!(server
            .requested
            .borrow()
            .iter()
            .any(|p| p == "Builder/js/combined.ext.js"));
        let text = std::fs::read_to_string(solution.root.join(FILE)).unwrap();
        assert!(text.contains("\"platform_version\": \"10.2.0-b1\""));
        // The cache it wrote is read back on the next load.
        Solution::load(&solution.root.join("twaco.toml")).unwrap();
        let tree = widgets::catalog().by_type("acmetree").unwrap();
        assert!(tree
            .provenance
            .as_deref()
            .unwrap()
            .contains("ThingWorx 10.2.0-b1"));
    }

    #[test]
    fn a_server_that_cannot_serve_files_or_a_folder_without_common_is_refused() {
        let (guard, solution) = solution();
        let held = lock::acquire(&solution.root, "test", &[]).unwrap();
        let empty = FakeServer {
            files: BTreeMap::new(),
            requested: Default::default(),
        };
        let error = refresh(&solution, Some(&empty), None, &held).unwrap_err();
        assert!(error.to_string().contains("--from"), "{error}");
        let error = refresh(&solution, None, Some(guard.path()), &held).unwrap_err();
        assert!(error.to_string().contains("Common"), "{error}");
        assert!(!solution.root.join(FILE).exists());
    }

    #[test]
    fn an_unreadable_cache_behaves_as_none() {
        let (_guard, solution) = solution();
        std::fs::create_dir_all(solution.root.join(".twaco")).unwrap();
        std::fs::write(solution.root.join(FILE), "{not json").unwrap();
        install(&solution.root);
        assert!(widgets::catalog().by_type("acmetree").is_none());
    }
}
