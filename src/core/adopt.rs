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

use super::config::Solution;
use super::lock::WorkspaceLock;
use super::normalise::{self, Element, Node};
use super::transaction::Transaction;
use super::workspace;
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
}

#[derive(Debug, Default)]
pub struct Report {
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

    /// Service differences that are not generated: each is a revert or a change to adopt.
    pub fn reverts(&self) -> impl Iterator<Item = &ServiceReport> {
        self.services.iter().filter(|service| !service.generated)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    #[error("cannot read export {}: {why}", .path.display())]
    Export { path: PathBuf, why: String },
    #[error("cannot read {}: {why}", .path.display())]
    Repository { path: PathBuf, why: String },
    /// The writes, made as one transaction, did not happen.
    #[error("nothing was adopted: {0}")]
    Write(super::transaction::TransactionError),
}

/// Compare an export with the solution. `only` narrows the entity comparison, not the service
/// check, to names containing any of its fragments, preserving the established filter semantics.
pub fn compare(solution: &Solution, export: &Path, only: &[String]) -> Result<Report, AdoptError> {
    let exported = export_entities(export)?;
    let here = repository_entities(solution)?;
    let mut report = Report::default();

    let generated: BTreeSet<&str> = solution
        .adopt
        .generated_services
        .iter()
        .map(String::as_str)
        .collect();
    let src_root = solution.src_root();
    for (entity, element) in &exported {
        let mut repo_bodies: Option<BTreeMap<String, String>> = None;
        for (service, body) in service_bodies(element) {
            let label = format!("{}.{service}", entity.name);
            let sidecar = src_root
                .join(&entity.name)
                .join("services")
                .join(&service)
                .join("script.js");
            let (mine, source, from_sidecar) = if sidecar.is_file() {
                let text =
                    std::fs::read_to_string(&sidecar).map_err(|e| AdoptError::Repository {
                        path: sidecar.clone(),
                        why: e.to_string(),
                    })?;
                (text, sidecar, true)
            } else {
                let Some((repo_element, path)) = here.get(entity) else {
                    report.unmatched_services.push(label);
                    continue;
                };
                let bodies = repo_bodies.get_or_insert_with(|| service_bodies(repo_element));
                let Some(mine) = bodies.get(&service) else {
                    report.unmatched_services.push(label);
                    continue;
                };
                (mine.clone(), path.clone(), false)
            };
            if canonical(&body) != canonical(&mine) {
                report.services.push(ServiceReport {
                    entity: entity.name.clone(),
                    service,
                    source,
                    generated: generated.contains(label.as_str()),
                    sidecar: from_sidecar,
                });
            }
        }
    }

    let passes = |name: &str| {
        only.is_empty() || only.iter().any(|fragment| name.contains(fragment.as_str()))
    };
    for (entity, element) in &exported {
        if !passes(&entity.name) {
            continue;
        }
        let project = attribute(element, "projectName").unwrap_or_default();
        match here.get(entity) {
            None => report.entities.push(EntityReport {
                entity: entity.clone(),
                project,
                status: Status::New,
                differences: Vec::new(),
                volatile_ids: 0,
                ignored: 0,
            }),
            Some((repo_element, _)) => {
                let mut compared =
                    compare_entity(entity, element, repo_element, &solution.adopt.ignore_paths);
                compared.project = project;
                report.entities.push(compared);
            }
        }
    }

    let collections: BTreeSet<&str> = exported.keys().map(|e| e.collection.as_str()).collect();
    report.absent = here
        .keys()
        .filter(|e| {
            !exported.contains_key(*e)
                && collections.contains(e.collection.as_str())
                && passes(&e.name)
        })
        .cloned()
        .collect();
    Ok(report)
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
fn export_entities(path: &Path) -> Result<BTreeMap<EntityRef, Element>, AdoptError> {
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
fn repository_entities(
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
fn service_bodies(entity: &Element) -> BTreeMap<String, String> {
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

/// Adopt what is mechanical in an export, for the entities `report` found new or changed:
///
/// - a mashup's content, written to its sidecars through the same rendering `extract` uses;
/// - a new mashup's entity file, shaped like an existing mashup of its own project;
/// - a MediaEntity's image, re-wrapped as the repository stores it.
///
/// Everything else (a Thing's configuration table, a template's inline DataShape copy, a
/// service) is a judgement call, and stays reported and untouched. So does anything the report
/// counts as a revert: `apply` never writes a service. Run `twaco sync --all` afterwards to fold
/// the sidecars into the entity XML. Returns one line per step and any declaration refresh.
///
/// Every write is worked out before the first is made, so an export that cannot be adopted
/// (a payload that is not JSON, a name that is not a file name) changes nothing. The writes are
/// then made as one transaction under `lock`: all of them or, after a failure or a crash, none.
pub fn apply(
    solution: &Solution,
    export: &Path,
    report: &Report,
    lock: &WorkspaceLock,
) -> Result<ApplyOutcome, AdoptError> {
    let exported = export_entities(export)?;
    let here = repository_entities(solution)?;
    // In the report's order, notes and writes alike, so the output reads as the comparison did.
    let mut steps: Vec<(Option<Write>, String)> = Vec::new();
    for entry in report
        .entities
        .iter()
        .filter(|e| e.status != Status::Identical)
    {
        let Some(element) = exported.get(&entry.entity) else {
            continue;
        };
        let name = &entry.entity.name;
        match entry.entity.collection.as_str() {
            "Mashups" => {
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
                    let current = workspace::read_mashup(&dir).map_err(|e| io_error(&dir, e))?;
                    if current.as_ref() != Some(&assets) {
                        steps.push((
                            Some(Write::Sidecars(dir, assets)),
                            format!("sidecar  {name}"),
                        ));
                    }
                }
            }
            "MediaEntities" => {
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
            _ => {}
        }
    }
    let mut transaction = Transaction::new(&solution.root, "adopt");
    let mut lines = Vec::new();
    for (write, line) in steps {
        match write {
            Some(Write::Entity(path, text)) => transaction
                .write_file(&path, text.into_bytes())
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
            None => {}
        }
        lines.push(line);
    }
    let wrote = !transaction.is_empty();
    if wrote {
        transaction.apply(lock).map_err(AdoptError::Write)?;
    }
    Ok(ApplyOutcome {
        lines,
        types: super::types::refresh_after_write(solution, wrote),
    })
}

/// One file or sidecar folder `apply` will write, worked out in full beforehand.
enum Write {
    Entity(PathBuf, String),
    Sidecars(PathBuf, super::mashup::Assets),
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
        let error = apply(&solution, &export, &report, &locked(&solution))
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
        let outcome = apply(&solution, &export, &report, &locked(&solution)).unwrap();
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
                apply(&solution, &export, &report, &lock).is_err(),
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
        let lines: Vec<String> = apply(&solution, &export, &report, &locked(&solution))
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
        let lines: Vec<String> = apply(&solution, &export, &report, &locked(&solution))
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
