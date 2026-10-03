//! Change a Thing's template, or a template's base template, or the shapes either implements, in
//! the repository, and say what that does to everything below it before it is done.
//!
//! The platform changes a Thing's effective members when its template changes: properties,
//! services and configuration tables it inherited from the old parents are gone, and what the new
//! parents declare appears. Nothing warns about what the Thing still holds for the lost ones. This
//! computes it from the repository's own inheritance model: members gained and lost for the entity
//! and everything inheriting it, stored property values and configuration-table rows that would be
//! left with no definition, and references (scripts, bindings, alerts, mashups) to a lost member.
//! A loss that holds data or is still referenced is refused unless `--accept-loss`.
//!
//! The only edits are the `thingTemplate` / `baseThingTemplate` attribute and the entity's
//! `ImplementedShape` elements, made in place; nothing else in the document moves.

use super::catalog;
use super::config::Solution;
use super::relocate::{indent_at, line_bounds, starts_line};
use super::rename::{self, Kind as RenameKind, Spec};
use super::scan::{self, Kind as TokenKind, Span, Token};
use super::sidecar;
use super::splice::{self, Edit};
use super::types::{self, Entity};
use super::workspace::{self, EntityFile};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// More lost members than this are listed but not traced through the workspace.
const TRACE_LIMIT: usize = 25;

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub entity: String,
    /// The new `thingTemplate` of a Thing, or `baseThingTemplate` of a template.
    pub template: Option<String>,
    pub add_shapes: Vec<String>,
    pub remove_shapes: Vec<String>,
    /// Go ahead although stored data or references would be left without a definition.
    pub accept_loss: bool,
}

#[derive(Debug)]
pub enum RetemplateError {
    Invalid(String),
    Unknown { name: String },
    Unreadable { files: Vec<String> },
    Xml { path: PathBuf, why: String },
    /// A loss that needs `--accept-loss`.
    Loss { reasons: Vec<String> },
    Apply { path: PathBuf, why: String },
}

impl fmt::Display for RetemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RetemplateError::Invalid(why) => f.write_str(why),
            RetemplateError::Unknown { name } => write!(f, "no Thing or ThingTemplate named {name} in this solution"),
            RetemplateError::Unreadable { files } => write!(f, "cannot plan with unreadable files: {}", files.join(", ")),
            RetemplateError::Xml { path, why } => write!(f, "{}: {why}", path.display()),
            RetemplateError::Loss { reasons } => write!(f, "this would leave data or references without a definition ({}); pass --accept-loss to go ahead", reasons.join("; ")),
            RetemplateError::Apply { path, why } => write!(f, "{}: {why}", path.display()),
        }
    }
}

impl std::error::Error for RetemplateError {}

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    /// `service`, `property` or `configuration table`.
    pub kind: &'static str,
    pub name: String,
    /// Where it was declared before (lost) or is declared after (gained).
    pub declared_on: String,
    /// The entities (the changed one and what inherits it) that lose or gain it.
    pub entities: Vec<String>,
    /// Stored values or rows left behind with no definition (lost members only).
    pub orphaned: usize,
    /// References that would stop resolving (lost members only).
    pub references: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub request: Request,
    pub collection: String,
    pub file: PathBuf,
    old: Vec<u8>,
    new: Vec<u8>,
    /// The changed entity and everything inheriting from it.
    pub affected: Vec<String>,
    pub gained: Vec<Change>,
    pub lost: Vec<Change>,
    pub notes: Vec<String>,
    /// Why this needs `--accept-loss`; empty when it does not.
    pub blocked: Vec<String>,
}

type Effective = BTreeMap<(&'static str, String), String>;

fn xml(path: &Path, error: impl fmt::Display) -> RetemplateError {
    RetemplateError::Xml { path: path.to_path_buf(), why: error.to_string() }
}

/// What an entity has: its own members and those of everything it inherits, nearest first.
fn effective(entities: &[Entity], entity: &Entity) -> Effective {
    let mut out = Effective::new();
    for (kind, name) in entity.member_list() {
        out.entry((kind, name.to_string())).or_insert_with(|| entity.name.clone());
    }
    for ancestor in catalog::inheritance_names(entity, entities) {
        if let Some(parent) = entities.iter().find(|item| item.name == ancestor && matches!(item.collection.as_str(), "ThingTemplates" | "ThingShapes")) {
            for (kind, name) in parent.member_list() {
                out.entry((kind, name.to_string())).or_insert_with(|| parent.name.clone());
            }
        }
    }
    out
}

/// The names of the configuration tables an entity document defines.
fn config_table_names(bytes: &[u8]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Ok(tokens) = scan::tokenize(bytes) else { return names };
    for section in tokens.iter().enumerate().filter(|(_, token)| matches!(token.kind, TokenKind::Start | TokenKind::Empty) && token.name.of(bytes) == b"ConfigurationTableDefinitions").map(|(at, _)| at) {
        for definition in scan::child_tags(&tokens, bytes, "ConfigurationTableDefinition", section) {
            if let Ok(Some(value)) = scan::attribute(bytes, &tokens[definition], "name") {
                names.insert(scan::decode_entities(&String::from_utf8_lossy(value.of(bytes))));
            }
        }
    }
    names
}

/// The names a Thing or template holds stored values for, and the configuration tables it holds.
fn held(bytes: &[u8]) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut values, mut tables) = (BTreeSet::new(), BTreeSet::new());
    let Ok(tokens) = scan::tokenize(bytes) else { return (values, tables) };
    let Some(entity) = sidecar::entity_element(&tokens, bytes) else { return (values, tables) };
    if let Some(&section) = scan::child_tags(&tokens, bytes, "ThingProperties", entity).first() {
        if let Some(end) = scan::element_end(&tokens, section) {
            let mut index = section + 1;
            while index < end {
                match tokens[index].kind {
                    TokenKind::Start => {
                        values.insert(String::from_utf8_lossy(tokens[index].name.of(bytes)).into_owned());
                        index = scan::element_end(&tokens, index).map_or(end, |e| e + 1);
                    }
                    TokenKind::Empty => {
                        values.insert(String::from_utf8_lossy(tokens[index].name.of(bytes)).into_owned());
                        index += 1;
                    }
                    _ => index += 1,
                }
            }
        }
    }
    for section in scan::child_tags(&tokens, bytes, "ConfigurationTables", entity) {
        for table in scan::child_tags(&tokens, bytes, "ConfigurationTable", section) {
            if let Ok(Some(value)) = scan::attribute(bytes, &tokens[table], "name") {
                tables.insert(scan::decode_entities(&String::from_utf8_lossy(value.of(bytes))));
            }
        }
    }
    (values, tables)
}

fn config_tables_of(files: &[EntityFile], entities: &[Entity], entity: &Entity) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let names = std::iter::once(entity.name.clone()).chain(catalog::inheritance_names(entity, entities));
    for name in names {
        let Some(item) = entities.iter().find(|item| item.name == name && matches!(item.collection.as_str(), "Things" | "ThingTemplates" | "ThingShapes")) else { continue };
        let Some(file) = files.iter().find(|file| file.info.name == name && file.info.collection == item.collection) else { continue };
        if let Ok(bytes) = std::fs::read(&file.path) {
            for table in config_table_names(&bytes) {
                out.entry(table).or_insert_with(|| name.clone());
            }
        }
    }
    out
}

pub fn plan(solution: &Solution, request: &Request) -> Result<Plan, RetemplateError> {
    if request.template.is_none() && request.add_shapes.is_empty() && request.remove_shapes.is_empty() {
        return Err(RetemplateError::Invalid("nothing to change: give --to <template>, --add-shapes or --remove-shapes".to_string()));
    }
    let mut discovery = workspace::discover(solution);
    if !discovery.unreadable.is_empty() {
        return Err(RetemplateError::Unreadable { files: discovery.unreadable });
    }
    discovery.entities.sort_by(|a, b| a.path.cmp(&b.path));
    discovery.entities.dedup_by(|a, b| a.path == b.path);
    let found: Vec<&EntityFile> = discovery.entities.iter().filter(|item| item.info.name == request.entity && matches!(item.info.collection.as_str(), "Things" | "ThingTemplates")).collect();
    let [file] = found.as_slice() else {
        return Err(RetemplateError::Unknown { name: request.entity.clone() });
    };
    let (model, skipped) = types::load_model(solution);
    if !skipped.is_empty() {
        return Err(RetemplateError::Unreadable { files: skipped });
    }
    let entities = &model.entities;
    let Some(target) = entities.iter().find(|item| item.name == request.entity && item.collection == file.info.collection) else {
        return Err(RetemplateError::Unknown { name: request.entity.clone() });
    };
    let mut notes = Vec::new();

    // What the entity will inherit from.
    let mut after_target = target.clone();
    if let Some(template) = &request.template {
        if target.template.as_deref() == Some(template.as_str()) {
            return Err(RetemplateError::Invalid(format!("{} already has template {template}", request.entity)));
        }
        match entities.iter().find(|item| &item.name == template && item.collection == "ThingTemplates") {
            Some(parent) => {
                if target.collection == "ThingTemplates" && (parent.name == target.name || catalog::inheritance_names(parent, entities).contains(&target.name)) {
                    return Err(RetemplateError::Invalid(format!("{template} inherits from {}, so making it the base would be a cycle", request.entity)));
                }
            }
            None => notes.push(format!("{template} is not in this solution (a platform template, or defined elsewhere), so what it declares is not known here and only what the old parents lose is reported.")),
        }
        after_target.template = Some(template.clone());
    }
    for shape in &request.add_shapes {
        if target.shapes.contains(shape) {
            return Err(RetemplateError::Invalid(format!("{} already implements {shape}", request.entity)));
        }
        if !entities.iter().any(|item| &item.name == shape && item.collection == "ThingShapes") {
            notes.push(format!("{shape} is not in this solution, so what it declares is not known here."));
        }
        after_target.shapes.push(shape.clone());
    }
    for shape in &request.remove_shapes {
        if !target.shapes.contains(shape) {
            return Err(RetemplateError::Invalid(format!("{} does not itself implement {shape} (it may inherit it from its template)", request.entity)));
        }
        after_target.shapes.retain(|item| item != shape);
    }
    let mut after: Vec<Entity> = entities.clone();
    if let Some(slot) = after.iter_mut().find(|item| item.name == target.name && item.collection == target.collection) {
        *slot = after_target;
    }

    // Everything whose inherited members can change: the entity and what inherits it.
    let affected: Vec<&Entity> = entities
        .iter()
        .filter(|item| item.collection == "Things" || item.collection == "ThingTemplates")
        .filter(|item| (item.name == target.name && item.collection == target.collection) || catalog::inheritance_names(item, entities).contains(&target.name))
        .collect();
    let mut lost: BTreeMap<(&'static str, String), Change> = BTreeMap::new();
    let mut gained: BTreeMap<(&'static str, String), Change> = BTreeMap::new();
    for entity in &affected {
        let now = after.iter().find(|item| item.name == entity.name && item.collection == entity.collection).expect("every affected entity is in the copy");
        let (before_members, after_members) = (effective(entities, entity), effective(&after, now));
        let (before_tables, after_tables) = (config_tables_of(&discovery.entities, entities, entity), config_tables_of(&discovery.entities, &after, now));
        let record = |map: &mut BTreeMap<(&'static str, String), Change>, kind: &'static str, name: &str, declared_on: &str| {
            map.entry((kind, name.to_string())).or_insert_with(|| Change { kind, name: name.to_string(), declared_on: declared_on.to_string(), entities: Vec::new(), orphaned: 0, references: Vec::new() }).entities.push(entity.name.clone());
        };
        for (key, declared_on) in &before_members {
            if !after_members.contains_key(key) {
                record(&mut lost, key.0, &key.1, declared_on);
            }
        }
        for (key, declared_on) in &after_members {
            if !before_members.contains_key(key) {
                record(&mut gained, key.0, &key.1, declared_on);
            }
        }
        for (name, declared_on) in &before_tables {
            if !after_tables.contains_key(name) {
                record(&mut lost, "configuration table", name, declared_on);
            }
        }
        for (name, declared_on) in &after_tables {
            if !before_tables.contains_key(name) {
                record(&mut gained, "configuration table", name, declared_on);
            }
        }
    }

    // Stored values and rows with no definition left.
    let mut file_of: BTreeMap<String, &EntityFile> = BTreeMap::new();
    for entity in &affected {
        if let Some(file) = discovery.entities.iter().find(|item| item.info.name == entity.name && item.info.collection == entity.collection) {
            file_of.insert(entity.name.clone(), file);
        }
    }
    for change in lost.values_mut() {
        for name in &change.entities {
            let Some(file) = file_of.get(name) else { continue };
            let Ok(bytes) = std::fs::read(&file.path) else { continue };
            let (values, tables) = held(&bytes);
            let present = if change.kind == "property" { values.contains(&change.name) } else if change.kind == "configuration table" { tables.contains(&change.name) } else { false };
            change.orphaned += usize::from(present);
        }
    }

    // References to a lost member, from what the rename machinery already finds.
    if lost.len() > TRACE_LIMIT {
        notes.push(format!("{} members would be lost: too many to trace references for; search for the ones you rely on.", lost.len()));
    } else {
        let affected_names: BTreeSet<&str> = affected.iter().map(|entity| entity.name.as_str()).collect();
        for change in lost.values_mut().filter(|change| matches!(change.kind, "service" | "property")) {
            let kind = if change.kind == "service" { RenameKind::Service } else { RenameKind::Property };
            let probe = Spec { kind, old: change.name.clone(), new: "TwacoProbeName".to_string(), scope: Some(change.declared_on.clone()), service: None };
            let Ok(planned) = rename::plan(solution, &probe) else { continue };
            for item in planned.changes.iter().chain(&planned.outside) {
                let declaring = discovery.entities.iter().find(|entity| entity.info.name == change.declared_on);
                if declaring.is_some_and(|entity| item.path == entity.path || item.path.starts_with(workspace::services_dir(solution, entity))) {
                    continue;
                }
                let in_affected = discovery.entities.iter().any(|entity| affected_names.contains(entity.info.name.as_str()) && (item.path == entity.path || item.path.starts_with(workspace::services_dir(solution, entity))));
                for finding in item.findings.iter().filter(|finding| matches!(finding.tier, super::refs::Tier::Exact | super::refs::Tier::Embedded)) {
                    let names_affected = affected_names.iter().any(|name| finding.excerpt.contains(&format!("\"{name}\"")));
                    if in_affected || names_affected {
                        change.references.push(format!("{}:{}  {}", item.path.strip_prefix(&solution.root).unwrap_or(&item.path).display().to_string().replace('\\', "/"), finding.line, finding.excerpt));
                    }
                }
            }
        }
    }

    let lost: Vec<Change> = lost.into_values().collect();
    let gained: Vec<Change> = gained.into_values().collect();
    let mut blocked = Vec::new();
    for change in &lost {
        if change.orphaned > 0 {
            blocked.push(format!("{} {} is held by {} entit{} with no definition left", change.kind, change.name, change.orphaned, if change.orphaned == 1 { "y" } else { "ies" }));
        }
        if !change.references.is_empty() {
            blocked.push(format!("{} {} is still referenced {} time(s)", change.kind, change.name, change.references.len()));
        }
    }

    // The edits.
    let old = std::fs::read(&file.path).map_err(|error| xml(&file.path, error))?;
    let new = edit_document(&old, file, request).map_err(|why| xml(&file.path, why))?;
    if scan::tokenize(&new).is_err() {
        return Err(xml(&file.path, "the edited document would not read back; nothing was written"));
    }
    if file.info.collection == "Things" || file.info.collection == "ThingTemplates" {
        sidecar::extract_services(&new).map_err(|error| xml(&file.path, error))?;
    }

    Ok(Plan {
        request: request.clone(),
        collection: file.info.collection.clone(),
        file: file.path.clone(),
        old,
        new,
        affected: affected.iter().map(|entity| entity.name.clone()).collect(),
        gained,
        lost,
        notes,
        blocked,
    })
}

fn attribute_value(src: &[u8], token: &Token, name: &str) -> Option<Span> {
    scan::attribute(src, token, name).ok().flatten()
}

/// The document with the template attribute and the implemented shapes changed, in place.
fn edit_document(src: &[u8], file: &EntityFile, request: &Request) -> Result<Vec<u8>, String> {
    let tokens = scan::tokenize(src).map_err(|error| error.to_string())?;
    let entity = sidecar::entity_element(&tokens, src).ok_or("not an entity document")?;
    let mut edits = Vec::new();
    if let Some(template) = &request.template {
        let attribute = if file.info.collection == "Things" { "thingTemplate" } else { "baseThingTemplate" };
        let span = attribute_value(src, &tokens[entity], attribute).ok_or_else(|| format!("the entity has no {attribute} attribute to change"))?;
        edits.push(Edit::new(span, template.as_bytes().to_vec()));
    }
    if !request.add_shapes.is_empty() || !request.remove_shapes.is_empty() {
        let host = sidecar::member_host_of(&tokens, src).ok_or("the entity has no member host")?;
        let section = [entity, host].into_iter().find_map(|parent| scan::child_tags(&tokens, src, "ImplementedShapes", parent).first().copied().map(|at| (parent, at)));
        // Remove: drop the element's line.
        for shape in &request.remove_shapes {
            let (_, section_at) = section.ok_or("the entity has no ImplementedShapes section")?;
            let element = scan::child_tags(&tokens, src, "ImplementedShape", section_at).into_iter().find(|&at| {
                attribute_value(src, &tokens[at], "name").is_some_and(|value| scan::decode_entities(&String::from_utf8_lossy(value.of(src))) == *shape)
            });
            let Some(element) = element else { return Err(format!("{shape} is not listed in ImplementedShapes")) };
            let span = scan::element_span(&tokens, element).ok_or("malformed ImplementedShape")?;
            edits.push(Edit::new(line_bounds(src, span), Vec::new()));
        }
        // Add: all in one insertion, in the style of the section.
        let newline = if src.windows(2).any(|pair| pair == b"\r\n") { "\r\n" } else { "\n" };
        let elements: Vec<String> = request.add_shapes.iter().map(|shape| format!("<ImplementedShape name=\"{shape}\"></ImplementedShape>")).collect();
        if !elements.is_empty() {
            match section {
                Some((_, section_at)) if tokens[section_at].kind == TokenKind::Start => {
                    let close = scan::element_end(&tokens, section_at).ok_or("ImplementedShapes is not closed")?;
                    let section_indent = indent_at(src, tokens[section_at].span.start).len();
                    let children = scan::child_tags(&tokens, src, "ImplementedShape", section_at);
                    let indent = children.first().map_or(section_indent + 4, |&child| indent_at(src, tokens[child].span.start).len());
                    let close_start = tokens[close].span.start;
                    let pad = " ".repeat(indent);
                    let lines: String = elements.iter().map(|element| format!("{pad}{element}{newline}")).collect();
                    if starts_line(src, close_start) {
                        let mut line = close_start;
                        while line > 0 && src[line - 1] != b'\n' {
                            line -= 1;
                        }
                        edits.push(Edit::new(Span::new(line, line), lines.into_bytes()));
                    } else {
                        let text = format!("{newline}{lines}{}", " ".repeat(section_indent));
                        edits.push(Edit::new(Span::new(close_start, close_start), text.into_bytes()));
                    }
                }
                Some((_, section_at)) => {
                    let token = tokens[section_at];
                    let name = token.name.of(src);
                    if token.span.len() != name.len() + 3 {
                        return Err("ImplementedShapes is an empty element with attributes".to_string());
                    }
                    let section_indent = indent_at(src, token.span.start).len();
                    let pad = " ".repeat(section_indent + 4);
                    let lines: String = elements.iter().map(|element| format!("{pad}{element}{newline}")).collect();
                    let text = format!("<ImplementedShapes>{newline}{lines}{}</ImplementedShapes>", " ".repeat(section_indent));
                    edits.push(Edit::new(token.span, text.into_bytes()));
                }
                None => {
                    let end = scan::element_end(&tokens, entity).ok_or("the entity is not closed")?;
                    let first = (entity + 1..end).find(|&index| matches!(tokens[index].kind, TokenKind::Start | TokenKind::Empty)).ok_or("the entity has no content to place a section before")?;
                    let indent = indent_at(src, tokens[first].span.start).len();
                    let mut line = tokens[first].span.start;
                    while line > 0 && src[line - 1] != b'\n' {
                        line -= 1;
                    }
                    let pad = " ".repeat(indent);
                    let lines: String = elements.iter().map(|element| format!("{pad}    {element}{newline}")).collect();
                    let text = format!("{pad}<ImplementedShapes>{newline}{lines}{pad}</ImplementedShapes>{newline}");
                    edits.push(Edit::new(Span::new(line, line), text.into_bytes()));
                }
            }
        }
    }
    splice::splice(src, &edits).map_err(|error| error.to_string())
}

impl Plan {
    pub fn file_relative(&self, solution: &Solution) -> String {
        self.file.strip_prefix(&solution.root).unwrap_or(&self.file).display().to_string().replace('\\', "/")
    }
}

/// Write the plan. A file changed since the plan is refused; a loss needs `--accept-loss`.
pub fn apply(plan: &Plan) -> Result<(), RetemplateError> {
    if !plan.request.accept_loss && !plan.blocked.is_empty() {
        return Err(RetemplateError::Loss { reasons: plan.blocked.clone() });
    }
    match std::fs::read(&plan.file) {
        Ok(bytes) if bytes == plan.old => {}
        Ok(_) => return Err(RetemplateError::Invalid(format!("{} changed since the plan was made; plan again", plan.file.display()))),
        Err(error) => return Err(xml(&plan.file, error)),
    }
    workspace::atomic_replace(&plan.file, &plan.new).map_err(|error| RetemplateError::Apply { path: plan.file.clone(), why: error.to_string() })
}

#[cfg(test)]
mod tests;
