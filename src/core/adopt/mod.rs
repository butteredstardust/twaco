//! Comparing a designer's Composer export with the repository, before anything is adopted.
//!
//! A Composer export may contain stale service bodies, so importing it wholesale can revert
//! repository changes. It also uses exporter-specific layout and includes live metadata and
//! values, making a plain diff too noisy to isolate meaningful changes.
//!
//! So the report answers two questions, in this order:
//!
//! 1. **What would the export revert?** Every service body in the export is compared with its
//!    sidecar, or with the entity XML where there is none.
//! 2. **What would it change?** Entity by entity and node by node, with the two exporters'
//!    formatting settled, regenerated binding ids counted rather than listed, and the solution's
//!    `[adopt] ignore_paths` left out.
//!
//! This is the generic engine; applying an export is project policy. The comparison follows
//! ElementTree-compatible text semantics: an element's text is what precedes its first child,
//! CDATA included and comments invisible.

mod base;
mod classify;

#[cfg(test)]
mod apply_tests;
#[cfg(test)]
mod base_tests;
#[cfg(test)]
mod classify_tests;

pub use base::{
    handoffs, history_versions, record_handoff, resolve, Base, BaseSource, Handoff, Side,
};
pub use classify::{Change, Kind};

use super::config::Solution;
use super::lock::WorkspaceLock;
use super::normalise::{self, Element, Node};
use super::transaction::Transaction;
use super::workspace;
use super::{scan, sidecar, splice, sync};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// Elements the platform rewrites on its own: whoever exported the file, an icon nobody sets,
/// and two history tables that grow on every save.
const NOISE_TAGS: [&str; 4] = ["ChangeHistory", "ConfigurationChanges", "Owner", "avatar"];
/// Attributes with the same problem. `sourceType`/`source` record which import last wrote the
/// entity, so they differ by construction between two servers.
const NOISE_ATTRS: [&str; 5] = [
    "lastModifiedDate",
    "owner",
    "sourceType",
    "source",
    "creationDate",
];
/// A configuration table row identifies itself by one of these children, in this order. Keying
/// on the value rather than the position stops an inserted row from shifting every path after it.
const ROW_KEY_TAGS: [&str; 4] = ["UID", "Name", "name", "id"];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityRef {
    pub collection: String,
    pub name: String,
}

impl EntityRef {
    pub fn path(&self) -> String {
        format!("{}/{}", self.collection, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub path: String,
    pub export: Option<String>,
    pub repo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    New,
    Changed,
    Identical,
}

#[derive(Debug, Clone)]
pub struct EntityReport {
    pub entity: EntityRef,
    /// The export's own `projectName`, so a multi-project drop is attributed per project.
    pub project: String,
    pub status: Status,
    pub differences: Vec<Difference>,
    pub volatile_ids: usize,
    pub ignored: usize,
    /// Whether this is a designer-owned collection or backend work.
    pub kind: Kind,
    /// What the three-way comparison says about importing the export.
    pub change: Change,
}

#[derive(Debug, Clone)]
pub struct ServiceReport {
    pub entity: String,
    pub service: String,
    pub source: PathBuf,
    /// Generated here; the repository is authoritative and the difference expected.
    pub generated: bool,
    /// False when compared with the entity XML because the service has no sidecar.
    pub sidecar: bool,
    /// What the three-way comparison says about this service body.
    pub change: Change,
}

#[derive(Debug, Default)]
pub struct Report {
    /// The base used for the three-way comparison, when one was found.
    pub base: Option<String>,
    /// The parsed base is retained for apply's frame-level merge.  It is intentionally not a
    /// presentation field: callers present `base`, above, rather than an XML tree.
    base_side: Option<Side>,
    pub services: Vec<ServiceReport>,
    pub entities: Vec<EntityReport>,
    /// Exported services with nothing here to compare with.
    pub unmatched_services: Vec<String>,
    /// In the repository, in a collection the export carries, but not in the export.
    pub absent: Vec<EntityRef>,
}

impl Report {
    pub fn with_status(&self, status: Status) -> impl Iterator<Item = &EntityReport> {
        self.entities
            .iter()
            .filter(move |entity| entity.status == status)
    }

    /// Non-generated services for which importing the export would undo or conflict with
    /// repository work. `Theirs`, `Added` and `Same` are not reverts.
    pub fn reverts(&self) -> impl Iterator<Item = &ServiceReport> {
        self.services.iter().filter(|service| {
            !service.generated
                && matches!(
                    service.change,
                    Change::Stale | Change::Conflict | Change::Unknown
                )
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    #[error("cannot read export {}: {why}", .path.display())]
    Export { path: PathBuf, why: String },
    #[error("cannot read {}: {why}", .path.display())]
    Repository { path: PathBuf, why: String },
    #[error("cannot use base {base}: {why}")]
    Base { base: String, why: String },
    #[error("handoff {name} already exists")]
    AlreadyExists { name: String },
    /// The writes, made as one transaction, did not happen.
    #[error("nothing was adopted: {0}")]
    Write(super::transaction::TransactionError),
    #[error("--take names nothing in this report: {0}")]
    Take(String),
}

/// Which side an explicit adopt resolution selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakeSide {
    Theirs,
    Ours,
}

/// An explicit resolution for an entity (`Thing`) or service (`Thing.Run`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Take {
    pub side: TakeSide,
    pub target: String,
}

/// Extra inputs to the three-way comparison.  The old [`compare`] entry point intentionally
/// keeps its two-way-shaped signature for existing front ends.
#[derive(Debug, Default)]
pub struct CompareOptions {
    pub base: Option<String>,
    pub only_kind: Option<Kind>,
}

/// Compare an export with the solution. `only` retains its established name-fragment meaning.
pub fn compare(solution: &Solution, export: &Path, only: &[String]) -> Result<Report, AdoptError> {
    compare_with(solution, export, only, &CompareOptions::default())
}

/// Compare an export using an explicit base and/or collection kind filter.
pub fn compare_with(
    solution: &Solution,
    export: &Path,
    only: &[String],
    options: &CompareOptions,
) -> Result<Report, AdoptError> {
    let exported = export_entities(export)?;
    let export_side = Side::from_export(export)?;
    let here = repository_entities(solution)?;
    let ours = Side::from_solution(solution)?;
    let base = resolve(solution, options.base.as_deref(), &export_side)?;
    let mut report = Report {
        base: base.as_ref().map(|base| base.label.clone()),
        base_side: base.as_ref().map(|base| base.side.clone()),
        ..Report::default()
    };

    let generated: BTreeSet<&str> = solution
        .adopt
        .generated_services
        .iter()
        .map(String::as_str)
        .collect();
    let kind_passes = |entity: &EntityRef| {
        options
            .only_kind
            .is_none_or(|kind| kind == kind_of(solution, entity))
    };
    // `only` narrows the entity comparison, not the service check: a revert anywhere in the
    // export matters whichever entity was asked about.
    let passes = |entity: &EntityRef| {
        kind_passes(entity)
            && (only.is_empty()
                || only
                    .iter()
                    .any(|fragment| entity.name.contains(fragment.as_str())))
    };

    for (entity, element) in &exported {
        if !kind_passes(entity) {
            continue;
        }
        for (service, body) in service_bodies(element) {
            let label = format!("{}.{service}", entity.name);
            let sidecar = solution
                .src_root()
                .join(&entity.name)
                .join("services")
                .join(&service)
                .join("script.js");
            let key = (entity.name.clone(), service.clone());
            let mine = ours.services.get(&key);
            let (source, from_sidecar) = if sidecar.is_file() {
                (sidecar, true)
            } else if let Some((_, path)) = here.get(entity) {
                (path.clone(), false)
            } else {
                report.unmatched_services.push(label);
                continue;
            };
            let base_service = base.as_ref().and_then(|base| base.side.services.get(&key));
            let change = match mine {
                Some(mine) => script_change(&body, mine, base_service, || {
                    history_versions(solution, &source, 50).iter().any(|old| {
                        std::str::from_utf8(old).is_ok_and(|old| canonical(&body) == canonical(old))
                    })
                }),
                None => {
                    if base_service.is_some() {
                        Change::WeRemoved
                    } else {
                        Change::Added
                    }
                }
            };
            // Only a difference is reported, as before the three-way states existed.
            if change == Change::Same {
                continue;
            }
            report.services.push(ServiceReport {
                entity: entity.name.clone(),
                service,
                source,
                generated: generated.contains(label.as_str()),
                sidecar: from_sidecar,
                change,
            });
        }
    }

    for (entity, element) in &exported {
        if !passes(entity) {
            continue;
        }
        let project = attribute(element, "projectName").unwrap_or_default();
        let kind = kind_of(solution, entity);
        match here.get(entity) {
            None => report.entities.push(EntityReport {
                entity: entity.clone(),
                project,
                status: Status::New,
                differences: Vec::new(),
                volatile_ids: 0,
                ignored: 0,
                kind,
                change: if base
                    .as_ref()
                    .is_some_and(|base| base.side.entities.contains_key(entity))
                {
                    Change::WeRemoved
                } else {
                    Change::Added
                },
            }),
            Some((repo_element, _)) => {
                let mut compared =
                    compare_entity(entity, element, repo_element, &solution.adopt.ignore_paths);
                compared.project = project;
                compared.kind = kind;
                compared.change = entity_change(
                    solution,
                    entity,
                    element,
                    repo_element,
                    base.as_ref()
                        .and_then(|base| base.side.entities.get(entity)),
                    here.get(entity).map(|(_, path)| path.as_path()),
                );
                report.entities.push(compared);
            }
        }
    }

    let collections: BTreeSet<&str> = exported.keys().map(|e| e.collection.as_str()).collect();
    report.absent = here
        .keys()
        .filter(|e| {
            !exported.contains_key(*e) && collections.contains(e.collection.as_str()) && passes(e)
        })
        .cloned()
        .collect();
    Ok(report)
}

fn script_change(
    theirs: &str,
    ours: &str,
    base: Option<&String>,
    history_stale: impl FnOnce() -> bool,
) -> Change {
    if canonical(theirs) == canonical(ours) {
        Change::Same
    } else if let Some(base) = base {
        classify::three_way(
            canonical(theirs) == canonical(base),
            canonical(ours) == canonical(base),
        )
    } else if history_stale() {
        Change::Stale
    } else {
        Change::Unknown
    }
}

fn entity_change(
    solution: &Solution,
    entity: &EntityRef,
    theirs: &Element,
    ours: &Element,
    base: Option<&Element>,
    source: Option<&Path>,
) -> Change {
    if entity_same(entity, theirs, ours, &solution.adopt.ignore_paths) {
        Change::Same
    } else if let Some(base) = base {
        classify::three_way(
            entity_same(entity, theirs, base, &solution.adopt.ignore_paths),
            entity_same(entity, ours, base, &solution.adopt.ignore_paths),
        )
    } else if source.is_some_and(|source| {
        history_versions(solution, source, 50).iter().any(|bytes| {
            entity_from_bytes(bytes, entity)
                .is_some_and(|old| entity_same(entity, theirs, &old, &solution.adopt.ignore_paths))
        })
    }) {
        Change::Stale
    } else {
        Change::Unknown
    }
}

fn entity_same(entity: &EntityRef, left: &Element, right: &Element, ignore: &[String]) -> bool {
    compare_entity(entity, left, right, ignore)
        .differences
        .is_empty()
}

fn entity_from_bytes(bytes: &[u8], wanted: &EntityRef) -> Option<Element> {
    normalise::parse_document(bytes)
        .ok()?
        .into_iter()
        .find_map(|node| match node {
            Node::Element(root) if root.name == b"Entities" => entities_in(root).remove(wanted),
            _ => None,
        })
}

fn kind_of(solution: &Solution, entity: &EntityRef) -> Kind {
    const UI: &[&str] = &[
        "Mashups",
        "MediaEntities",
        "StyleThemes",
        "StyleDefinitions",
        "StateDefinitions",
        "Menus",
        "Dashboards",
    ];
    if UI.contains(&entity.collection.as_str())
        || solution
            .bundle
            .ui_collections
            .iter()
            .any(|name| name == &entity.collection)
    {
        Kind::Ui
    } else {
        Kind::Backend
    }
}

fn compare_entity(
    entity: &EntityRef,
    export: &Element,
    repo: &Element,
    ignore: &[String],
) -> EntityReport {
    let (left, right) = (flatten(export), flatten(repo));
    let mut report = EntityReport {
        entity: entity.clone(),
        project: String::new(),
        status: Status::Identical,
        differences: Vec::new(),
        volatile_ids: 0,
        ignored: 0,
        kind: Kind::Backend,
        change: Change::Unknown,
    };
    let entity_path = entity.path();
    let paths: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    for path in paths {
        let (export_value, repo_value) = (left.get(path), right.get(path));
        if export_value == repo_value {
            continue;
        }
        if is_volatile_id(path) {
            report.volatile_ids += 1;
            continue;
        }
        let full = format!("{entity_path}{path}");
        if ignore.iter().any(|pattern| glob_matches(pattern, &full)) {
            report.ignored += 1;
            continue;
        }
        report.differences.push(Difference {
            path: path.clone(),
            export: export_value.cloned(),
            repo: repo_value.cloned(),
        });
    }
    if !report.differences.is_empty() {
        report.status = Status::Changed;
    }
    report
}

/// Every named entity in a flat export, keyed and ordered by `Collection/Name`.
pub(crate) fn export_entities(path: &Path) -> Result<BTreeMap<EntityRef, Element>, AdoptError> {
    let bad = |why: String| AdoptError::Export {
        path: path.to_path_buf(),
        why,
    };
    let bytes = std::fs::read(path).map_err(|e| bad(e.to_string()))?;
    let roots = normalise::parse_document(&bytes).map_err(|e| bad(e.to_string()))?;
    let root = roots
        .into_iter()
        .find_map(|node| match node {
            Node::Element(element) => Some(element),
            _ => None,
        })
        .ok_or_else(|| bad("no document element".to_string()))?;
    if root.name != b"Entities" {
        return Err(bad(format!(
            "not an <Entities> export (root is <{}>)",
            String::from_utf8_lossy(&root.name)
        )));
    }
    let found = entities_in(root);
    if found.is_empty() {
        return Err(bad("it holds no named entities".to_string()));
    }
    Ok(found)
}

/// Every entity file of the solution, parsed the same way.
pub(crate) fn repository_entities(
    solution: &Solution,
) -> Result<BTreeMap<EntityRef, (Element, PathBuf)>, AdoptError> {
    let mut found = BTreeMap::new();
    for file in workspace::discover(solution).entities {
        let bad = |why: String| AdoptError::Repository {
            path: file.path.clone(),
            why,
        };
        let bytes = std::fs::read(&file.path).map_err(|e| bad(e.to_string()))?;
        let roots = normalise::parse_document(&bytes).map_err(|e| bad(e.to_string()))?;
        for node in roots {
            if let Node::Element(root) = node {
                if root.name == b"Entities" {
                    for (entity, element) in entities_in(root) {
                        found.insert(entity, (element, file.path.clone()));
                    }
                }
            }
        }
    }
    Ok(found)
}

fn entities_in(root: Element) -> BTreeMap<EntityRef, Element> {
    let mut found = BTreeMap::new();
    for collection in child_elements(&root) {
        for entity in child_elements(collection) {
            if let Some(name) = attribute(entity, "name").filter(|n| !n.is_empty()) {
                let key = EntityRef {
                    collection: text_of(&collection.name),
                    name,
                };
                found.insert(key, entity.clone());
            }
        }
    }
    found
}

// ---- ElementTree's view of a tree ---------------------------------------------------------

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn child_elements(element: &Element) -> impl Iterator<Item = &Element> {
    element.children.iter().filter_map(|node| match node {
        Node::Element(child) => Some(child),
        _ => None,
    })
}

fn attribute(element: &Element, name: &str) -> Option<String> {
    element
        .attributes
        .iter()
        .find(|(key, _)| key == name.as_bytes())
        .map(|(_, value)| text_of(value))
}

fn is_named(element: &Element, name: &str) -> bool {
    element.name == name.as_bytes()
}

/// ElementTree's `.text`: character data before the first child element. CDATA is character
/// data; comments and processing instructions are not, and do not end the run.
fn et_text(element: &Element) -> Option<String> {
    let mut text = String::new();
    let mut any = false;
    for node in &element.children {
        match node {
            Node::Element(_) => break,
            Node::Text(bytes) | Node::Cdata(bytes) => {
                text.push_str(&String::from_utf8_lossy(bytes));
                any = true;
            }
            _ => {}
        }
    }
    any.then_some(text)
}

/// ElementTree's `itertext`: every non-empty run of character data, in document order.
fn et_itertext(element: &Element, out: &mut Vec<String>) {
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<String>| {
        if !run.is_empty() {
            out.push(std::mem::take(run));
        }
    };
    for node in &element.children {
        match node {
            Node::Text(bytes) | Node::Cdata(bytes) => run.push_str(&String::from_utf8_lossy(bytes)),
            Node::Element(child) => {
                flush(&mut run, out);
                et_itertext(child, out);
            }
            _ => {}
        }
    }
    flush(&mut run, out);
}

/// Every descendant, depth first, in document order, as ElementTree's `iter`.
fn descendants<'a>(element: &'a Element, out: &mut Vec<&'a Element>) {
    for child in child_elements(element) {
        out.push(child);
        descendants(child, out);
    }
}

// ---- comparison rules -----------------------------------------------------------------------

/// ElementTree-compatible whitespace semantics include Unicode whitespace plus
/// the four information separators.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// A value as a comparison should see it: whitespace runs collapsed, and JSON re-serialised with
/// sorted keys, so one object written two ways is one value.
pub fn canonical(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut in_space = false;
    for c in value.chars() {
        if py_space(c) {
            in_space = true;
        } else {
            if in_space && !collapsed.is_empty() {
                collapsed.push(' ');
            }
            in_space = false;
            collapsed.push(c);
        }
    }
    if collapsed.starts_with('{') || collapsed.starts_with('[') {
        if let Ok(parsed) = serde_json::from_str::<Value>(&collapsed) {
            return py_dumps(&parsed, true);
        }
    }
    collapsed
}

/// `json.dumps(value, ensure_ascii=False[, sort_keys=True])`: `", "` and `": "` separators.
fn py_dumps(value: &Value, sort_keys: bool) -> String {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if sort_keys {
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            let inner: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", py_string(k), py_dumps(v, sort_keys)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(|v| py_dumps(v, sort_keys)).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::String(text) => py_string(text),
        other => other.to_string(),
    }
}

fn py_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One entry per JSON leaf, keyed by its path, so a one-property change in a mashup's widget tree
/// names that property. Empty objects and arrays contribute nothing, as in the reference.
fn flatten_json(value: &Value, prefix: &str, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                flatten_json(item, &format!("{prefix}.{key}"), out);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                flatten_json(item, &format!("{prefix}[{index}]"), out);
            }
        }
        leaf => {
            out.insert(prefix.to_string(), py_dumps(leaf, false));
        }
    }
}

/// The value a `<Row>` identifies itself by, if it has one.
fn row_key(element: &Element) -> Option<String> {
    if !is_named(element, "Row") {
        return None;
    }
    for tag in ROW_KEY_TAGS {
        if let Some(child) = child_elements(element).find(|c| is_named(c, tag)) {
            let text = et_text(child).unwrap_or_default();
            if !text.trim_matches(py_space).is_empty() {
                return Some(canonical(&text));
            }
        }
    }
    None
}

/// One entity as a path-to-value map that survives re-serialisation. Paths are built from tag
/// names, `name` attributes and row keys, never from position, except for a repeated tag with no
/// name of its own.
pub(crate) fn flatten(element: &Element) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    flatten_into(element, "", &mut out);
    out
}

fn flatten_into(element: &Element, prefix: &str, out: &mut BTreeMap<String, String>) {
    for (key, value) in &element.attributes {
        let key = text_of(key);
        if !NOISE_ATTRS.contains(&key.as_str()) {
            out.insert(format!("{prefix}@{key}"), canonical(&text_of(value)));
        }
    }
    let text = canonical(&et_text(element).unwrap_or_default());
    if !text.is_empty() {
        let json = if is_named(element, "mashupContent")
            && (text.starts_with('{') || text.starts_with('['))
        {
            serde_json::from_str::<Value>(&text).ok()
        } else {
            None
        };
        match json {
            Some(parsed) => flatten_json(&parsed, &format!("{prefix}#json"), out),
            None => {
                out.insert(format!("{prefix}#text"), text);
            }
        }
    }

    let mut tally: BTreeMap<&[u8], usize> = BTreeMap::new();
    for child in child_elements(element) {
        *tally.entry(child.name.as_slice()).or_default() += 1;
    }
    let mut counts: BTreeMap<&[u8], usize> = BTreeMap::new();
    for child in child_elements(element) {
        let tag = text_of(&child.name);
        if NOISE_TAGS.contains(&tag.as_str()) {
            continue;
        }
        let key = attribute(child, "name")
            .filter(|n| !n.is_empty())
            .or_else(|| row_key(child));
        let step = match key {
            Some(key) => format!("{tag}[{key}]"),
            None if tally[child.name.as_slice()] > 1 => {
                let n = counts.entry(child.name.as_slice()).or_default();
                *n += 1;
                format!("{tag}[{n}]")
            }
            None => tag,
        };
        flatten_into(child, &format!("{prefix}/{step}"), out);
    }
}

/// Composer mints a fresh id for a binding or an event handler whenever it rewrites a mashup:
/// `...Events[3].Id` or `...DataBindings[12].Id`. Counted, never listed.
fn is_volatile_id(path: &str) -> bool {
    let Some(rest) = path.strip_suffix(".Id") else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(']') else {
        return false;
    };
    let Some(open) = rest.rfind('[') else {
        return false;
    };
    let digits = &rest[open + 1..];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let head = &rest[..open];
    ["Events", "DataBindings"].iter().any(|word| {
        head.strip_suffix(word)
            .is_some_and(|before| before.is_empty() || before.ends_with('.'))
    })
}

/// An `ignore_paths` glob, anchored at both ends: `*` matches within one path step, `**` across
/// steps, and every other character, brackets included, is literal. `fnmatch` would read
/// `ConfigurationTable[ConnectionInfo]` as a character class and silently match nothing.
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum Token {
        Char(char),
        Star,
        DoubleStar,
    }
    let mut tokens = Vec::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            tokens.push(Token::DoubleStar);
            i += 2;
        } else if chars[i] == '*' {
            tokens.push(Token::Star);
            i += 1;
        } else {
            tokens.push(Token::Char(chars[i]));
            i += 1;
        }
    }
    let text: Vec<char> = text.chars().collect();
    // matches[t][p]: text[t..] matches tokens[p..]. Filled from the end, so no recursion.
    let mut matches = vec![vec![false; tokens.len() + 1]; text.len() + 1];
    matches[text.len()][tokens.len()] = true;
    for t in (0..=text.len()).rev() {
        for p in (0..tokens.len()).rev() {
            matches[t][p] = match tokens[p] {
                Token::Char(c) => t < text.len() && text[t] == c && matches[t + 1][p + 1],
                Token::Star => {
                    matches[t][p + 1] || (t < text.len() && text[t] != '/' && matches[t + 1][p])
                }
                Token::DoubleStar => matches[t][p + 1] || (t < text.len() && matches[t + 1][p]),
            };
        }
    }
    matches[0][0]
}

/// Every service script on one entity, keyed by service name.
///
/// A `ServiceImplementation` wraps its script in a configuration table beside an editor-state
/// blob that changes whenever the designer scrolls. The longest text run that is not that blob
/// is the script.
pub(crate) fn service_bodies(entity: &Element) -> BTreeMap<String, String> {
    let mut all = Vec::new();
    descendants(entity, &mut all);
    let mut bodies = BTreeMap::new();
    for implementation in all
        .into_iter()
        .filter(|e| is_named(e, "ServiceImplementation"))
    {
        let Some(name) = attribute(implementation, "name").filter(|n| !n.is_empty()) else {
            continue;
        };
        let mut texts = Vec::new();
        et_itertext(implementation, &mut texts);
        let mut best: Option<String> = None;
        for text in texts {
            if text.trim_matches(py_space).starts_with("{\"viewState\"") {
                continue;
            }
            // `max` keeps the first of equally long candidates.
            if best
                .as_ref()
                .is_none_or(|b| text.chars().count() > b.chars().count())
            {
                best = Some(text);
            }
        }
        if let Some(body) = best {
            bodies.insert(name, body);
        }
    }
    bodies
}

// ---- applying the mechanical half ----------------------------------------------------------

/// What `apply` did: one line per step, in the report's order, and any declaration refresh.
pub struct ApplyOutcome {
    pub lines: Vec<String>,
    pub types: super::types::Refresh,
}

/// Adopt the report's selected changes from an export.
///
/// UI content follows the designer workflow, but a stale or unresolved conflicting export never
/// overwrites it.  Backend entities are either taken whole or merged with service sidecars: the
/// frame is selected by the three-way base and each script service is selected independently.
/// Explicit takes resolve only reported conflicts or unknowns.
///
/// Every write is worked out before the first is made, so an export that cannot be adopted
/// (a payload that is not JSON, a name that is not a file name) changes nothing. The writes are
/// then made as one transaction under `lock`: all of them or, after a failure or a crash, none.
pub fn apply(
    solution: &Solution,
    export: &Path,
    report: &Report,
    takes: &[Take],
    lock: &WorkspaceLock,
) -> Result<ApplyOutcome, AdoptError> {
    let exported = export_entities(export)?;
    let here = repository_entities(solution)?;
    validate_takes(report, takes)?;
    // In the report's order, notes and writes alike, so the output reads as the comparison did.
    let mut steps: Vec<(Option<Write>, String)> = Vec::new();
    for entry in &report.entities {
        // An identical entity has nothing to adopt and nothing worth a line.
        if entry.change == Change::Same {
            continue;
        }
        let Some(element) = exported.get(&entry.entity) else {
            continue;
        };
        let name = &entry.entity.name;
        match entry.kind {
            Kind::Ui => match entry.entity.collection.as_str() {
                "Mashups" => {
                    if !ui_should_write(report, entry, takes) {
                        steps.push((None, ui_skip_line(entry, takes)));
                        continue;
                    }
                    if !is_file_name(name) {
                        return Err(AdoptError::Export {
                            path: export.to_path_buf(),
                            why: format!(
                            "the mashup name {name:?} cannot be a file name; nothing was adopted"
                        ),
                        });
                    }
                    let dir = solution.src_root().join(name).join("mashup");
                    let assets = mashup_assets(element).map_err(|why| io_error(&dir, why))?;
                    if !here.contains_key(&entry.entity) {
                        if assets.is_none() {
                            steps.push((None, format!("skipped  Mashups/{name}: the export holds no mashup content to create it from")));
                            continue;
                        }
                        match new_mashup_file(solution, &entry.project, element, name)? {
                            Some((path, text)) => {
                                let line = format!("created  {}", relative_to(solution, &path));
                                steps.push((Some(Write::Entity(path, text)), line));
                            }
                            None => {
                                steps.push((
                                    None,
                                    format!(
                                    "skipped  Mashups/{name}: project {:?} is not in this solution",
                                    entry.project
                                ),
                                ));
                                continue;
                            }
                        }
                    }
                    if let Some(assets) = assets {
                        let current =
                            workspace::read_mashup(&dir).map_err(|e| io_error(&dir, e))?;
                        if current.as_ref() != Some(&assets) {
                            steps.push((
                                Some(Write::Sidecars(dir, assets)),
                                format!("sidecar  {name}"),
                            ));
                        }
                    }
                }
                "MediaEntities" => {
                    if !ui_should_write(report, entry, takes) {
                        steps.push((None, ui_skip_line(entry, takes)));
                        continue;
                    }
                    if let (Some((_, path)), Some(content)) = (
                        here.get(&entry.entity),
                        child_elements(element)
                            .find(|c| is_named(c, "content"))
                            .and_then(et_text),
                    ) {
                        let encoded: String = content.chars().filter(|c| !py_space(*c)).collect();
                        if encoded.is_empty() {
                            continue;
                        }
                        if !is_base64(&encoded) {
                            return Err(AdoptError::Export {
                            path: export.to_path_buf(),
                            why: format!("MediaEntities/{name}: the image is not base64; nothing was adopted"),
                        });
                        }
                        if let Some(text) = media_content(path, &encoded)? {
                            let line = format!("content  {}", relative_to(solution, path));
                            steps.push((Some(Write::Entity(path.clone(), text)), line));
                        }
                    }
                }
                _ => {
                    if !ui_should_write(report, entry, takes) {
                        steps.push((None, ui_skip_line(entry, takes)));
                    }
                }
            },
            Kind::Backend => plan_backend(
                solution, export, report, entry, element, &here, takes, &mut steps,
            )?,
        }
    }
    let mut transaction = Transaction::new(&solution.root, "adopt");
    let mut lines = Vec::new();
    for (write, line) in steps {
        match write {
            Some(Write::Entity(path, text)) => transaction
                .write_file(&path, text.into_bytes())
                .map_err(AdoptError::Write)?,
            Some(Write::Bytes(path, bytes)) => transaction
                .write_file(&path, bytes)
                .map_err(AdoptError::Write)?,
            Some(Write::Sidecars(dir, assets)) => {
                // As extract writes them: LF line endings.
                for (file, text) in [
                    ("content.json", &assets.content),
                    ("custom.css", &assets.css),
                ] {
                    transaction
                        .write_file(&dir.join(file), text.replace("\r\n", "\n").into_bytes())
                        .map_err(AdoptError::Write)?;
                }
            }
            Some(Write::Services {
                dir,
                services,
                remove,
            }) => {
                for service in services.values() {
                    for (file, text) in [
                        ("definition.xml", &service.definition),
                        ("script.js", &service.script),
                    ] {
                        let path = dir.join(&service.name).join(file);
                        let wanted = text.replace("\r\n", "\n");
                        // A sidecar we keep is left byte for byte, CRLF checkout included.
                        let current = std::fs::read_to_string(&path).ok();
                        if current.is_some_and(|current| current.replace("\r\n", "\n") == wanted) {
                            continue;
                        }
                        transaction
                            .write_file(&path, wanted.into_bytes())
                            .map_err(AdoptError::Write)?;
                    }
                }
                for service in remove {
                    let folder = dir.join(service);
                    for file in [
                        "definition.xml",
                        "script.js",
                        "jsconfig.json",
                        "twaco-globals.d.ts",
                    ] {
                        let path = folder.join(file);
                        if let Ok(bytes) = std::fs::read(&path) {
                            transaction
                                .delete_file(&path, &bytes)
                                .map_err(AdoptError::Write)?;
                        }
                    }
                }
            }
            None => {}
        }
        lines.push(line);
    }
    let wrote = !transaction.is_empty();
    tracing::info!(writes = transaction.len(), "adopt: writing");
    if wrote {
        transaction.apply(lock).map_err(AdoptError::Write)?;
    }
    tracing::info!(wrote, "adopt: done");
    Ok(ApplyOutcome {
        lines,
        types: super::types::refresh_after_write(solution, wrote),
    })
}

/// One file or sidecar folder `apply` will write, worked out in full beforehand.
enum Write {
    Entity(PathBuf, String),
    Bytes(PathBuf, Vec<u8>),
    Sidecars(PathBuf, super::mashup::Assets),
    Services {
        dir: PathBuf,
        services: BTreeMap<String, sidecar::ServiceSidecar>,
        remove: Vec<String>,
    },
}

fn has_take(takes: &[Take], side: TakeSide, target: &str) -> bool {
    takes
        .iter()
        .any(|take| take.side == side && take.target == target)
}

fn validate_takes(report: &Report, takes: &[Take]) -> Result<(), AdoptError> {
    let known: BTreeSet<String> = report
        .entities
        .iter()
        .filter(|entry| matches!(entry.change, Change::Conflict | Change::Unknown))
        .map(|entry| entry.entity.name.clone())
        .chain(
            report
                .services
                .iter()
                .filter(|service| matches!(service.change, Change::Conflict | Change::Unknown))
                .map(|service| format!("{}.{}", service.entity, service.service)),
        )
        .collect();
    let bad: Vec<String> = takes
        .iter()
        .filter(|take| !known.contains(&take.target))
        .map(|take| take.target.clone())
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        let available = known.into_iter().collect::<Vec<_>>().join(", ");
        Err(AdoptError::Take(format!(
            "{}; conflicts and unknowns that can be taken: {available}",
            bad.join(", ")
        )))
    }
}

fn ui_should_write(report: &Report, entry: &EntityReport, takes: &[Take]) -> bool {
    matches!(entry.change, Change::Theirs | Change::Added)
        || has_take(takes, TakeSide::Theirs, &entry.entity.name)
        || (report.base.is_none() && entry.change == Change::Unknown)
}

fn ui_skip_line(entry: &EntityReport, takes: &[Take]) -> String {
    let path = entry.entity.path();
    match entry.change {
        Change::Stale => format!("kept     {path}: stale, ours is newer"),
        Change::Conflict | Change::Unknown
            if !has_take(takes, TakeSide::Theirs, &entry.entity.name) =>
        {
            format!(
                "conflict {path}: both changed; --take theirs:{} or --take ours:{}",
                entry.entity.name, entry.entity.name
            )
        }
        Change::WeRemoved => format!("skipped  {path}: we removed it"),
        Change::Same => format!("skipped  {path}: unchanged"),
        _ => format!("skipped  {path}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_backend(
    solution: &Solution,
    export: &Path,
    report: &Report,
    entry: &EntityReport,
    _element: &Element,
    here: &BTreeMap<EntityRef, (Element, PathBuf)>,
    takes: &[Take],
    steps: &mut Vec<(Option<Write>, String)>,
) -> Result<(), AdoptError> {
    let entity = &entry.entity;
    let entity_name = &entity.name;
    let their_raw = raw_entity(export, entity)?;
    let their_services = services_from_document(&entity_document(&entity.collection, &their_raw))?;
    let service_dir = solution.src_root().join(entity_name).join("services");

    if entry.change == Change::Added {
        let Some(path) = new_backend_path(solution, entry, here)? else {
            steps.push((
                None,
                format!(
                    "skipped  {}: project {:?} is not in this solution",
                    entity.path(),
                    entry.project
                ),
            ));
            return Ok(());
        };
        let bytes = if let Some((sample_key, (_, sample))) =
            here.iter().find(|(key, (value, _))| {
                key.collection == entity.collection
                    && attribute(value, "projectName").as_deref() == Some(entry.project.as_str())
            }) {
            // Shaped like a file of its own collection and project: that file's entity is
            // swapped for the new one, so the declaration and wrapper match the solution's.
            let old = std::fs::read(sample).map_err(|e| io_error(sample, e))?;
            replace_entity_bytes(&old, sample_key, &their_raw)?
        } else {
            entity_document(&entity.collection, &their_raw)
        };
        steps.push((
            Some(Write::Bytes(path.clone(), bytes)),
            format!("created  {}", relative_to(solution, &path)),
        ));
        steps.push((
            Some(Write::Services {
                dir: service_dir,
                services: their_services,
                remove: Vec::new(),
            }),
            format!("sidecar  {entity_name}"),
        ));
        return Ok(());
    }

    let Some((ours_element, path)) = here.get(entity) else {
        steps.push((None, format!("skipped  {}: we removed it", entity.path())));
        return Ok(());
    };
    let ours_raw = std::fs::read(path).map_err(|e| io_error(path, e))?;
    let ours_services = ours_services(solution, entity_name, &ours_raw)?;
    let their_entity_take = has_take(takes, TakeSide::Theirs, entity_name);

    let generated_here = report
        .services
        .iter()
        .any(|service| service.entity == *entity_name && service.generated);
    if (entry.change == Change::Theirs && !generated_here)
        || (entry.change == Change::Unknown && report.base.is_none() && their_entity_take)
    {
        let bytes = replace_entity_bytes(&ours_raw, entity, &their_raw)?;
        let remove = stale_service_dirs(&service_dir, &their_services);
        steps.push((
            Some(Write::Bytes(path.clone(), bytes)),
            format!("replaced {}", relative_to(solution, path)),
        ));
        steps.push((
            Some(Write::Services {
                dir: service_dir,
                services: their_services,
                remove,
            }),
            format!("sidecar  {entity_name}"),
        ));
        return Ok(());
    }
    if matches!(
        entry.change,
        Change::Stale | Change::Same | Change::WeRemoved
    ) {
        steps.push((
            None,
            match entry.change {
                Change::Stale => format!("kept     {}: stale, ours is newer", entity.path()),
                Change::WeRemoved => format!("skipped  {}: we removed it", entity.path()),
                _ => format!("skipped  {}: unchanged", entity.path()),
            },
        ));
        return Ok(());
    }
    if entry.change == Change::Unknown && report.base.is_none() {
        steps.push((
            None,
            format!(
                "conflict {}: both changed; --take theirs:{} or --take ours:{}",
                entity.path(),
                entity_name,
                entity_name
            ),
        ));
        return Ok(());
    }

    let base = report
        .base_side
        .as_ref()
        .and_then(|side| side.entities.get(entity));
    let theirs_frame_is_base = base.is_some_and(|base| same_frame(_element, base));
    let ours_frame_is_base = base.is_some_and(|base| same_frame(ours_element, base));
    let frame_theirs = their_entity_take || (!theirs_frame_is_base && ours_frame_is_base);
    let frame_conflict = !theirs_frame_is_base && !ours_frame_is_base;
    let mut chosen = BTreeMap::new();
    let mut took = 0usize;
    let mut kept_stale = 0usize;
    let service_changes: BTreeMap<String, &ServiceReport> = report
        .services
        .iter()
        .filter(|service| service.entity == *entity_name)
        .map(|service| (service.service.clone(), service))
        .collect();
    let names: BTreeSet<String> = ours_services
        .keys()
        .chain(their_services.keys())
        .cloned()
        .collect();
    for name in names {
        let change = service_changes
            .get(&name)
            .map_or(Change::Same, |service| service.change);
        let target = format!("{entity_name}.{name}");
        let generated = service_changes
            .get(&name)
            .is_some_and(|service| service.generated);
        let choose_theirs = !generated
            && (matches!(change, Change::Theirs | Change::Added)
                || has_take(takes, TakeSide::Theirs, &target));
        if change == Change::WeRemoved {
            continue;
        }
        if choose_theirs {
            if let Some(service) = their_services.get(&name) {
                chosen.insert(name.clone(), service.clone());
                took += 1;
            }
        } else if let Some(service) = ours_services.get(&name) {
            chosen.insert(name.clone(), service.clone());
            if change == Change::Stale || has_take(takes, TakeSide::Ours, &target) {
                kept_stale += 1;
            }
            if matches!(change, Change::Conflict | Change::Unknown)
                && !has_take(takes, TakeSide::Ours, &target)
            {
                steps.push((
                    None,
                    format!("conflict {target}: both changed; --take theirs:{target} or --take ours:{target}"),
                ));
            } else if change == Change::Stale {
                steps.push((None, format!("kept     {target}: stale")));
            } else if has_take(takes, TakeSide::Ours, &target) {
                steps.push((None, format!("kept     {target}: ours")));
            }
        }
    }
    let their_document = entity_document(&entity.collection, &their_raw);
    let seed = if frame_theirs {
        their_document.as_slice()
    } else {
        ours_raw.as_slice()
    };
    let merged = sync::sync(
        seed,
        &chosen,
        true,
        solution.format.indent_cdata_payload,
        false,
    )
    .map_err(|error| io_error(path, error))?
    .0;
    let output = if frame_theirs {
        let merged_entity = entity_span(
            &scan::tokenize(&merged).map_err(|error| io_error(path, error))?,
            &merged,
            entity,
        )
        .map(|span| span.of(&merged).to_vec())
        .ok_or_else(|| io_error(path, "the merged entity disappeared"))?;
        replace_entity_bytes(&ours_raw, entity, &merged_entity)?
    } else {
        merged
    };
    if output != ours_raw {
        let remove = stale_service_dirs(&service_dir, &chosen);
        let frame = if frame_theirs { "theirs" } else { "ours" };
        let suffix = if frame_conflict && !their_entity_take && !frame_theirs {
            "; frame conflict"
        } else {
            ""
        };
        steps.push((Some(Write::Bytes(path.clone(), output)), format!("merged   {entity_name}: took {took} services, kept {kept_stale} (stale), frame {frame}{suffix}")));
        steps.push((
            Some(Write::Services {
                dir: service_dir,
                services: chosen,
                remove,
            }),
            format!("sidecar  {entity_name}"),
        ));
    }
    Ok(())
}

fn same_frame(left: &Element, right: &Element) -> bool {
    let frame = |element: &Element| {
        flatten(element)
            .into_iter()
            .filter(|(path, _)| {
                !path.contains("/ServiceDefinitions") && !path.contains("/ServiceImplementations")
            })
            .collect::<BTreeMap<_, _>>()
    };
    frame(left) == frame(right)
}

fn services_from_document(
    bytes: &[u8],
) -> Result<BTreeMap<String, sidecar::ServiceSidecar>, AdoptError> {
    sidecar::extract(bytes)
        .map(|extraction| {
            extraction
                .services
                .into_iter()
                .map(|service| (service.name.clone(), service))
                .collect()
        })
        .map_err(|error| AdoptError::Export {
            path: PathBuf::from("export entity"),
            why: error.to_string(),
        })
}

fn ours_services(
    solution: &Solution,
    entity: &str,
    bytes: &[u8],
) -> Result<BTreeMap<String, sidecar::ServiceSidecar>, AdoptError> {
    let mut services = services_from_document(bytes)?;
    for service in services.values_mut() {
        let script = solution
            .src_root()
            .join(entity)
            .join("services")
            .join(&service.name)
            .join("script.js");
        if script.is_file() {
            service.script = std::fs::read_to_string(&script).map_err(|e| io_error(&script, e))?;
        }
    }
    Ok(services)
}

fn stale_service_dirs(
    dir: &Path,
    wanted: &BTreeMap<String, sidecar::ServiceSidecar>,
) -> Vec<String> {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !wanted.contains_key(name))
        .collect()
}

fn new_backend_path(
    solution: &Solution,
    entry: &EntityReport,
    here: &BTreeMap<EntityRef, (Element, PathBuf)>,
) -> Result<Option<PathBuf>, AdoptError> {
    let Some(project) = solution.project(&entry.project) else {
        return Ok(None);
    };
    let folder = here
        .iter()
        .find_map(|(key, (element, path))| {
            (key.collection == entry.entity.collection
                && attribute(element, "projectName").as_deref() == Some(entry.project.as_str()))
            .then(|| path.parent().map(Path::to_path_buf))
            .flatten()
        })
        .unwrap_or_else(|| {
            solution
                .project_root(project)
                .join(&entry.entity.collection)
        });
    let path = folder.join(format!("{}.xml", entry.entity.name));
    if path.exists() {
        return Err(io_error(
            &path,
            "already exists, though the comparison found no such entity",
        ));
    }
    Ok(Some(path))
}

fn entity_document(collection: &str, entity: &[u8]) -> Vec<u8> {
    [
        b"<Entities><".as_slice(),
        collection.as_bytes(),
        b">",
        entity,
        b"</",
        collection.as_bytes(),
        b"></Entities>",
    ]
    .concat()
}

fn raw_entity(path: &Path, wanted: &EntityRef) -> Result<Vec<u8>, AdoptError> {
    let bytes = std::fs::read(path).map_err(|e| io_error(path, e))?;
    let tokens = scan::tokenize(&bytes).map_err(|e| io_error(path, e))?;
    entity_span(&tokens, &bytes, wanted)
        .map(|span| span.of(&bytes).to_vec())
        .ok_or_else(|| io_error(path, format!("no {} in export", wanted.path())))
}

fn replace_entity_bytes(
    src: &[u8],
    wanted: &EntityRef,
    entity: &[u8],
) -> Result<Vec<u8>, AdoptError> {
    let tokens = scan::tokenize(src).map_err(|e| io_error(Path::new("entity"), e))?;
    let span = entity_span(&tokens, src, wanted).ok_or_else(|| {
        io_error(
            Path::new("entity"),
            format!("no {} in entity file", wanted.path()),
        )
    })?;
    splice::splice(src, &[splice::Edit::new(span, entity.to_vec())])
        .map_err(|e| io_error(Path::new("entity"), e))
}

fn entity_span(tokens: &[scan::Token], src: &[u8], wanted: &EntityRef) -> Option<scan::Span> {
    let root = tokens.iter().position(|token| {
        matches!(token.kind, scan::Kind::Start) && token.name.of(src) == b"Entities"
    })?;
    let root_end = scan::element_end(tokens, root)?;
    let mut at = root + 1;
    while at < root_end {
        if matches!(tokens[at].kind, scan::Kind::Start)
            && tokens[at].name.of(src) == wanted.collection.as_bytes()
        {
            let end = scan::element_end(tokens, at)?;
            let mut child = at + 1;
            while child < end {
                if matches!(tokens[child].kind, scan::Kind::Start | scan::Kind::Empty)
                    && scan::attribute(src, &tokens[child], "name")
                        .ok()
                        .flatten()
                        .is_some_and(|name| {
                            scan::decode_entities(&String::from_utf8_lossy(name.of(src)))
                                == wanted.name
                        })
                {
                    return scan::element_span(tokens, child);
                }
                child = if tokens[child].kind == scan::Kind::Start {
                    scan::element_end(tokens, child).map_or(end, |at| at + 1)
                } else {
                    child + 1
                };
            }
        }
        at = if tokens[at].kind == scan::Kind::Start {
            scan::element_end(tokens, at).map_or(root_end, |at| at + 1)
        } else {
            at + 1
        };
    }
    None
}

/// Whether an entity name can be used as one file or folder name on every platform: an export
/// is data from elsewhere, and `..`, a separator or a device name would write outside the tree.
fn is_file_name(name: &str) -> bool {
    const DEVICES: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        && !DEVICES.contains(&stem.as_str())
}

fn is_base64(encoded: &str) -> bool {
    let body = encoded.trim_end_matches('=');
    encoded.len().is_multiple_of(4)
        && encoded.len() - body.len() <= 2
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

fn relative_to(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

fn io_error(path: &Path, error: impl fmt::Display) -> AdoptError {
    AdoptError::Repository {
        path: path.to_path_buf(),
        why: error.to_string(),
    }
}

/// A mashup's content and stylesheet, as its sidecars hold them; `None` when the export has none.
fn mashup_assets(element: &Element) -> Result<Option<super::mashup::Assets>, String> {
    let Some(text) = child_elements(element)
        .find(|c| is_named(c, "mashupContent"))
        .and_then(et_text)
    else {
        return Ok(None);
    };
    if text.trim_matches(py_space).is_empty() {
        return Ok(None);
    }
    super::mashup::assets_from_payload(&text)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// The entity file for a mashup the repository does not have yet: the first mashup of its own
/// project, with the name and `ParameterDefinitions` replaced. Every mashup shares one attribute
/// set and one `MobileSettings` table, so the project's own file is the shape, not a template
/// held here that could drift from it. `None` when the project is not in this solution.
fn new_mashup_file(
    solution: &Solution,
    project: &str,
    element: &Element,
    name: &str,
) -> Result<Option<(PathBuf, String)>, AdoptError> {
    let Some(project) = solution.project(project) else {
        return Ok(None);
    };
    let directory = solution.project_root(project).join("Mashups");
    let target = directory.join(format!("{name}.xml"));
    if target.exists() {
        return Err(io_error(
            &target,
            "already exists, though the comparison found no such mashup",
        ));
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&directory)
        .map_err(|e| io_error(&directory, e))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "xml"))
        .collect();
    candidates.sort();
    let skeleton_path = candidates
        .first()
        .ok_or_else(|| io_error(&directory, "no mashup to take the entity shape from"))?;
    let skeleton =
        std::fs::read_to_string(skeleton_path).map_err(|e| io_error(skeleton_path, e))?;

    // The shape is attribute-per-line; the entity's own name is the one on a line of its own.
    // The first line that is exactly `         name="<value>"`, as required by the legacy format.
    // Only that attribute is replaced: another one that happens to hold the same text is not
    // the entity's name.
    let marker = "\n         name=\"";
    let (start, end) = skeleton
        .match_indices(marker)
        .find_map(|(at, _)| {
            let start = at + marker.len();
            let end = start + skeleton[start..].find('"')?;
            (end > start && skeleton[end + 1..].starts_with('\n')).then_some((start, end))
        })
        .ok_or_else(|| {
            io_error(
                skeleton_path,
                "cannot find the name attribute in the mashup shape",
            )
        })?;
    let text = format!(
        "{}{}{}",
        &skeleton[..start],
        super::scan::escape_attribute(name),
        &skeleton[end..]
    );

    let open = "            <ParameterDefinitions>";
    let close = "</ParameterDefinitions>";
    let start = text
        .find(open)
        .ok_or_else(|| io_error(skeleton_path, "no ParameterDefinitions to replace"))?;
    let end = text[start..]
        .find(close)
        .map(|at| start + at + close.len())
        .ok_or_else(|| io_error(skeleton_path, "ParameterDefinitions is not closed"))?;
    let text = format!(
        "{}{}{}",
        &text[..start],
        render_parameter_definitions(element),
        &text[end..]
    );
    Ok(Some((target, text)))
}

/// An export's `ParameterDefinitions` in the repository's attribute-per-line style.
fn render_parameter_definitions(element: &Element) -> String {
    let fields: Vec<&Element> = child_elements(element)
        .find(|c| is_named(c, "ParameterDefinitions"))
        .map(|definitions| child_elements(definitions).collect())
        .unwrap_or_default();
    if fields.is_empty() {
        return "            <ParameterDefinitions></ParameterDefinitions>".to_string();
    }
    let mut lines = vec!["            <ParameterDefinitions>".to_string()];
    for field in fields {
        lines.push("                <FieldDefinition".to_string());
        // Attributes arrive sorted by name, which is the order the repository writes them in.
        let count = field.attributes.len();
        for (index, (key, value)) in field.attributes.iter().enumerate() {
            let escaped = super::scan::escape_attribute(&text_of(value));
            let tail = if index + 1 == count {
                "></FieldDefinition>"
            } else {
                ""
            };
            lines.push(format!(
                "                 {}=\"{escaped}\"{tail}",
                text_of(key)
            ));
        }
    }
    lines.push("            </ParameterDefinitions>".to_string());
    lines.join("\n")
}

/// A MediaEntity's file with its base64 payload replaced, wrapped at 76 characters inside its
/// CDATA, as the repository stores it. `None` when that is what the file already holds.
fn media_content(path: &Path, encoded: &str) -> Result<Option<String>, AdoptError> {
    let text = std::fs::read_to_string(path).map_err(|e| io_error(path, e))?;
    let start = text
        .find("<content>")
        .ok_or_else(|| io_error(path, "no <content> element"))?;
    let end = text[start..]
        .find("]]>")
        .map(|at| start + at)
        .ok_or_else(|| io_error(path, "<content> has no CDATA"))?;
    let indent = " ".repeat(12);
    let chars: Vec<char> = encoded.chars().collect();
    let wrapped: Vec<String> = chars
        .chunks(76)
        .map(|chunk| format!("{indent}{}", chunk.iter().collect::<String>()))
        .collect();
    let rebuilt = format!(
        "{}<content>\n{indent}<![CDATA[\n{}\n{indent}{}",
        &text[..start],
        wrapped.join("\n"),
        &text[end..]
    );
    Ok((rebuilt != text).then_some(rebuilt))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(src: &str) -> Element {
        match normalise::parse_document(src.as_bytes())
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
        {
            Node::Element(e) => e,
            _ => panic!("not an element"),
        }
    }

    #[test]
    fn canonical_settles_whitespace_and_json_key_order() {
        assert_eq!(canonical("  a \n\t b  "), "a b");
        assert_eq!(
            canonical("{\"b\": 1,\n \"a\": [1, 2]}"),
            "{\"a\": [1, 2], \"b\": 1}"
        );
        assert_eq!(canonical("{not json"), "{not json");
        assert_eq!(
            canonical("x\u{1f}y"),
            "x y",
            "Unicode separators are whitespace"
        );
    }

    #[test]
    fn element_text_is_what_precedes_the_first_child_cdata_included() {
        let e = element("<a> one <![CDATA[two]]><!-- c --> three<b>x</b>tail</a>");
        assert_eq!(et_text(&e).unwrap(), " one two three");
        let mut texts = Vec::new();
        et_itertext(&e, &mut texts);
        assert_eq!(texts, [" one two three", "x", "tail"]);
    }

    #[test]
    fn paths_use_names_and_row_keys_not_positions() {
        let e = element(
            "<Thing name=\"T\" lastModifiedDate=\"x\"><Owner name=\"me\"/><ConfigurationTables>\
             <ConfigurationTable name=\"CT\"><Rows>\
             <Row><UID>2</UID><v>b</v></Row><Row><UID>1</UID><v>a</v></Row>\
             </Rows></ConfigurationTable></ConfigurationTables><x>1</x><x>2</x></Thing>",
        );
        let flat = flatten(&e);
        assert_eq!(flat.get("@name").map(String::as_str), Some("T"));
        assert!(!flat
            .keys()
            .any(|k| k.contains("lastModifiedDate") || k.contains("Owner")));
        assert_eq!(
            flat.get("/ConfigurationTables/ConfigurationTable[CT]/Rows/Row[1]/v#text")
                .map(String::as_str),
            Some("a")
        );
        assert_eq!(flat.get("/x[2]#text").map(String::as_str), Some("2"));
    }

    #[test]
    fn mashup_json_is_compared_leaf_by_leaf() {
        let e = element("<Mashup><mashupContent><![CDATA[{\"UI\": {\"Properties\": {\"Width\": 10, \"Id\": \"a\"}, \"Empty\": {}}}]]></mashupContent></Mashup>");
        let flat = flatten(&e);
        assert_eq!(
            flat.get("/mashupContent#json.UI.Properties.Width")
                .map(String::as_str),
            Some("10")
        );
        assert_eq!(
            flat.get("/mashupContent#json.UI.Properties.Id")
                .map(String::as_str),
            Some("\"a\"")
        );
        assert!(
            !flat.keys().any(|k| k.contains("Empty")),
            "an empty object contributes nothing"
        );
    }

    #[test]
    fn regenerated_ids_are_recognised_and_nothing_else() {
        assert!(is_volatile_id("/mashupContent#json.Events[3].Id"));
        assert!(is_volatile_id("/mashupContent#json.UI.DataBindings[12].Id"));
        assert!(!is_volatile_id("/mashupContent#json.Events[3].Name"));
        assert!(!is_volatile_id("/mashupContent#json.MyEvents[3].Id"));
        assert!(!is_volatile_id("/mashupContent#json.Events[x].Id"));
    }

    #[test]
    fn globs_keep_brackets_literal_and_star_within_a_step() {
        let pattern = "Things/*/ConfigurationTables/ConfigurationTable[ConnectionInfo]/**";
        assert!(glob_matches(pattern, "Things/Db/ConfigurationTables/ConfigurationTable[ConnectionInfo]/Rows/Row/password#text"));
        assert!(!glob_matches(
            pattern,
            "Things/Db/ConfigurationTables/ConfigurationTable[Other]/Rows"
        ));
        assert!(
            !glob_matches(
                pattern,
                "Things/a/b/ConfigurationTables/ConfigurationTable[ConnectionInfo]/x"
            ),
            "* stays in one step"
        );
        assert!(glob_matches(
            "Things/*/ThingProperties/**",
            "Things/T/ThingProperties/p/Value#text"
        ));
        assert!(
            !glob_matches("Things/*", "Things/T/extra"),
            "anchored at the end"
        );
    }

    #[test]
    fn parameter_definitions_render_one_attribute_per_line_escaped() {
        let e = element(
            "<Mashup><ParameterDefinitions><FieldDefinition name=\"p\" description=\"a &lt;b&gt; &amp; &quot;c&quot;\"/>\
             </ParameterDefinitions></Mashup>",
        );
        assert_eq!(
            render_parameter_definitions(&e),
            "            <ParameterDefinitions>\n                <FieldDefinition\n                 \
             description=\"a &lt;b&gt; &amp; &quot;c&quot;\"\n                 name=\"p\"></FieldDefinition>\n            \
             </ParameterDefinitions>"
        );
        assert_eq!(
            render_parameter_definitions(&element("<Mashup/>")),
            "            <ParameterDefinitions></ParameterDefinitions>"
        );
    }

    fn locked(solution: &Solution) -> WorkspaceLock {
        crate::core::lock::acquire(&solution.root, "test", &[]).unwrap()
    }

    /// A one-project solution with one mashup to take the shape from, and an export of `mashups`
    /// (name, mashupContent) next to it.
    fn adopt_case(mashups: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, Solution, PathBuf) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-adopt-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Mashups")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("Mashups/P.A.xml"),
            "<Entities>\n    <Mashups>\n        <Mashup\n         aspect.isFlex=\"false\"\n         \
             name=\"P.A\"\n         projectName=\"P\">\n            <ParameterDefinitions>\
             </ParameterDefinitions>\n            <mashupContent><![CDATA[{\"UI\":{}}]]></mashupContent>\n\
             \x20       </Mashup>\n    </Mashups>\n</Entities>\n",
        )
        .unwrap();
        let mut export = String::from("<Entities><Mashups>");
        export.push_str("<Mashup name=\"P.A\" projectName=\"P\"><mashupContent><![CDATA[{\"UI\":{}}]]></mashupContent></Mashup>");
        for (name, content) in mashups {
            export.push_str(&format!(
                "<Mashup name=\"{}\" projectName=\"P\"><mashupContent><![CDATA[{content}]]></mashupContent></Mashup>",
                name.replace('&', "&amp;")
            ));
        }
        export.push_str("</Mashups></Entities>");
        let export_path = root.join("export.xml");
        std::fs::write(&export_path, export).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution, export_path)
    }

    fn files_under(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path())
                } else {
                    found.push(entry.path())
                }
            }
        }
        found.sort();
        found
    }

    #[test]
    fn a_write_that_cannot_happen_leaves_none_of_the_others_behind() {
        let (_dir, root, solution, export) = adopt_case(&[("P.B", "{\"UI\":{\"x\":1}}")]);
        // A file where the new mashup's sidecar folder must go: its entity file is written
        // first, and used to stay when the sidecars then failed.
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/P.B"), "in the way").unwrap();
        let report = compare(&solution, &export, &[]).unwrap();
        let outside_twaco = |root: &Path| -> Vec<(PathBuf, Vec<u8>)> {
            files_under(root)
                .into_iter()
                .filter(|path| !path.starts_with(root.join(".twaco")))
                .map(|path| {
                    let bytes = std::fs::read(&path).unwrap();
                    (path, bytes)
                })
                .collect()
        };
        let before = outside_twaco(&root);
        let error = apply(&solution, &export, &report, &[], &locked(&solution))
            .err()
            .expect("the sidecar folder cannot be made");
        // Where it fails depends on the platform: Windows reads `src/P.B/mashup/content.json` as
        // missing, so the write is planned and the transaction undoes it; Unix says a file is in
        // the way (ENOTDIR) while planning. Either way nothing may be left behind.
        match &error {
            AdoptError::Write(_) => assert!(
                error.to_string().starts_with("nothing was adopted"),
                "{error}"
            ),
            AdoptError::Repository { .. } => {}
            other => panic!("unexpected {other}"),
        }
        assert_eq!(outside_twaco(&root), before, "every change was undone");
        assert!(!root.join("Mashups/P.B.xml").exists());
    }

    #[test]
    fn a_new_mashup_is_created_under_its_own_name_escaped_and_only_there() {
        let (_dir, root, solution, export) = adopt_case(&[("P.B&C", "{\"UI\":{\"x\":1}}")]);
        std::fs::create_dir_all(root.join(".twaco/types")).unwrap();
        let report = compare(&solution, &export, &[]).unwrap();
        let outcome = apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
        assert!(outcome.types.files_written.is_some());
        let lines: Vec<String> = outcome
            .lines
            .into_iter()
            .filter(|l| !l.ends_with("P.A"))
            .collect();
        assert_eq!(lines, ["created  Mashups/P.B&C.xml", "sidecar  P.B&C"]);
        let text = std::fs::read_to_string(root.join("Mashups/P.B&C.xml")).unwrap();
        assert!(text.contains("\n         name=\"P.B&amp;C\"\n"), "{text}");
        assert!(root.join("src/P.B&C/mashup/content.json").is_file());
    }

    #[test]
    fn an_export_that_cannot_be_adopted_whole_changes_nothing() {
        for bad in [
            ("P.C", "not json"),
            ("../escape", "{\"UI\":{}}"),
            ("CON", "{\"UI\":{}}"),
        ] {
            let (_dir, root, solution, export) = adopt_case(&[("P.B", "{\"UI\":{\"x\":1}}"), bad]);
            let lock = locked(&solution);
            let before = files_under(&root);
            let report = compare(&solution, &export, &[]).unwrap();
            assert!(
                apply(&solution, &export, &report, &[], &lock).is_err(),
                "{bad:?}"
            );
            assert_eq!(files_under(&root), before, "{bad:?} wrote something");
            assert!(!root.parent().unwrap().join("escape").exists());
        }
    }

    #[test]
    fn a_new_mashup_without_content_is_skipped_not_given_another_ones() {
        let (_dir, root, solution, export) = adopt_case(&[("P.D", "  ")]);
        let report = compare(&solution, &export, &[]).unwrap();
        let lines: Vec<String> = apply(&solution, &export, &report, &[], &locked(&solution))
            .unwrap()
            .lines
            .into_iter()
            .filter(|l| !l.ends_with("P.A"))
            .collect();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with("skipped  Mashups/P.D: the export holds no mashup content"),
            "{lines:?}"
        );
        assert!(!root.join("Mashups/P.D.xml").exists());
    }

    #[test]
    fn lines_come_in_the_reports_order() {
        let (_dir, _, solution, export) =
            adopt_case(&[("P.B", "{\"UI\":{\"x\":1}}"), ("P.C", " ")]);
        let report = compare(&solution, &export, &[]).unwrap();
        let lines: Vec<String> = apply(&solution, &export, &report, &[], &locked(&solution))
            .unwrap()
            .lines
            .into_iter()
            .filter(|l| !l.ends_with("P.A"))
            .collect();
        let kinds: Vec<&str> = lines
            .iter()
            .map(|l| l.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(kinds, ["created", "sidecar", "skipped"], "{lines:?}");
    }

    #[test]
    fn names_and_images_are_checked_before_use() {
        for good in ["P.Mashup_1", "Acme.Dash-Board.X", "P.B&C", "Console"] {
            assert!(is_file_name(good), "{good}");
        }
        for bad in [
            "", ".", "..", "../x", "a/b", "a\\b", "C:x", "x.", "x ", "NUL", "com1.txt", "a\u{1}b",
        ] {
            assert!(!is_file_name(bad), "{bad:?}");
        }
        assert!(is_base64("QUJD") && is_base64("QUI=") && is_base64("QQ=="));
        assert!(!is_base64("QUJ") && !is_base64("Q===") && !is_base64("QU!D"));
    }

    #[test]
    fn media_content_is_rewrapped_and_an_unchanged_file_is_not_written() {
        let dir_guard = tempfile::Builder::new()
            .prefix("twaco-media-")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("M.xml");
        std::fs::write(&path, "<a>\n        <content>\n            <![CDATA[\n            old\n            ]]>\n        </content>\n</a>").unwrap();
        let encoded = "A".repeat(80);
        let text = media_content(&path, &encoded)
            .unwrap()
            .expect("new content changes the file");
        let expected_body = format!("            {}\n            {}", "A".repeat(76), "AAAA");
        assert!(
            text.contains(&format!(
                "<content>\n            <![CDATA[\n{expected_body}\n            ]]>"
            )),
            "{text}"
        );
        std::fs::write(&path, &text).unwrap();
        assert_eq!(
            media_content(&path, &encoded).unwrap(),
            None,
            "the same content is not rewritten"
        );
    }

    #[test]
    fn a_service_body_is_the_longest_text_that_is_not_editor_state() {
        let e = element(
            "<Thing><ThingShape><ServiceImplementations><ServiceImplementation name=\"S\"><ConfigurationTables>\
             <ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[result = 1;]]></code>\
             <editor><![CDATA[{\"viewState\": {\"cursor\": \"far longer than the script itself\"}}]]></editor>\
             </Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation>\
             </ServiceImplementations></ThingShape></Thing>",
        );
        let bodies = service_bodies(&e);
        assert_eq!(bodies.get("S").map(String::as_str), Some("result = 1;"));
    }
}
