//! Changes to existing entities: Thing, ThingTemplate and ThingShape properties, subscriptions and
//! implemented shapes, timer and scheduler settings, styles, media, menu items and states.
//!
//! An edit replaces one element or one value and leaves every other byte of the file alone. What
//! it writes is what `twaco new` writes for the same final state, through the same writers, and
//! what a 10.1 server exports after importing it. A persistent property also gives every Thing
//! that has it a stored value, because the server writes one into each such Thing's export; the
//! edit writes those Things too, so the repository and the server agree.

use crate::core::catalog;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity_key::EntityKey;
use crate::core::entity_new::{self, Kind, NewError, NewRequest};
use crate::core::index::{Confidence, DependentOptions, Index};
use crate::core::lock::WorkspaceLock;
use crate::core::scan::{self, Kind as TokenKind, Span, Token};
use crate::core::splice::{self, Edit};
use crate::core::transaction::{Transaction, TransactionError};
use crate::core::types;
use crate::core::workspace::{self, EntityFile};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A property as `property add` and `property set` give it.
#[derive(Clone, Debug, Default)]
pub struct Property {
    pub name: String,
    /// Required for add; for set, a new type.
    pub base_type: Option<String>,
    pub default: Option<String>,
    pub no_default: bool,
    pub persistent: Option<bool>,
    pub logged: Option<bool>,
    pub read_only: Option<bool>,
    pub category: Option<String>,
    pub units: Option<String>,
    pub data_shape: Option<String>,
    pub description: Option<String>,
}

/// The style keys `style set` changes; `None` leaves a key as it is.
#[derive(Clone, Debug, Default)]
pub struct StyleChange {
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub secondary_background: Option<String>,
    pub line_color: Option<String>,
    pub line_style: Option<String>,
    pub line_thickness: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub text_size: Option<String>,
    pub image: Option<String>,
    pub display_string: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Action {
    PropertyAdd(Property),
    PropertySet {
        property: Property,
        force: bool,
    },
    PropertyRemove {
        name: String,
        force: bool,
    },
    SubscriptionAdd {
        name: String,
        event: String,
        source: Option<String>,
        source_property: Option<String>,
        code: String,
    },
    SubscriptionRemove {
        name: String,
    },
    ShapeAdd {
        shape: String,
    },
    ShapeRemove {
        shape: String,
    },
    TimerSet {
        every: Option<String>,
        run_as: Option<String>,
        enabled: Option<bool>,
    },
    SchedulerSet {
        cron: Option<String>,
        run_as: Option<String>,
        enabled: Option<bool>,
    },
    StyleSet(StyleChange),
    MediaSet {
        file: PathBuf,
    },
    MenuItemAdd {
        title: String,
        mashup: String,
        default: bool,
        before: Option<String>,
    },
    MenuItemRemove {
        title: String,
    },
    StateSet {
        state_type: String,
        states: Vec<String>,
    },
}

#[derive(Clone, Debug)]
pub struct Request {
    pub entity: String,
    pub action: Action,
}

/// One file the edit rewrites.
#[derive(Debug)]
pub struct FileChange {
    pub path: PathBuf,
    /// `Collection/Name`.
    pub entity: String,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

#[derive(Debug)]
pub struct Plan {
    /// `Collection/Name` of the entity asked about.
    pub entity: String,
    pub summary: String,
    /// The entity's own file first, then the Things whose stored values follow it.
    pub changes: Vec<FileChange>,
    /// What `--force` overrode, or what the edit could not check.
    pub warnings: Vec<String>,
}

impl Plan {
    fn new(entity: String) -> Plan {
        Plan {
            entity,
            summary: String::new(),
            changes: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unknown(String),
    #[error("{0}")]
    Exists(String),
    /// Something still uses what the edit would remove or retype.
    #[error("{0}")]
    InUse(String),
    #[error("{0}")]
    Write(TransactionError),
}

impl Coded for EditError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Unknown(_) => ErrorCode::UnknownEntity,
            Self::Exists(_) => ErrorCode::AlreadyExists,
            Self::InUse(_) => ErrorCode::GuardRefused,
            Self::Write(error) => error.code(),
        }
    }
}

impl From<NewError> for EditError {
    fn from(error: NewError) -> Self {
        match error {
            NewError::Unknown(why) => EditError::Unknown(why),
            NewError::Exists(why) => EditError::Exists(why),
            other => EditError::Invalid(other.to_string()),
        }
    }
}

fn invalid(why: impl std::fmt::Display) -> EditError {
    EditError::Invalid(why.to_string())
}

/// The base types a property may have. Anything else is refused rather than written.
const BASE_TYPES: [&str; 28] = [
    "ANYSCALAR",
    "BASETYPENAME",
    "BLOB",
    "BOOLEAN",
    "DATASHAPENAME",
    "DATETIME",
    "GUID",
    "HTML",
    "HYPERLINK",
    "IMAGELINK",
    "INFOTABLE",
    "INTEGER",
    "JSON",
    "LOCATION",
    "LONG",
    "MASHUPNAME",
    "MENUNAME",
    "NUMBER",
    "PASSWORD",
    "QUERY",
    "STRING",
    "STYLEDEFINITION",
    "TAGS",
    "TEXT",
    "THINGNAME",
    "THINGSHAPENAME",
    "THINGTEMPLATENAME",
    "USERNAME",
];

/// Events every Thing has, besides the ones its template and shapes define.
const THING_EVENTS: [&str; 8] = [
    "Alert",
    "AlertAck",
    "AlertReset",
    "AnyAlert",
    "AnyAlertAck",
    "AnyAlertReset",
    "DataChange",
    "ThingStart",
];

const MEMBER_HOSTS: [&str; 3] = ["Things", "ThingTemplates", "ThingShapes"];

// ---------------------------------------------------------------------------------------------
// Planning and applying

/// Plan an edit; nothing is written.
pub fn plan(solution: &Solution, request: &Request) -> Result<Plan, EditError> {
    let found = workspace::entities(solution);
    let file = workspace::resolve(&found, &request.entity)
        .map_err(|error| EditError::Unknown(error.to_string()))?
        .clone();
    let doc = Doc::read(&file)?;
    let mut plan = Plan::new(doc.label());
    let edit = Edit2 {
        solution,
        found: &found,
        doc: &doc,
    };
    match &request.action {
        Action::PropertyAdd(property) => edit.property_add(property, &mut plan)?,
        Action::PropertySet { property, force } => {
            edit.property_set(property, *force, &mut plan)?
        }
        Action::PropertyRemove { name, force } => edit.property_remove(name, *force, &mut plan)?,
        Action::SubscriptionAdd {
            name,
            event,
            source,
            source_property,
            code,
        } => {
            let source = Source {
                thing: source.as_deref().filter(|thing| !thing.is_empty()),
                property: source_property.as_deref().unwrap_or(""),
            };
            edit.subscription_add(name, event, &source, code, &mut plan)?
        }
        Action::SubscriptionRemove { name } => subscription_remove(&doc, name, &mut plan)?,
        Action::ShapeAdd { shape } => edit.shape_add(shape, &mut plan)?,
        Action::ShapeRemove { shape } => edit.shape_remove(shape, &mut plan)?,
        Action::TimerSet {
            every,
            run_as,
            enabled,
        } => {
            let value = every
                .as_deref()
                .map(entity_new::parse_duration)
                .transpose()?;
            let value = value.map(|ms| format!("{ms}.0"));
            let change = Settings {
                value,
                run_as: run_as.as_deref(),
                enabled: *enabled,
            };
            settings_set(&doc, Kind::Timer, &change, &mut plan)?
        }
        Action::SchedulerSet {
            cron,
            run_as,
            enabled,
        } => {
            let value = cron.as_deref().map(entity_new::validate_cron).transpose()?;
            let change = Settings {
                value,
                run_as: run_as.as_deref(),
                enabled: *enabled,
            };
            settings_set(&doc, Kind::Scheduler, &change, &mut plan)?
        }
        Action::StyleSet(change) => style_set(solution, &doc, change, &mut plan)?,
        Action::MediaSet { file } => media_set(&doc, file, &mut plan)?,
        Action::MenuItemAdd {
            title,
            mashup,
            default,
            before,
        } => {
            let item = (title.clone(), mashup.clone(), *default);
            menu_item_add(&found, &doc, item, before.as_deref(), &mut plan)?
        }
        Action::MenuItemRemove { title } => menu_item_remove(&doc, title, &mut plan)?,
        Action::StateSet { state_type, states } => state_set(&doc, state_type, states, &mut plan)?,
    }
    Ok(plan)
}

/// Write a plan in one transaction under the caller's lock.
pub fn apply(solution: &Solution, plan: &Plan, lock: &WorkspaceLock) -> Result<(), EditError> {
    let mut transaction = Transaction::new(&solution.root, "entity edit");
    for change in &plan.changes {
        transaction
            .replace_file(&change.path, &change.before, change.after.clone())
            .map_err(EditError::Write)?;
    }
    transaction.apply(lock).map_err(EditError::Write)?;
    Ok(())
}

/// The plan as JSON, for `--json` and MCP.
pub fn plan_json(solution: &Solution, plan: &Plan, applied: bool) -> Value {
    let files: Vec<String> = plan
        .changes
        .iter()
        .map(|change| relative(solution, &change.path))
        .collect();
    json!({
        "entity": plan.entity,
        "summary": plan.summary,
        "applied": applied,
        "files": files,
        "warnings": plan.warnings,
    })
}

/// A path relative to the solution, with forward slashes.
pub fn relative(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------------------------
// One parsed entity file

struct Doc {
    file: EntityFile,
    src: Vec<u8>,
    tokens: Vec<Token>,
    /// The entity element.
    entity: usize,
}

impl Doc {
    fn read(file: &EntityFile) -> Result<Doc, EditError> {
        let src = read_file(&file.path)?;
        Doc::parse(file.clone(), src)
    }

    fn parse(file: EntityFile, src: Vec<u8>) -> Result<Doc, EditError> {
        let tokens = scan::tokenize(&src).map_err(invalid)?;
        let entity = crate::core::sidecar::entity_element(&tokens, &src)
            .ok_or_else(|| invalid(format!("{} is not an entity document", file.path.display())))?;
        Ok(Doc {
            file,
            src,
            tokens,
            entity,
        })
    }

    fn collection(&self) -> &str {
        &self.file.info.collection
    }

    fn name(&self) -> &str {
        &self.file.info.name
    }

    fn label(&self) -> String {
        format!("{}/{}", self.collection(), self.name())
    }

    fn require(&self, collections: &[&str], what: &str) -> Result<(), EditError> {
        if collections.contains(&self.collection()) {
            Ok(())
        } else {
            Err(invalid(format!("{what}; {} is not one", self.label())))
        }
    }

    fn attr(&self, token: usize, name: &str) -> Option<String> {
        scan::attribute(&self.src, &self.tokens[token], name)
            .ok()
            .flatten()
            .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(&self.src))))
    }

    /// Where members live: a ThingShape holds them itself, a Thing or template in its own
    /// `<ThingShape>`.
    fn members(&self) -> Result<usize, EditError> {
        if self.collection() == "ThingShapes" {
            return Ok(self.entity);
        }
        self.only_child(self.entity, "ThingShape")
    }

    fn only_child(&self, parent: usize, name: &str) -> Result<usize, EditError> {
        match scan::child_tags(&self.tokens, &self.src, name, parent).as_slice() {
            [one] => Ok(*one),
            [] => Err(invalid(format!("{} has no {name}", self.label()))),
            _ => Err(invalid(format!(
                "{} has more than one {name}",
                self.label()
            ))),
        }
    }

    /// The child of `section` with this `name` attribute.
    fn named(&self, section: usize, tag: &str, name: &str) -> Option<usize> {
        scan::child_tags(&self.tokens, &self.src, tag, section)
            .into_iter()
            .find(|&child| self.attr(child, "name").as_deref() == Some(name))
    }

    /// The `name` attributes of the `tag` children of every `section` under `parent`.
    fn names_in(&self, parent: usize, section: &str, tag: &str) -> Vec<String> {
        scan::child_tags(&self.tokens, &self.src, section, parent)
            .into_iter()
            .flat_map(|list| scan::child_tags(&self.tokens, &self.src, tag, list))
            .filter_map(|child| self.attr(child, "name"))
            .collect()
    }

    fn span(&self, element: usize) -> Result<Span, EditError> {
        scan::element_span(&self.tokens, element)
            .ok_or_else(|| invalid(format!("{}: an element is not closed", self.label())))
    }

    /// The bytes between an element's start and end tags (empty for `<X/>`).
    fn inner(&self, element: usize) -> Result<Span, EditError> {
        if self.tokens[element].kind == TokenKind::Empty {
            let at = self.tokens[element].span.end;
            return Ok(Span::new(at, at));
        }
        let end = scan::element_end_in(&self.tokens, &self.src, element)
            .ok_or_else(|| invalid(format!("{}: an element is not closed", self.label())))?;
        Ok(Span::new(
            self.tokens[element].span.end,
            self.tokens[end].span.start,
        ))
    }

    /// An element's text: its CDATA payload, or its decoded text.
    fn text(&self, element: usize) -> Result<String, EditError> {
        let inner = String::from_utf8_lossy(self.inner(element)?.of(&self.src)).into_owned();
        let trimmed = inner.trim();
        let payload = trimmed
            .strip_prefix("<![CDATA[")
            .and_then(|value| value.strip_suffix("]]>"));
        Ok(match payload {
            Some(payload) => payload.to_string(),
            None => scan::decode_entities(trimmed),
        })
    }

    /// The whitespace before an element's start tag on its line.
    fn indent_of(&self, element: usize) -> String {
        let start = self.tokens[element].span.start;
        let line = self.src[..start]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        String::from_utf8_lossy(&self.src[line..start]).into_owned()
    }

    fn change(&self, after: Vec<u8>) -> FileChange {
        FileChange {
            path: self.file.path.clone(),
            entity: self.label(),
            before: self.src.clone(),
            after,
        }
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, EditError> {
    std::fs::read(path).map_err(|error| invalid(format!("{}: {error}", path.display())))
}

fn splice_one(src: &[u8], span: Span, bytes: impl Into<Vec<u8>>) -> Result<Vec<u8>, EditError> {
    splice_all(src, vec![Edit::new(span, bytes.into())])
}

/// Splice `edits` into `src`, their text written with the file's own line ending: what twaco
/// writes uses `\n`, and a CRLF file must not gain bare line feeds.
fn splice_all(src: &[u8], edits: Vec<Edit>) -> Result<Vec<u8>, EditError> {
    let crlf = workspace::line_ending(&String::from_utf8_lossy(src)) == "\r\n";
    let edits: Vec<Edit> = edits
        .into_iter()
        .map(|edit| {
            if !crlf {
                return edit;
            }
            let text = String::from_utf8_lossy(&edit.replacement).replace("\r\n", "\n");
            Edit::new(edit.span, text.replace('\n', "\r\n").into_bytes())
        })
        .collect();
    splice::splice(src, &edits).map_err(invalid)
}

/// Shift each line of `xml`, written for `from` leading spaces, to start with `to` instead.
fn reindent(xml: &str, from: usize, to: &str) -> String {
    xml.lines()
        .map(|line| {
            let spaces = line.len() - line.trim_start_matches(' ').len();
            let keep = spaces.saturating_sub(from);
            format!("{to}{}{}", " ".repeat(keep), &line[spaces..])
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Append `child` (written at `child_indent` spaces) as the last child of `section`, one level
/// deeper than the section: after the last child, or into an empty section.
fn append_child(
    doc: &Doc,
    section: usize,
    child: &str,
    child_indent: usize,
) -> Result<Vec<u8>, EditError> {
    let indent = doc.indent_of(section);
    let body = reindent(child, child_indent, &format!("{indent}    "));
    let inner = doc.inner(section)?;
    let content = &doc.src[inner.start..inner.end];
    if content.iter().all(u8::is_ascii_whitespace) {
        let tag = &doc.tokens[section];
        if tag.kind == TokenKind::Empty {
            let name = String::from_utf8_lossy(tag.name.of(&doc.src)).into_owned();
            let open = String::from_utf8_lossy(&doc.src[tag.span.start..tag.span.end - 2]);
            let element = format!("{}>\n{body}\n{indent}</{name}>", open.trim_end());
            return splice_one(&doc.src, tag.span, element);
        }
        return splice_one(&doc.src, inner, format!("\n{body}\n{indent}"));
    }
    let end = content
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |at| at + 1);
    let at = inner.start + end;
    splice_one(&doc.src, Span::new(at, at), format!("\n{body}"))
}

/// The sections `twaco new` writes empty across two lines, `<X>\n  </X>`; the rest it writes
/// as `<X></X>`.
const OPEN_WHEN_EMPTY: [&str; 2] = ["Subscriptions", "ImplementedShapes"];

/// Remove `element` with its line; a section left empty is written as `twaco new` writes it.
fn remove_child(doc: &Doc, section: usize, element: usize) -> Result<Vec<u8>, EditError> {
    let span = doc.span(element)?;
    let inner = doc.inner(section)?;
    let before = &doc.src[inner.start..span.start];
    let after = &doc.src[span.end..inner.end];
    if before.iter().all(u8::is_ascii_whitespace) && after.iter().all(u8::is_ascii_whitespace) {
        let name = String::from_utf8_lossy(doc.tokens[section].name.of(&doc.src)).into_owned();
        let body = if OPEN_WHEN_EMPTY.contains(&name.as_str()) {
            format!("\n{}", doc.indent_of(section))
        } else {
            String::new()
        };
        return splice_one(&doc.src, inner, body);
    }
    let line = doc.src[..span.start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .unwrap_or(span.start);
    splice_one(&doc.src, Span::new(line, span.end), Vec::new())
}

// ---------------------------------------------------------------------------------------------
// The inheritance view

/// The Things, templates and shapes of the solution, what each inherits, and the platform cache.
struct Model {
    /// `Collection/Name` -> the templates and shapes it inherits, nearest first.
    inherits: BTreeMap<String, Vec<String>>,
    files: Vec<EntityFile>,
    platform: Option<types::Platform>,
}

impl Model {
    fn build(solution: &Solution, found: &[EntityFile]) -> Result<Model, EditError> {
        let (entities, skipped) = catalog::inheritance(solution);
        if !skipped.is_empty() {
            return Err(invalid(format!(
                "the inheritance model could not read {}; fix that before changing members",
                skipped.join(", ")
            )));
        }
        let inherits = entities
            .into_iter()
            .map(|entity| {
                let label = format!("{}/{}", entity.collection, entity.name);
                (label, entity.inherits)
            })
            .collect();
        let files = found
            .iter()
            .filter(|file| MEMBER_HOSTS.contains(&file.info.collection.as_str()))
            .cloned()
            .collect();
        Ok(Model {
            inherits,
            files,
            platform: types::cached_platform(solution),
        })
    }

    fn file(&self, collection: &str, name: &str) -> Option<&EntityFile> {
        self.files
            .iter()
            .find(|file| file.info.collection == collection && file.info.name == name)
    }

    /// The repository template or shape an inherited name refers to.
    fn ancestor(&self, name: &str) -> Option<&EntityFile> {
        self.file("ThingTemplates", name)
            .or_else(|| self.file("ThingShapes", name))
    }

    fn platform_meta(&self, name: &str) -> Option<&types::PlatformMeta> {
        let platform = self.platform.as_ref()?;
        platform
            .templates
            .get(name)
            .or_else(|| platform.shapes.get(name))
    }

    fn inherited_names(&self, doc: &Doc) -> Vec<String> {
        self.inherits.get(&doc.label()).cloned().unwrap_or_default()
    }

    /// The repository entities that inherit `doc`, a template or shape.
    fn descendants(&self, doc: &Doc) -> Vec<&EntityFile> {
        if doc.collection() == "Things" {
            return Vec::new();
        }
        self.inherits
            .iter()
            .filter(|(_, names)| names.iter().any(|name| name == doc.name()))
            .filter_map(|(label, _)| {
                let (collection, name) = label.split_once('/')?;
                self.file(collection, name)
            })
            .collect()
    }

    /// The Things that have `doc`'s members: itself if it is a Thing, else those inheriting it.
    fn things_having(&self, doc: &Doc) -> Vec<EntityFile> {
        if doc.collection() == "Things" {
            return vec![doc.file.clone()];
        }
        self.descendants(doc)
            .into_iter()
            .filter(|file| file.info.collection == "Things")
            .cloned()
            .collect()
    }

    /// Member names of what `doc` inherits, each with where it comes from. A platform template
    /// or shape counts when the platform cache knows it.
    fn inherited_members(&self, doc: &Doc) -> Result<Vec<(String, String)>, EditError> {
        let mut out = Vec::new();
        for name in self.inherited_names(doc) {
            if let Some(file) = self.ancestor(&name) {
                for member in own_members(&Doc::read(file)?)? {
                    out.push((member, name.clone()));
                }
            } else if let Some(meta) = self.platform_meta(&name) {
                for member in meta.properties.keys().chain(meta.services.keys()) {
                    out.push((member.clone(), name.clone()));
                }
            }
        }
        Ok(out)
    }

    /// Whether `doc` is, or inherits from, the platform template `wanted` (Timer, Scheduler).
    fn is_a(&self, doc: &Doc, wanted: &str) -> bool {
        let template = match doc.collection() {
            "Things" => doc.attr(doc.entity, "thingTemplate"),
            "ThingTemplates" => doc.attr(doc.entity, "baseThingTemplate"),
            _ => None,
        };
        template.as_deref() == Some(wanted)
            || self.inherited_names(doc).iter().any(|name| name == wanted)
    }
}

/// The property, service and event names an entity declares itself.
fn own_members(doc: &Doc) -> Result<Vec<String>, EditError> {
    let host = doc.members()?;
    let mut out = doc.names_in(host, "PropertyDefinitions", "PropertyDefinition");
    out.extend(doc.names_in(host, "ServiceDefinitions", "ServiceDefinition"));
    out.extend(doc.names_in(host, "EventDefinitions", "EventDefinition"));
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Properties

/// A property definition, as its attributes.
#[derive(Clone, Debug)]
struct Definition {
    base_type: String,
    default: Option<String>,
    persistent: bool,
    logged: bool,
    read_only: bool,
    category: String,
    units: Option<String>,
    data_shape: Option<String>,
    description: String,
    name: String,
    ordinal: String,
}

impl Definition {
    fn read(doc: &Doc, element: usize) -> Definition {
        let get = |name: &str| doc.attr(element, name);
        let flag = |name: &str| get(name).as_deref() == Some("true");
        Definition {
            base_type: get("baseType").unwrap_or_default(),
            default: get("aspect.defaultValue"),
            persistent: flag("aspect.isPersistent"),
            logged: flag("aspect.isLogged"),
            read_only: flag("aspect.isReadOnly"),
            category: get("category").unwrap_or_default(),
            units: get("aspect.units"),
            data_shape: get("aspect.dataShape"),
            description: get("description").unwrap_or_default(),
            name: get("name").unwrap_or_default(),
            ordinal: get("ordinal").unwrap_or_else(|| "0".into()),
        }
    }

    /// The element as written at 20 spaces, in the attribute order of 1,055 mined definitions.
    /// A 10.1 import keeps every attribute as given.
    fn xml(&self) -> String {
        let attr = |name: &str, value: &str| {
            let value = scan::escape_attribute(value);
            format!("\n                     {name}=\"{value}\"")
        };
        let mut out = String::from("                    <PropertyDefinition");
        out.push_str(&attr("aspect.cacheTime", "0.0"));
        out.push_str(&attr("aspect.dataChangeType", "VALUE"));
        out.push_str(&attr("aspect.isLogged", &self.logged.to_string()));
        out.push_str(&attr("aspect.isPersistent", &self.persistent.to_string()));
        out.push_str(&attr("aspect.isReadOnly", &self.read_only.to_string()));
        let optional = [
            ("aspect.units", &self.units),
            ("aspect.defaultValue", &self.default),
            ("aspect.dataShape", &self.data_shape),
        ];
        for (name, value) in optional {
            if let Some(value) = value {
                out.push_str(&attr(name, value));
            }
        }
        out.push_str(&attr("baseType", &self.base_type));
        out.push_str(&attr("category", &self.category));
        out.push_str(&attr("description", &self.description));
        out.push_str(&attr("isLocalOnly", "false"));
        out.push_str(&attr("name", &self.name));
        out.push_str(&attr("ordinal", &self.ordinal));
        out.push_str("></PropertyDefinition>");
        out
    }

    /// Apply the fields `property` gives over this definition.
    fn merge(&mut self, property: &Property) {
        if let Some(base_type) = &property.base_type {
            self.base_type = base_type.clone();
        }
        if property.no_default {
            self.default = None;
        } else if property.default.is_some() {
            self.default = property.default.clone();
        }
        let flags = [
            (&mut self.persistent, property.persistent),
            (&mut self.logged, property.logged),
            (&mut self.read_only, property.read_only),
        ];
        for (target, value) in flags {
            if let Some(value) = value {
                *target = value;
            }
        }
        if let Some(category) = &property.category {
            self.category = category.clone();
        }
        if let Some(units) = &property.units {
            self.units = Some(units.clone()).filter(|units| !units.is_empty());
        }
        if let Some(shape) = &property.data_shape {
            self.data_shape = Some(shape.clone()).filter(|shape| !shape.is_empty());
        }
        if let Some(description) = &property.description {
            self.description = description.clone();
        }
    }

    /// Refuse what the server would refuse or misread; warn about what cannot be checked.
    fn validate(&self, found: &[EntityFile]) -> Result<Vec<String>, EditError> {
        let mut warnings = Vec::new();
        if !BASE_TYPES.contains(&self.base_type.as_str()) {
            return Err(invalid(format!(
                "{} is not a property base type; known: {}",
                self.base_type,
                BASE_TYPES.join(", ")
            )));
        }
        if self.base_type == "INFOTABLE" {
            let shape = self
                .data_shape
                .as_deref()
                .ok_or_else(|| invalid("an INFOTABLE property needs --data-shape"))?;
            let known = found
                .iter()
                .any(|file| file.info.collection == "DataShapes" && file.info.name == shape);
            if !known {
                warnings.push(format!(
                    "DataShape {shape} is not in the solution; it must exist on the server"
                ));
            }
        } else if self.data_shape.is_some() {
            return Err(invalid("--data-shape is only for an INFOTABLE property"));
        }
        if let Some(default) = &self.default {
            let fine = match self.base_type.as_str() {
                "NUMBER" => default.parse::<f64>().is_ok_and(f64::is_finite),
                "INTEGER" => default.parse::<i32>().is_ok(),
                "LONG" => default.parse::<i64>().is_ok(),
                "BOOLEAN" => default == "true" || default == "false",
                "JSON" | "QUERY" => serde_json::from_str::<Value>(default).is_ok(),
                "INFOTABLE" => false,
                _ => true,
            };
            if !fine {
                return Err(invalid(format!(
                    "--default {default:?} is not a valid {} value",
                    self.base_type
                )));
            }
            if default.contains("]]>") {
                return Err(invalid("--default cannot contain `]]>`"));
            }
        }
        Ok(warnings)
    }

    /// The value a 10.1 server stores for this property in a Thing's `<ThingProperties>`: only a
    /// persistent property has one, its default or the type's (observed for every type here).
    fn stored_value(&self) -> Result<Option<String>, EditError> {
        if !self.persistent || self.base_type == "XML" {
            return Ok(None);
        }
        let given = self.default.as_deref();
        let value = match self.base_type.as_str() {
            "NUMBER" => java_double(given.unwrap_or("0")),
            "INTEGER" | "LONG" => given.unwrap_or("0").to_string(),
            "BOOLEAN" => given.unwrap_or("false").to_string(),
            "DATETIME" => given.unwrap_or("1970-01-01T00:00:00.000Z").to_string(),
            "LOCATION" => given.unwrap_or("0.0,0.0,0.0").to_string(),
            "JSON" | "QUERY" => format!("<json><![CDATA[{}]]></json>", given.unwrap_or("{}")),
            "GUID" => format!(
                "<![CDATA[{}]]>",
                given.unwrap_or("00000000-0000-0000-0000-000000000000")
            ),
            "INFOTABLE" => {
                return Err(invalid(
                    "a persistent INFOTABLE property needs a stored infotable that twaco does \
                     not write yet; make it in Composer, or leave it not persistent",
                ))
            }
            _ => given
                .map(|value| format!("<![CDATA[{value}]]>"))
                .unwrap_or_default(),
        };
        Ok(Some(value))
    }
}

/// Java's `Double.toString` for a number given as text.
fn java_double(text: &str) -> String {
    match text.parse::<f64>() {
        Ok(number) if number.fract() == 0.0 && number.abs() < 1e7 => format!("{number:.1}"),
        Ok(number) => number.to_string(),
        Err(_) => text.to_string(),
    }
}

fn validate_member_name(name: &str) -> Result<(), EditError> {
    let mut chars = name.chars();
    let first = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !first || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(invalid(format!(
            "{name:?} is not a member name: letters, digits and _, not starting with a digit"
        )));
    }
    Ok(())
}

/// The edits that need the solution around the entity.
struct Edit2<'a> {
    solution: &'a Solution,
    found: &'a [EntityFile],
    doc: &'a Doc,
}

impl Edit2<'_> {
    fn property_add(&self, property: &Property, plan: &mut Plan) -> Result<(), EditError> {
        let doc = self.doc;
        doc.require(
            &MEMBER_HOSTS,
            "properties belong to a Thing, ThingTemplate or ThingShape",
        )?;
        validate_member_name(&property.name)?;
        let base_type = property
            .base_type
            .clone()
            .ok_or_else(|| invalid("property add needs <name>:<TYPE>"))?;
        let model = Model::build(self.solution, self.found)?;
        refuse_clash(&model, doc, std::slice::from_ref(&property.name))?;
        let list = doc.only_child(doc.members()?, "PropertyDefinitions")?;
        let ordinal = scan::child_tags(&doc.tokens, &doc.src, "PropertyDefinition", list)
            .into_iter()
            .filter_map(|element| doc.attr(element, "ordinal")?.parse::<u64>().ok())
            .max()
            .map_or(1, |max| max + 1);
        let mut definition = Definition {
            base_type,
            default: None,
            persistent: false,
            logged: false,
            read_only: false,
            category: String::new(),
            units: None,
            data_shape: None,
            description: String::new(),
            name: property.name.clone(),
            ordinal: ordinal.to_string(),
        };
        definition.merge(property);
        plan.warnings.extend(definition.validate(self.found)?);
        let stored = definition.stored_value()?;
        let after = append_child(doc, list, &definition.xml(), 20)?;
        plan.summary = format!("add property {}:{}", definition.name, definition.base_type);
        let mut files = Files::new(doc, after);
        files.set_values(&model, &definition.name, stored.as_deref())?;
        plan.changes = files.into_changes();
        Ok(())
    }

    fn property_set(
        &self,
        property: &Property,
        force: bool,
        plan: &mut Plan,
    ) -> Result<(), EditError> {
        let doc = self.doc;
        doc.require(
            &MEMBER_HOSTS,
            "properties belong to a Thing, ThingTemplate or ThingShape",
        )?;
        let list = doc.only_child(doc.members()?, "PropertyDefinitions")?;
        let element = doc
            .named(list, "PropertyDefinition", &property.name)
            .ok_or_else(|| {
                EditError::Unknown(format!("{} has no property {}", doc.label(), property.name))
            })?;
        let old = Definition::read(doc, element);
        let mut definition = old.clone();
        definition.merge(property);
        plan.warnings.extend(definition.validate(self.found)?);
        let retyped = definition.base_type != old.base_type;
        if retyped {
            self.guard_users(&property.name, force, "retype", plan)?;
        }
        let stored = definition.stored_value()?;
        let xml = reindent(&definition.xml(), 20, &doc.indent_of(element));
        let after = splice_one(&doc.src, doc.span(element)?, xml.trim_start().to_string())?;
        plan.summary = format!("set property {}", property.name);
        let model = Model::build(self.solution, self.found)?;
        let mut files = Files::new(doc, after);
        if retyped || old.persistent != definition.persistent {
            // A stored value of the old type means nothing for the new one: it starts again.
            files.set_values(&model, &property.name, None)?;
            files.set_values(&model, &property.name, stored.as_deref())?;
        }
        plan.changes = files.into_changes();
        Ok(())
    }

    fn property_remove(&self, name: &str, force: bool, plan: &mut Plan) -> Result<(), EditError> {
        let doc = self.doc;
        doc.require(
            &MEMBER_HOSTS,
            "properties belong to a Thing, ThingTemplate or ThingShape",
        )?;
        let list = doc.only_child(doc.members()?, "PropertyDefinitions")?;
        let element = doc
            .named(list, "PropertyDefinition", name)
            .ok_or_else(|| EditError::Unknown(format!("{} has no property {name}", doc.label())))?;
        self.guard_users(name, force, "remove", plan)?;
        let after = remove_child(doc, list, element)?;
        plan.summary = format!("remove property {name}");
        let model = Model::build(self.solution, self.found)?;
        let mut files = Files::new(doc, after);
        files.set_values(&model, name, None)?;
        plan.changes = files.into_changes();
        Ok(())
    }

    /// Refuse while something uses `member` of the entity, unless forced; forced, the users are
    /// warnings. Users are what the index sees naming it (a mashup binding, another entity's
    /// script), and `me.<member>` in the scripts of the entity and everything inheriting it.
    fn guard_users(
        &self,
        member: &str,
        force: bool,
        verb: &str,
        plan: &mut Plan,
    ) -> Result<(), EditError> {
        let doc = self.doc;
        let mut users: BTreeSet<String> = BTreeSet::new();
        let index = Index::build(self.solution);
        if let Ok(key) = EntityKey::new(doc.collection(), doc.name()) {
            let options = DependentOptions {
                member: Some(member.to_string()),
                min: Confidence::Resolved,
                max_depth: Some(1),
            };
            for dependent in index.dependents(&key, &options) {
                let inherited = !dependent.path.is_empty()
                    && dependent.path.iter().all(|step| step.kind.is_inheritance());
                if !inherited {
                    users.insert(dependent.label);
                }
            }
        }
        let model = Model::build(self.solution, self.found)?;
        let mut scope = vec![doc.file.clone()];
        scope.extend(model.descendants(doc).into_iter().cloned());
        for file in scope {
            if reads_me_member(&read_file(&file.path)?, member) {
                let label = format!("{}/{}", file.info.collection, file.info.name);
                users.insert(format!("{label} (me.{member})"));
            }
        }
        if users.is_empty() {
            return Ok(());
        }
        let list = users.into_iter().collect::<Vec<_>>().join(", ");
        if force {
            plan.warnings
                .push(format!("--force: {member} is still used by {list}"));
            return Ok(());
        }
        Err(EditError::InUse(format!(
            "{member} is still used by {list}; refusing to {verb} it (--force does it anyway)"
        )))
    }

    fn subscription_add(
        &self,
        name: &str,
        event: &str,
        source: &Source<'_>,
        code: &str,
        plan: &mut Plan,
    ) -> Result<(), EditError> {
        let doc = self.doc;
        doc.require(
            &MEMBER_HOSTS,
            "subscriptions belong to a Thing, ThingTemplate or ThingShape",
        )?;
        validate_member_name(name)?;
        if code.contains("]]>") {
            return Err(invalid("the subscription script cannot contain `]]>`"));
        }
        let list = doc.only_child(doc.members()?, "Subscriptions")?;
        if doc.named(list, "Subscription", name).is_some() {
            return Err(EditError::Exists(format!(
                "{} already has a subscription {name}",
                doc.label()
            )));
        }
        let model = Model::build(self.solution, self.found)?;
        let source_doc = match source.thing {
            None => None,
            Some(thing) => {
                let file = model.file("Things", thing).ok_or_else(|| {
                    EditError::Unknown(format!("no Thing {thing} in the solution to subscribe to"))
                })?;
                Some(Doc::read(file)?)
            }
        };
        let target = source_doc.as_ref().unwrap_or(doc);
        let (events, properties) = events_and_properties(&model, target)?;
        if !events.contains(event) {
            let known = events.into_iter().collect::<Vec<_>>().join(", ");
            return Err(EditError::Unknown(format!(
                "{} has no event {event}; it has {known}",
                target.label()
            )));
        }
        if event == "DataChange" {
            if source.property.is_empty() {
                return Err(invalid("a DataChange subscription needs --source-property"));
            }
            if !properties.contains(source.property) {
                return Err(EditError::Unknown(format!(
                    "{} has no property {}",
                    target.label(),
                    source.property
                )));
            }
        } else if !source.property.is_empty() {
            return Err(invalid(
                "--source-property is only for a DataChange subscription",
            ));
        }
        let thing = source.thing.unwrap_or("");
        let xml = entity_new::subscription_xml(name, event, thing, source.property, code);
        plan.summary = format!("add subscription {name} on {event}");
        plan.changes
            .push(doc.change(append_child(doc, list, &xml, 20)?));
        Ok(())
    }

    fn shape_add(&self, shape: &str, plan: &mut Plan) -> Result<(), EditError> {
        let doc = self.doc;
        let what = "a ThingShape is implemented by a Thing or ThingTemplate";
        doc.require(&["Things", "ThingTemplates"], what)?;
        let model = Model::build(self.solution, self.found)?;
        let shape_doc = match model.file("ThingShapes", shape) {
            Some(file) => Some(Doc::read(file)?),
            None => None,
        };
        let platform = model
            .platform
            .as_ref()
            .and_then(|platform| platform.shapes.get(shape));
        if shape_doc.is_none() && platform.is_none() {
            return Err(EditError::Unknown(format!(
                "no ThingShape {shape} in the solution or the platform cache"
            )));
        }
        let list = doc.only_child(doc.entity, "ImplementedShapes")?;
        if doc.named(list, "ImplementedShape", shape).is_some() {
            return Err(EditError::Exists(format!(
                "{} already implements {shape}",
                doc.label()
            )));
        }
        if model.inherited_names(doc).iter().any(|name| name == shape) {
            return Err(EditError::Exists(format!(
                "{} already implements {shape} through its template",
                doc.label()
            )));
        }
        let members = match (&shape_doc, platform) {
            (Some(shape_doc), _) => own_members(shape_doc)?,
            (None, Some(meta)) => meta
                .properties
                .keys()
                .chain(meta.services.keys())
                .cloned()
                .collect(),
            (None, None) => Vec::new(),
        };
        refuse_clash(&model, doc, &members).map_err(|error| match error {
            EditError::Exists(why) => {
                EditError::Exists(format!("{shape} brings a member that clashes: {why}"))
            }
            other => other,
        })?;
        let after = append_child(doc, list, &entity_new::implemented_shape_xml(shape), 16)?;
        plan.summary = format!("implement {shape}");
        // The shape's persistent properties now have stored values in every Thing that has it.
        let mut files = Files::new(doc, after);
        if let Some(shape_doc) = &shape_doc {
            let host = shape_doc.members()?;
            let lists = scan::child_tags(
                &shape_doc.tokens,
                &shape_doc.src,
                "PropertyDefinitions",
                host,
            );
            for list in lists {
                let elements = scan::child_tags(
                    &shape_doc.tokens,
                    &shape_doc.src,
                    "PropertyDefinition",
                    list,
                );
                for element in elements {
                    let definition = Definition::read(shape_doc, element);
                    if let Some(stored) = definition.stored_value()? {
                        files.set_values(&model, &definition.name, Some(&stored))?;
                    }
                }
            }
        }
        plan.changes = files.into_changes();
        Ok(())
    }
}

impl Edit2<'_> {
    fn shape_remove(&self, shape: &str, plan: &mut Plan) -> Result<(), EditError> {
        let doc = self.doc;
        let what = "a ThingShape is implemented by a Thing or ThingTemplate";
        doc.require(&["Things", "ThingTemplates"], what)?;
        let list = doc.only_child(doc.entity, "ImplementedShapes")?;
        let element = doc.named(list, "ImplementedShape", shape).ok_or_else(|| {
            EditError::Unknown(format!("{} does not implement {shape} itself", doc.label()))
        })?;
        let after = remove_child(doc, list, element)?;
        plan.summary = format!("stop implementing {shape}");
        let model = Model::build(self.solution, self.found)?;
        let Some(shape_file) = model.file("ThingShapes", shape) else {
            plan.changes.push(doc.change(after));
            return Ok(());
        };
        // A Thing that no longer has the shape any other way loses its stored values, as the
        // server's export does.
        let mut losing = Vec::new();
        for thing in model.things_having(doc) {
            if !still_implements(&model, doc, &thing, shape)? {
                losing.push(thing);
            }
        }
        let shape_doc = Doc::read(shape_file)?;
        let names = shape_doc.names_in(
            shape_doc.members()?,
            "PropertyDefinitions",
            "PropertyDefinition",
        );
        let mut files = Files::new(doc, after);
        for name in names {
            files.clear_values(&losing, &name)?;
        }
        plan.changes = files.into_changes();
        Ok(())
    }
}

/// Whether `thing` implements `shape` other than through `doc`'s own ImplementedShapes: itself,
/// or through another template of its chain.
fn still_implements(
    model: &Model,
    doc: &Doc,
    thing: &EntityFile,
    shape: &str,
) -> Result<bool, EditError> {
    let mut holders = vec![thing.clone()];
    let label = format!("{}/{}", thing.info.collection, thing.info.name);
    for name in model.inherits.get(&label).cloned().unwrap_or_default() {
        if let Some(template) = model.file("ThingTemplates", &name) {
            holders.push(template.clone());
        }
    }
    for holder in holders {
        if holder.path == doc.file.path {
            continue;
        }
        let other = Doc::read(&holder)?;
        let list = other.only_child(other.entity, "ImplementedShapes")?;
        if other.named(list, "ImplementedShape", shape).is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The files an edit rewrites, keyed by path, each from its original bytes: the entity first,
/// then the Things whose stored values follow it.
struct Files {
    entity: EntityFile,
    order: Vec<PathBuf>,
    by_path: BTreeMap<PathBuf, FileChange>,
}

impl Files {
    fn new(doc: &Doc, after: Vec<u8>) -> Files {
        let change = doc.change(after);
        let path = change.path.clone();
        Files {
            entity: doc.file.clone(),
            order: vec![path.clone()],
            by_path: BTreeMap::from([(path, change)]),
        }
    }

    /// The current bytes of a Thing, as this edit has left them so far.
    fn current(&mut self, file: &EntityFile) -> Result<&mut FileChange, EditError> {
        if !self.by_path.contains_key(&file.path) {
            let src = read_file(&file.path)?;
            let change = FileChange {
                path: file.path.clone(),
                entity: format!("{}/{}", file.info.collection, file.info.name),
                before: src.clone(),
                after: src,
            };
            self.order.push(file.path.clone());
            self.by_path.insert(file.path.clone(), change);
        }
        Ok(self.by_path.get_mut(&file.path).expect("inserted"))
    }

    /// Give `property` the stored value `value` (or none) in every Thing that has the edited
    /// entity's members. The entity's own file is read as this edit has left it.
    fn set_values(
        &mut self,
        model: &Model,
        property: &str,
        value: Option<&str>,
    ) -> Result<(), EditError> {
        let entity = self.entity.clone();
        let host = Doc::parse(entity.clone(), self.by_path[&entity.path].after.clone())?;
        for thing in model.things_having(&host) {
            let change = self.current(&thing)?;
            let doc = Doc::parse(thing.clone(), change.after.clone())?;
            change.after = set_stored_value(&doc, property, value)?;
        }
        Ok(())
    }

    /// Remove `property`'s stored value from each of `things`.
    fn clear_values(&mut self, things: &[EntityFile], property: &str) -> Result<(), EditError> {
        for thing in things {
            let change = self.current(thing)?;
            let doc = Doc::parse(thing.clone(), change.after.clone())?;
            change.after = set_stored_value(&doc, property, None)?;
        }
        Ok(())
    }

    fn into_changes(mut self) -> Vec<FileChange> {
        self.order
            .iter()
            .filter_map(|path| self.by_path.remove(path))
            .filter(|change| change.after != change.before)
            .collect()
    }
}

/// Refuse a member name the entity, what it inherits, or anything inheriting it already has.
fn refuse_clash(model: &Model, doc: &Doc, names: &[String]) -> Result<(), EditError> {
    let own = own_members(doc)?;
    let inherited = model.inherited_members(doc)?;
    let mut below: Vec<(String, Vec<String>)> = Vec::new();
    for descendant in model.descendants(doc) {
        let label = format!("{}/{}", descendant.info.collection, descendant.info.name);
        below.push((label, own_members(&Doc::read(descendant)?)?));
    }
    for name in names {
        if own.contains(name) {
            return Err(EditError::Exists(format!(
                "{} already has a member {name}",
                doc.label()
            )));
        }
        if let Some((_, from)) = inherited.iter().find(|(member, _)| member == name) {
            return Err(EditError::Exists(format!(
                "{} already has a member {name}, from {from}",
                doc.label()
            )));
        }
        if let Some((label, _)) = below.iter().find(|(_, members)| members.contains(name)) {
            return Err(EditError::Exists(format!(
                "{label} inherits {} and has its own member {name}; the server refuses the clash",
                doc.label()
            )));
        }
    }
    Ok(())
}

/// A Thing's `<ThingProperties>` with `property`'s stored value set to `value`, or removed. The
/// entries go in the order the server's export uses: a Java HashMap of the names. An existing
/// value is run-time state and stays as it is.
fn set_stored_value(doc: &Doc, property: &str, value: Option<&str>) -> Result<Vec<u8>, EditError> {
    let blocks = scan::child_tags(&doc.tokens, &doc.src, "ThingProperties", doc.entity);
    let Some(&block) = blocks.first() else {
        // An export without the element has no stored values; the server writes it last.
        let Some(value) = value else {
            return Ok(doc.src.clone());
        };
        let xml = format!(
            "<ThingProperties>\n    {}\n</ThingProperties>",
            stored_entry(property, value)
        );
        return append_child(doc, doc.entity, &xml, 0);
    };
    if blocks.len() > 1 {
        return Err(invalid(format!(
            "{} has more than one ThingProperties",
            doc.label()
        )));
    }
    let inner = doc.inner(block)?;
    let mut entries: BTreeMap<String, String> = BTreeMap::new();
    let mut at = block + 1;
    while at < doc.tokens.len() && doc.tokens[at].span.start < inner.end {
        if matches!(doc.tokens[at].kind, TokenKind::Start | TokenKind::Empty) {
            let span = doc.span(at)?;
            let name = String::from_utf8_lossy(doc.tokens[at].name.of(&doc.src)).into_owned();
            let text = String::from_utf8_lossy(span.of(&doc.src)).into_owned();
            entries.insert(name, text);
            while at < doc.tokens.len() && doc.tokens[at].span.start < span.end {
                at += 1;
            }
            continue;
        }
        at += 1;
    }
    match value {
        Some(_) if entries.contains_key(property) => return Ok(doc.src.clone()),
        Some(value) => {
            entries.insert(property.to_string(), stored_entry(property, value));
        }
        None if entries.remove(property).is_none() => return Ok(doc.src.clone()),
        None => {}
    }
    let indent = doc.indent_of(block);
    let names: Vec<String> = entries.keys().cloned().collect();
    let mut body: String = java_hash_map_order(&names)
        .into_iter()
        .map(|name| format!("\n{indent}    {}", entries[&name]))
        .collect();
    if !body.is_empty() {
        body.push('\n');
        body.push_str(&indent);
    }
    if doc.tokens[block].kind == TokenKind::Empty {
        let element = format!("<ThingProperties>{body}</ThingProperties>");
        return splice_one(&doc.src, doc.tokens[block].span, element);
    }
    splice_one(&doc.src, inner, body)
}

/// One stored value, as a 10.1 server writes it for a property it has just met.
fn stored_entry(property: &str, value: &str) -> String {
    format!(
        "<{property}><Value>{value}</Value><Timestamp>1970-01-01T00:00:00.000Z</Timestamp>\
         <Quality>UNKNOWN</Quality></{property}>"
    )
}

/// `names`, inserted in sorted order, in the order a `java.util.HashMap<String, _>` iterates
/// them: by hash bucket, then by insertion.
fn java_hash_map_order(names: &[String]) -> Vec<String> {
    let mut capacity = 16usize;
    while names.len() * 4 > capacity * 3 {
        capacity *= 2;
    }
    let bucket = |name: &str| {
        let hash = name.encode_utf16().fold(0u32, |hash, unit| {
            hash.wrapping_mul(31).wrapping_add(u32::from(unit))
        });
        ((hash ^ (hash >> 16)) as usize) & (capacity - 1)
    };
    let mut ordered: Vec<(usize, usize, &String)> = names
        .iter()
        .enumerate()
        .map(|(at, name)| (bucket(name), at, name))
        .collect();
    ordered.sort();
    ordered
        .into_iter()
        .map(|(_, _, name)| name.clone())
        .collect()
}

/// Whether a script in `src` reads `me.<member>` (and not `me.<member>Other`).
fn reads_me_member(src: &[u8], member: &str) -> bool {
    let Ok(tokens) = scan::tokenize(src) else {
        return false;
    };
    let needle = format!("me.{member}");
    scan::cdata_sections(&tokens).iter().any(|token| {
        let code = String::from_utf8_lossy(token.inner.of(src));
        code.match_indices(&needle).any(|(at, _)| {
            let before = code[..at].chars().last();
            let before_ok = !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
            let after = code[at + needle.len()..].chars().next();
            before_ok && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    })
}

// ---------------------------------------------------------------------------------------------
// Subscriptions

struct Source<'a> {
    /// The Thing subscribed to; none is the entity itself.
    thing: Option<&'a str>,
    property: &'a str,
}

/// The events and properties `target` has: its own, what it inherits, and every Thing's.
fn events_and_properties(
    model: &Model,
    target: &Doc,
) -> Result<(BTreeSet<String>, BTreeSet<String>), EditError> {
    let mut events: BTreeSet<String> = THING_EVENTS.iter().map(|e| e.to_string()).collect();
    if model.is_a(target, "Timer") {
        events.insert("Timer".into());
    }
    if model.is_a(target, "Scheduler") {
        events.insert("ScheduledEvent".into());
    }
    let host = target.members()?;
    events.extend(target.names_in(host, "EventDefinitions", "EventDefinition"));
    let mut properties: BTreeSet<String> = target
        .names_in(host, "PropertyDefinitions", "PropertyDefinition")
        .into_iter()
        .collect();
    for name in model.inherited_names(target) {
        if let Some(file) = model.ancestor(&name) {
            let ancestor = Doc::read(file)?;
            let host = ancestor.members()?;
            events.extend(ancestor.names_in(host, "EventDefinitions", "EventDefinition"));
            properties.extend(ancestor.names_in(host, "PropertyDefinitions", "PropertyDefinition"));
        } else if let Some(meta) = model.platform_meta(&name) {
            properties.extend(meta.properties.keys().cloned());
        }
    }
    Ok((events, properties))
}

fn subscription_remove(doc: &Doc, name: &str, plan: &mut Plan) -> Result<(), EditError> {
    doc.require(
        &MEMBER_HOSTS,
        "subscriptions belong to a Thing, ThingTemplate or ThingShape",
    )?;
    let list = doc.only_child(doc.members()?, "Subscriptions")?;
    let element = doc
        .named(list, "Subscription", name)
        .ok_or_else(|| EditError::Unknown(format!("{} has no subscription {name}", doc.label())))?;
    plan.summary = format!("remove subscription {name}");
    plan.changes
        .push(doc.change(remove_child(doc, list, element)?));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Implemented shapes

// ---------------------------------------------------------------------------------------------
// Timers and schedulers

struct Settings<'a> {
    /// The update rate (`N.0` milliseconds) or the cron string.
    value: Option<String>,
    run_as: Option<&'a str>,
    enabled: Option<bool>,
}

fn settings_set(
    doc: &Doc,
    kind: Kind,
    change: &Settings<'_>,
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["Things"], "timers and schedulers are Things")?;
    let template = if kind == Kind::Timer {
        "Timer"
    } else {
        "Scheduler"
    };
    if doc.attr(doc.entity, "thingTemplate").as_deref() != Some(template) {
        return Err(invalid(format!(
            "{} is not a {template} Thing",
            doc.label()
        )));
    }
    if change.value.is_none() && change.run_as.is_none() && change.enabled.is_none() {
        return Err(invalid(
            "nothing to change: give a new value, --run-as, --enabled or --disabled",
        ));
    }
    if change
        .run_as
        .is_some_and(|user| user.is_empty() || user.contains("]]>"))
    {
        return Err(invalid("--run-as must be a user name"));
    }
    let tables = doc.only_child(doc.entity, "ConfigurationTables")?;
    let settings = doc
        .named(tables, "ConfigurationTable", "Settings")
        .ok_or_else(|| invalid(format!("{} has no Settings table", doc.label())))?;
    let row = scan::tags_within(&doc.tokens, &doc.src, "Row", settings)
        .first()
        .copied()
        .ok_or_else(|| invalid(format!("{} has no Settings row", doc.label())))?;
    let field = if kind == Kind::Timer {
        "updateRate"
    } else {
        "schedule"
    };
    let value = change.value.as_ref().map(|value| match kind {
        Kind::Timer => value.clone(),
        _ => entity_new::cdata(value),
    });
    let values = [
        ("enabled", change.enabled.map(|on| on.to_string())),
        ("runAsUser", change.run_as.map(entity_new::cdata)),
        (field, value),
    ];
    let mut edits = Vec::new();
    let mut said = Vec::new();
    for (tag, written) in values {
        let Some(written) = written else {
            continue;
        };
        let element = scan::child_tags(&doc.tokens, &doc.src, tag, row)
            .first()
            .copied()
            .ok_or_else(|| invalid(format!("the Settings row has no {tag}")))?;
        let edit = if doc.tokens[element].kind == TokenKind::Empty {
            Edit::new(
                doc.tokens[element].span,
                format!("<{tag}>{written}</{tag}>").into_bytes(),
            )
        } else {
            Edit::new(doc.inner(element)?, written.into_bytes())
        };
        edits.push(edit);
        said.push(tag);
    }
    plan.summary = format!("set {} {}", kind.word(), said.join(", "));
    let after = splice_all(&doc.src, edits)?;
    plan.changes.push(doc.change(after));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Content: styles, media, states

fn content(doc: &Doc) -> Result<usize, EditError> {
    doc.only_child(doc.entity, "content")
}

fn style_set(
    solution: &Solution,
    doc: &Doc,
    change: &StyleChange,
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["StyleDefinitions"], "style set changes a StyleDefinition")?;
    let mut check = NewRequest::blank(Kind::Style, doc.name());
    let given = |value: &Option<String>| value.clone().filter(|value| !value.is_empty());
    check.background = given(&change.background);
    check.foreground = given(&change.foreground);
    check.secondary_background = given(&change.secondary_background);
    check.line_color = given(&change.line_color);
    check.line_style = change.line_style.clone();
    check.line_thickness = change.line_thickness.clone();
    check.text_size = change.text_size.clone();
    check.image = given(&change.image);
    entity_new::validate_style(solution, &check)?;
    let element = content(doc)?;
    let mut value: Value = serde_json::from_str(&doc.text(element)?)
        .map_err(|error| invalid(format!("{} content is not JSON: {error}", doc.label())))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| invalid(format!("{} content is not a JSON object", doc.label())))?;
    let mut changed = Vec::new();
    let colors = [
        ("backgroundColor", &change.background),
        ("foregroundColor", &change.foreground),
        ("secondaryBackgroundColor", &change.secondary_background),
        ("lineColor", &change.line_color),
    ];
    for (key, color) in colors {
        if let Some(color) = color {
            let written = if color.is_empty() {
                String::new()
            } else {
                entity_new::color(color)?
            };
            object.insert(key.into(), Value::String(written));
            changed.push(key);
        }
    }
    let flags = [
        ("fontEmphasisBold", change.bold),
        ("fontEmphasisItalic", change.italic),
        ("fontEmphasisUnderline", change.underline),
    ];
    for (key, flag) in flags {
        if let Some(flag) = flag {
            object.insert(key.into(), Value::Bool(flag));
            changed.push(key);
        }
    }
    let texts = [
        ("lineStyle", &change.line_style),
        ("textSize", &change.text_size),
        ("image", &change.image),
        ("displayString", &change.display_string),
    ];
    for (key, text) in texts {
        if let Some(text) = text {
            if text.contains("]]>") {
                return Err(invalid(format!("{key} cannot contain `]]>`")));
            }
            object.insert(key.into(), Value::String(text.clone()));
            changed.push(key);
        }
    }
    if let Some(thickness) = &change.line_thickness {
        let number: Value = serde_json::from_str(thickness).map_err(invalid)?;
        object.insert("lineThickness".into(), number);
        changed.push("lineThickness");
    }
    if changed.is_empty() {
        return Err(invalid("nothing to change: give one or more style flags"));
    }
    let written = entity_new::cdata(&entity_new::jackson_pretty(&value));
    plan.summary = format!("set {}", changed.join(", "));
    let after = splice_one(&doc.src, doc.inner(element)?, written)?;
    plan.changes.push(doc.change(after));
    Ok(())
}

fn media_set(doc: &Doc, file: &Path, plan: &mut Plan) -> Result<(), EditError> {
    doc.require(&["MediaEntities"], "media set changes a MediaEntity")?;
    let written = entity_new::cdata(&entity_new::media_content(file)?);
    let element = content(doc)?;
    plan.summary = format!("replace the content with {}", file.display());
    let after = splice_one(&doc.src, doc.inner(element)?, written)?;
    plan.changes.push(doc.change(after));
    Ok(())
}

fn state_set(
    doc: &Doc,
    state_type: &str,
    states: &[String],
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["StateDefinitions"], "state set changes a StateDefinition")?;
    let mut request = NewRequest::blank(Kind::State, doc.name());
    request.state_type = Some(state_type.to_string());
    request.states = states.to_vec();
    entity_new::validate_state_request(&request)?;
    let written = entity_new::cdata(&entity_new::render_state_content(&request));
    let element = content(doc)?;
    plan.summary = format!("set {} {state_type} states", states.len());
    let after = splice_one(&doc.src, doc.inner(element)?, written)?;
    plan.changes.push(doc.change(after));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Menus

/// A menu item: its title, target mashup, and whether it is the default.
type Item = (String, String, bool);

fn menu_items(doc: &Doc) -> Result<Vec<Item>, EditError> {
    let list = doc.only_child(doc.entity, "MenuItems")?;
    let mut items = Vec::new();
    for item in scan::child_tags(&doc.tokens, &doc.src, "MenuItem", list) {
        let kind = doc.attr(item, "linkType").unwrap_or_default();
        if kind != "Mashup" {
            return Err(invalid(format!(
                "{} has a {kind} item; twaco edits menus of mashup items only",
                doc.label()
            )));
        }
        items.push((
            doc.attr(item, "title").unwrap_or_default(),
            doc.attr(item, "linkDestination").unwrap_or_default(),
            doc.attr(item, "isDefault").as_deref() == Some("true"),
        ));
    }
    Ok(items)
}

fn menu_item_add(
    found: &[EntityFile],
    doc: &Doc,
    item: Item,
    before: Option<&str>,
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["Menus"], "menu item add changes a Menu")?;
    let (title, mashup, default) = &item;
    let bad = |text: &str| text.is_empty() || text.contains("]]>");
    if bad(title) || bad(mashup) {
        return Err(invalid(
            "a menu item is <Title>=<Mashup>, neither empty nor holding `]]>`",
        ));
    }
    let known = found
        .iter()
        .any(|file| file.info.collection == "Mashups" && &file.info.name == mashup);
    if !known {
        plan.warnings.push(format!(
            "mashup {mashup} is not in the solution; it must exist on the server"
        ));
    }
    let mut items = menu_items(doc)?;
    if items.iter().any(|(known, _, _)| known == title) {
        return Err(EditError::Exists(format!(
            "{} already has an item {title}",
            doc.label()
        )));
    }
    let at = match before {
        None => items.len(),
        Some(anchor) => items
            .iter()
            .position(|(known, _, _)| known == anchor)
            .ok_or_else(|| EditError::Unknown(format!("{} has no item {anchor}", doc.label())))?,
    };
    if *default {
        for item in &mut items {
            item.2 = false;
        }
    }
    plan.summary = format!("add menu item {title} -> {mashup}");
    items.insert(at, item);
    plan.changes.push(doc.change(write_menu(doc, &items)?));
    Ok(())
}

fn menu_item_remove(doc: &Doc, title: &str, plan: &mut Plan) -> Result<(), EditError> {
    doc.require(&["Menus"], "menu item remove changes a Menu")?;
    let mut items = menu_items(doc)?;
    let count = items.len();
    items.retain(|(known, _, _)| known != title);
    if items.len() == count {
        return Err(EditError::Unknown(format!(
            "{} has no item {title}",
            doc.label()
        )));
    }
    plan.summary = format!("remove menu item {title}");
    plan.changes.push(doc.change(write_menu(doc, &items)?));
    Ok(())
}

/// Rewrite `<MenuItems>` and the item rows of `<effectiveMenu>` (its root row stays) with the
/// writers `twaco new menu` uses, so ids and order stay in step.
fn write_menu(doc: &Doc, items: &[Item]) -> Result<Vec<u8>, EditError> {
    let list = doc.only_child(doc.entity, "MenuItems")?;
    let effective = doc.only_child(doc.entity, "effectiveMenu")?;
    let rows = doc.only_child(effective, "Rows")?;
    let root = *scan::child_tags(&doc.tokens, &doc.src, "Row", rows)
        .first()
        .ok_or_else(|| invalid(format!("{} effectiveMenu has no root row", doc.label())))?;
    let list_indent = doc.indent_of(list);
    let rows_indent = doc.indent_of(rows);
    let item_xml: String = items
        .iter()
        .map(|(title, target, default)| entity_new::menu_item_xml(title, target, *default))
        .collect();
    let row_xml: String = items
        .iter()
        .enumerate()
        .map(|(at, (title, target, default))| {
            entity_new::menu_row_xml(doc.name(), at, title, target, *default)
        })
        .collect();
    let item_xml = reindent_block(&item_xml, 16, &format!("{list_indent}    "));
    let row_xml = reindent_block(&row_xml, 20, &format!("{rows_indent}    "));
    let items_body = if items.is_empty() {
        String::new()
    } else {
        format!("{item_xml}\n{list_indent}")
    };
    let root_end = doc.span(root)?.end;
    let rows_end = doc.inner(rows)?.end;
    let edits = vec![
        Edit::new(doc.inner(list)?, items_body.into_bytes()),
        Edit::new(
            Span::new(root_end, rows_end),
            format!("{row_xml}\n{rows_indent}").into_bytes(),
        ),
    ];
    splice_all(&doc.src, edits)
}

/// Reindent a block of lines that each begin with a newline (the menu writers' shape).
fn reindent_block(xml: &str, from: usize, to: &str) -> String {
    xml.split('\n')
        .skip(1)
        .map(|line| format!("\n{}", reindent(line, from, to)))
        .collect()
}

#[cfg(test)]
mod tests;
