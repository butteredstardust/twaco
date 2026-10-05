//! Move or copy a service or a property from one entity to another, in the repository.
//!
//! A Thing, ThingTemplate or ThingShape keeps its own services and properties as definition (and,
//! for a service, implementation) elements. This lifts those elements out of one entity's XML and
//! puts them into another's, byte for byte, re-indented to where they land; a script inside a
//! CDATA section is never re-indented. Nothing is written without `apply`, a name that the target,
//! anything it inherits, or anything that inherits it already uses is refused, and what moving
//! breaks for callers is reported.
//!
//! Moving up into something the source inherits keeps every call working. Moving anywhere else
//! does not: instances of the source no longer have the member. The plan lists those callers (the
//! same findings a rename of the member would make), and `--leave-delegate` leaves a service on the
//! source that calls the moved one on a Thing target, so callers keep working.
//!
//! Not carried: property values stored on Things (they stay where they are and are listed), and
//! what the server holds. The next `deploy` makes the server follow.

use super::catalog;
use super::config::Solution;
use super::lock::WorkspaceLock;
use super::refs;
use super::rename::{self, Kind as RenameKind, Spec};
use super::scan::{self, Kind as TokenKind, Span, Token};
use super::sidecar::{self, ServiceSidecar};
use super::splice::{self, Edit};
use super::sync;
use super::transaction::{Transaction, TransactionError};
use super::workspace::{self, EntityFile};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Member {
    Service,
    Property,
}

impl Member {
    pub fn word(self) -> &'static str {
        match self {
            Member::Service => "service",
            Member::Property => "property",
        }
    }

    pub fn from_word(word: &str) -> Option<Member> {
        match word {
            "service" => Some(Member::Service),
            "property" => Some(Member::Property),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub member: Member,
    /// Copy instead of move: the source keeps its member.
    pub copy: bool,
    pub from: String,
    pub to: String,
    pub name: String,
    /// The name the member has on the target.
    pub new_name: Option<String>,
    /// Leave a service on the source that calls the moved one (a Thing target only).
    pub leave_delegate: bool,
}

#[derive(Debug)]
pub enum RelocateError {
    Invalid(String),
    Unknown {
        name: String,
    },
    Unreadable {
        files: Vec<String>,
    },
    Xml {
        path: PathBuf,
        why: String,
    },
    NotDeclared {
        entity: String,
        member: Member,
        name: String,
    },
    Inherited {
        entity: String,
        name: String,
        declared_on: String,
    },
    Exists {
        conflicts: Vec<String>,
    },
    Refused(String),
    Apply {
        path: PathBuf,
        why: String,
    },
    Verification(String),
    /// The journaled write failed; the error says whether every change was undone.
    Write(TransactionError),
}

impl fmt::Display for RelocateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelocateError::Invalid(why)
            | RelocateError::Refused(why)
            | RelocateError::Verification(why) => f.write_str(why),
            RelocateError::Write(error) => error.fmt(f),
            RelocateError::Unknown { name } => write!(
                f,
                "no Thing, ThingTemplate or ThingShape named {name} in this solution"
            ),
            RelocateError::Unreadable { files } => {
                write!(f, "cannot plan with unreadable files: {}", files.join(", "))
            }
            RelocateError::Xml { path, why } => write!(f, "{}: {why}", path.display()),
            RelocateError::NotDeclared {
                entity,
                member,
                name,
            } => write!(f, "{entity} declares no {} named {name}", member.word()),
            RelocateError::Inherited {
                entity,
                name,
                declared_on,
            } => {
                write!(
                    f,
                    "{name} on {entity} is declared on {declared_on}; move it from there"
                )
            }
            RelocateError::Exists { conflicts } => {
                write!(f, "the name is already taken: {}", conflicts.join("; "))
            }
            RelocateError::Apply { path, why } => write!(f, "{}: {why}", path.display()),
        }
    }
}

impl std::error::Error for RelocateError {}

/// What moving the member breaks for callers.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Callers {
    /// Files holding references to the member that stop resolving.
    pub files: usize,
    pub references: usize,
    pub first: Vec<String>,
}

/// A planned relocation.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The solution root the paths below are inside of.
    root: PathBuf,
    pub request: Request,
    /// The member's name on the target.
    pub final_name: String,
    pub from_file: PathBuf,
    pub to_file: PathBuf,
    from_old: Vec<u8>,
    to_old: Vec<u8>,
    from_new: Option<Vec<u8>>,
    to_new: Vec<u8>,
    /// Sidecar directories (a service only): where the member's sidecar is written on the target,
    /// and removed from or rewritten on the source.
    to_sidecars: Option<PathBuf>,
    to_sidecar: Option<ServiceSidecar>,
    from_sidecars: Option<PathBuf>,
    from_sidecar_after: Option<ServiceSidecar>,
    pub callers: Callers,
    pub notes: Vec<String>,
}

fn sections(
    member: Member,
) -> (
    &'static str,
    &'static str,
    Option<(&'static str, &'static str)>,
) {
    match member {
        Member::Service => (
            "ServiceDefinitions",
            "ServiceDefinition",
            Some(("ServiceImplementations", "ServiceImplementation")),
        ),
        Member::Property => ("PropertyDefinitions", "PropertyDefinition", None),
    }
}

/// The order a ThingShape lists its sections in, for putting a missing one where Composer would.
const SECTION_ORDER: [&str; 6] = [
    "PropertyDefinitions",
    "ServiceDefinitions",
    "EventDefinitions",
    "ServiceMappings",
    "ServiceImplementations",
    "Subscriptions",
];

pub(crate) fn is_relocatable(collection: &str) -> bool {
    matches!(collection, "Things" | "ThingTemplates" | "ThingShapes")
}

pub(crate) fn find_entity<'a>(
    entities: &'a [EntityFile],
    name: &str,
) -> Result<&'a EntityFile, RelocateError> {
    let found: Vec<&EntityFile> = entities
        .iter()
        .filter(|item| item.info.name == name && is_relocatable(&item.info.collection))
        .collect();
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(RelocateError::Unknown {
            name: name.to_string(),
        }),
        _ => Err(RelocateError::Invalid(format!(
            "{name} is defined more than once in this solution"
        ))),
    }
}

impl From<TransactionError> for RelocateError {
    fn from(error: TransactionError) -> Self {
        match error {
            TransactionError::Invalid(why) => RelocateError::Invalid(why),
            TransactionError::Stale(why) => match (
                why.strip_suffix(" changed since it was read"),
                why.strip_suffix(" exists already"),
            ) {
                (Some(path), _) => RelocateError::Refused(format!(
                    "{path} changed since the plan was made; plan again"
                )),
                (_, Some(path)) => RelocateError::Exists {
                    conflicts: vec![path.to_string()],
                },
                _ => RelocateError::Refused(why),
            },
            other => RelocateError::Write(other),
        }
    }
}

fn xml(path: &Path, error: impl fmt::Display) -> RelocateError {
    RelocateError::Xml {
        path: path.to_path_buf(),
        why: error.to_string(),
    }
}

/// The leading spaces and tabs of the line holding `at`.
pub(crate) fn indent_at(src: &[u8], at: usize) -> &[u8] {
    let mut start = at;
    while start > 0 && src[start - 1] != b'\n' {
        start -= 1;
    }
    let mut end = start;
    while end < src.len() && (src[end] == b' ' || src[end] == b'\t') {
        end += 1;
    }
    &src[start..end]
}

/// Whether only spaces and tabs sit between the start of the line and `at`.
pub(crate) fn starts_line(src: &[u8], at: usize) -> bool {
    let mut position = at;
    while position > 0 && src[position - 1] != b'\n' {
        if src[position - 1] != b' ' && src[position - 1] != b'\t' {
            return false;
        }
        position -= 1;
    }
    true
}

/// `span` widened to whole lines when it is alone on them; otherwise unchanged.
pub(crate) fn line_bounds(src: &[u8], span: Span) -> Span {
    if !starts_line(src, span.start) {
        return span;
    }
    let mut start = span.start;
    while start > 0 && src[start - 1] != b'\n' {
        start -= 1;
    }
    let mut end = span.end;
    while end < src.len() && matches!(src[end], b' ' | b'\t' | b'\r') {
        end += 1;
    }
    if end >= src.len() {
        return Span::new(start, end);
    }
    if src[end] == b'\n' {
        return Span::new(start, end + 1);
    }
    span
}

/// The block's lines moved from indentation `from` to `to`, leaving CDATA payload lines alone.
fn reindent(block: &[u8], block_start: usize, cdata: &[Span], from: usize, to: usize) -> Vec<u8> {
    if from == to || block.contains(&b'\t') {
        return block.to_vec();
    }
    let mut out = Vec::with_capacity(block.len());
    let mut at = 0;
    while at < block.len() {
        let line_end = block[at..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(block.len(), |p| at + p + 1);
        let line = &block[at..line_end];
        let absolute = block_start + at;
        let in_cdata = cdata
            .iter()
            .any(|span| span.start < absolute && absolute < span.end);
        let blank = line.iter().all(|b| b.is_ascii_whitespace());
        if in_cdata || blank {
            out.extend_from_slice(line);
        } else if to > from {
            out.extend(std::iter::repeat_n(b' ', to - from));
            out.extend_from_slice(line);
        } else {
            let leading = line.iter().take_while(|&&b| b == b' ').count();
            out.extend_from_slice(&line[leading.min(from - to)..]);
        }
        at = line_end;
    }
    out
}

/// One member block as found in a document.
struct Block {
    /// The element, whole lines when it is alone on them.
    span: Span,
    own_lines: bool,
    indent: usize,
    /// The section's own indentation, to learn the nesting unit from.
    section_indent: usize,
    cdata: Vec<Span>,
}

fn block_of(
    tokens: &[Token],
    src: &[u8],
    section: usize,
    element: usize,
) -> Result<Block, scan::ScanError> {
    let span = scan::element_span(tokens, element).ok_or(scan::ScanError::Malformed {
        what: "member element",
        at: tokens[element].span.start,
    })?;
    let lines = line_bounds(src, span);
    let cdata = tokens
        .iter()
        .filter(|token| {
            token.kind == TokenKind::Cdata
                && token.span.start >= span.start
                && token.span.end <= span.end
        })
        .map(|token| token.span)
        .collect();
    Ok(Block {
        span: lines,
        own_lines: lines != span,
        indent: indent_at(src, span.start).len(),
        section_indent: indent_at(src, tokens[section].span.start).len(),
        cdata,
    })
}

/// A planned insertion of `block` (from `from_src`) into a target section.
fn insert_edit(
    tokens: &[Token],
    src: &[u8],
    host: usize,
    section: &'static str,
    block: &Block,
    block_bytes: &[u8],
) -> Result<Edit, String> {
    let unit = block.indent.saturating_sub(block.section_indent).max(1);
    let existing = scan::child_tags(tokens, src, section, host)
        .first()
        .copied();
    let newline = if block_bytes.windows(2).any(|pair| pair == b"\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    match existing {
        Some(at) if tokens[at].kind == TokenKind::Start => {
            let close = scan::element_end(tokens, at).ok_or("the section is not closed")?;
            let section_indent = indent_at(src, tokens[at].span.start).len();
            let first_child = scan::child_tags(
                tokens,
                src,
                if section == "PropertyDefinitions" {
                    "PropertyDefinition"
                } else if section == "ServiceDefinitions" {
                    "ServiceDefinition"
                } else {
                    "ServiceImplementation"
                },
                at,
            )
            .first()
            .copied();
            let target_indent = first_child.map_or(section_indent + unit, |child| {
                indent_at(src, tokens[child].span.start).len()
            });
            let moved = reindent(
                block_bytes,
                block.span.start,
                &block.cdata,
                block.indent,
                target_indent,
            );
            let close_start = tokens[close].span.start;
            if block.own_lines && starts_line(src, close_start) {
                let mut line = close_start;
                while line > 0 && src[line - 1] != b'\n' {
                    line -= 1;
                }
                Ok(Edit::new(Span::new(line, line), moved))
            } else if block.own_lines {
                // `<Section></Section>` on one line: open it up.
                let mut text = Vec::new();
                text.extend_from_slice(newline.as_bytes());
                text.extend_from_slice(&moved);
                text.extend(std::iter::repeat_n(b' ', section_indent));
                Ok(Edit::new(Span::new(close_start, close_start), text))
            } else {
                Ok(Edit::new(
                    Span::new(close_start, close_start),
                    block_bytes.to_vec(),
                ))
            }
        }
        Some(at) => {
            // `<Section/>`: only a bare empty element can be opened without guessing at attributes.
            let token = tokens[at];
            let name = token.name.of(src);
            if token.span.len() != name.len() + 3 {
                return Err(format!(
                    "{section} is an empty element with attributes; add a member in Composer first"
                ));
            }
            let section_indent = indent_at(src, token.span.start).len();
            let moved = reindent(
                block_bytes,
                block.span.start,
                &block.cdata,
                block.indent,
                section_indent + unit,
            );
            let mut text = Vec::new();
            text.push(b'<');
            text.extend_from_slice(name);
            text.push(b'>');
            text.extend_from_slice(newline.as_bytes());
            text.extend_from_slice(&moved);
            text.extend(std::iter::repeat_n(b' ', section_indent));
            text.extend_from_slice(b"</");
            text.extend_from_slice(name);
            text.push(b'>');
            Ok(Edit::new(token.span, text))
        }
        None => {
            // The section is missing: put a new one where Composer would, before the first later one.
            let order = SECTION_ORDER
                .iter()
                .position(|name| *name == section)
                .unwrap_or(0);
            let host_end = scan::element_end(tokens, host).ok_or("the entity is not closed")?;
            let mut children = Vec::new();
            let mut index = host + 1;
            while index < host_end {
                match tokens[index].kind {
                    TokenKind::Start => {
                        children.push(index);
                        index = scan::element_end(tokens, index).map_or(host_end, |e| e + 1);
                    }
                    TokenKind::Empty => {
                        children.push(index);
                        index += 1;
                    }
                    _ => index += 1,
                }
            }
            let child_indent = children
                .first()
                .map(|&child| indent_at(src, tokens[child].span.start).len());
            let Some(child_indent) = child_indent else {
                return Err("the target has no members section to place this next to; add a member in Composer first".to_string());
            };
            let anchor = children
                .iter()
                .find(|&&child| {
                    SECTION_ORDER
                        .iter()
                        .position(|name| name.as_bytes() == tokens[child].name.of(src))
                        .is_some_and(|position| position > order)
                })
                .map(|&child| {
                    line_bounds(
                        src,
                        Span::new(tokens[child].span.start, tokens[child].span.start),
                    )
                    .start
                    .min(tokens[child].span.start)
                })
                .unwrap_or(tokens[host_end].span.start);
            let at = {
                let mut line = anchor;
                while line > 0 && src[line - 1] != b'\n' {
                    line -= 1;
                }
                if starts_line(src, anchor) {
                    line
                } else {
                    anchor
                }
            };
            let moved = reindent(
                block_bytes,
                block.span.start,
                &block.cdata,
                block.indent,
                child_indent + unit,
            );
            let pad: Vec<u8> = std::iter::repeat_n(b' ', child_indent).collect();
            let mut text = Vec::new();
            text.extend_from_slice(&pad);
            text.extend_from_slice(format!("<{section}>").as_bytes());
            text.extend_from_slice(newline.as_bytes());
            text.extend_from_slice(&moved);
            text.extend_from_slice(&pad);
            text.extend_from_slice(format!("</{section}>").as_bytes());
            text.extend_from_slice(newline.as_bytes());
            Ok(Edit::new(Span::new(at, at), text))
        }
    }
}

/// The member's name attribute rewritten inside a block's bytes (the first tag only).
fn with_name(
    tokens: &[Token],
    src: &[u8],
    element: usize,
    block: &Block,
    block_bytes: &[u8],
    new_name: &str,
) -> Result<Vec<u8>, String> {
    let value = scan::attribute(src, &tokens[element], "name")
        .map_err(|error| error.to_string())?
        .ok_or("the element has no name")?;
    let relative = Span::new(value.start - block.span.start, value.end - block.span.start);
    let mut out = block_bytes[..relative.start].to_vec();
    out.extend_from_slice(new_name.as_bytes());
    out.extend_from_slice(&block_bytes[relative.end..]);
    Ok(out)
}

fn is_ancestor(catalog: &catalog::Catalog, entity: &str, of: &str) -> bool {
    catalog
        .entities
        .iter()
        .find(|item| item.name == of)
        .is_some_and(|item| item.inherits.iter().any(|name| name == entity))
}

pub fn plan(solution: &Solution, request: &Request) -> Result<Plan, RelocateError> {
    let member = request.member;
    let noun = member.word();
    refs::validate_field_name(&request.name, noun).map_err(RelocateError::Invalid)?;
    let final_name = request
        .new_name
        .clone()
        .unwrap_or_else(|| request.name.clone());
    refs::validate_field_name(&final_name, noun).map_err(RelocateError::Invalid)?;
    if request.from == request.to && final_name == request.name {
        return Err(RelocateError::Invalid(
            "the source and the target are the same entity; give --as a new name to copy within it"
                .to_string(),
        ));
    }
    if request.leave_delegate && (request.copy || member != Member::Service) {
        return Err(RelocateError::Invalid(
            "--leave-delegate applies to moving a service".to_string(),
        ));
    }

    let mut discovery = workspace::discover(solution);
    if !discovery.unreadable.is_empty() {
        return Err(RelocateError::Unreadable {
            files: discovery.unreadable,
        });
    }
    discovery.entities.sort_by(|a, b| a.path.cmp(&b.path));
    discovery.entities.dedup_by(|a, b| a.path == b.path);
    let from = find_entity(&discovery.entities, &request.from)?.clone();
    let to = find_entity(&discovery.entities, &request.to)?.clone();
    if request.leave_delegate && to.info.collection != "Things" {
        return Err(RelocateError::Invalid(format!(
            "--leave-delegate calls the service on {} by name, which needs a Thing; {} is a {}",
            request.to, request.to, to.info.collection
        )));
    }

    let from_old = std::fs::read(&from.path).map_err(|error| xml(&from.path, error))?;
    let to_old = std::fs::read(&to.path).map_err(|error| xml(&to.path, error))?;
    let from_tokens = scan::tokenize(&from_old).map_err(|error| xml(&from.path, error))?;
    let to_tokens = scan::tokenize(&to_old).map_err(|error| xml(&to.path, error))?;
    let from_host = sidecar::member_host_of(&from_tokens, &from_old)
        .ok_or_else(|| xml(&from.path, "not an entity"))?;
    let to_host = sidecar::member_host_of(&to_tokens, &to_old)
        .ok_or_else(|| xml(&to.path, "not an entity"))?;

    let (def_section, def_block, impl_section) = sections(member);
    let defs =
        sidecar::named_children_of(&from_tokens, &from_old, from_host, def_section, def_block)
            .map_err(|error| xml(&from.path, error))?;
    let Some(&def_at) = defs.get(&request.name) else {
        return Err(RelocateError::NotDeclared {
            entity: request.from.clone(),
            member,
            name: request.name.clone(),
        });
    };
    let impls = match impl_section {
        Some((section, block)) => {
            sidecar::named_children_of(&from_tokens, &from_old, from_host, section, block)
                .map_err(|error| xml(&from.path, error))?
        }
        None => Default::default(),
    };

    // Names: the target, what it inherits and what inherits it must not already use the name.
    let catalog = catalog::build(solution, catalog::Query::default())
        .map_err(|error| RelocateError::Invalid(format!("cannot build the model: {error}")))?;
    if !catalog.skipped.is_empty() {
        return Err(RelocateError::Unreadable {
            files: catalog.skipped.clone(),
        });
    }
    let source_entry = catalog
        .entities
        .iter()
        .find(|item| item.name == request.from && item.collection == from.info.collection);
    if member == Member::Service {
        if let Some(service) = source_entry.and_then(|entry| {
            entry
                .services
                .iter()
                .find(|service| service.name == request.name)
        }) {
            if service.from != "own" && service.from != request.from {
                return Err(RelocateError::Inherited {
                    entity: request.from.clone(),
                    name: request.name.clone(),
                    declared_on: service.from.clone(),
                });
            }
        }
    }
    let target_entry = catalog
        .entities
        .iter()
        .find(|item| item.name == request.to && item.collection == to.info.collection);
    let mut related: BTreeSet<String> = BTreeSet::from([request.to.clone()]);
    if let Some(entry) = target_entry {
        related.extend(entry.inherits.iter().cloned());
        related.extend(entry.implemented_by.iter().cloned());
    }
    for item in &catalog.entities {
        if item.inherits.iter().any(|name| name == &request.to) {
            related.insert(item.name.clone());
        }
    }
    let mut conflicts = BTreeSet::new();
    for entity in discovery
        .entities
        .iter()
        .filter(|item| related.contains(&item.info.name) && is_relocatable(&item.info.collection))
    {
        // A moved member is leaving the source, so the source's own copy is not a clash.
        if entity.path == from.path && !request.copy && !request.leave_delegate {
            continue;
        }
        let bytes = if entity.path == to.path {
            to_old.clone()
        } else {
            std::fs::read(&entity.path).map_err(|error| xml(&entity.path, error))?
        };
        let tokens = scan::tokenize(&bytes).map_err(|error| xml(&entity.path, error))?;
        let Some(host) = sidecar::member_host_of(&tokens, &bytes) else {
            continue;
        };
        let taken = sidecar::named_children_of(&tokens, &bytes, host, def_section, def_block)
            .map_err(|error| xml(&entity.path, error))?;
        if taken.contains_key(&final_name) {
            let relation = if entity.info.name == request.to {
                "the target"
            } else {
                "related to the target"
            };
            conflicts.insert(format!(
                "{noun} {final_name} is declared on {} ({relation})",
                entity.info.name
            ));
        }
    }
    if !conflicts.is_empty() {
        return Err(RelocateError::Exists {
            conflicts: conflicts.into_iter().collect(),
        });
    }

    // The edits to the target.
    let def_block_info = block_of(
        &from_tokens,
        &from_old,
        find_section(&from_tokens, &from_old, from_host, def_section)?,
        def_at,
    )
    .map_err(|error| xml(&from.path, error))?;
    let mut def_bytes = def_block_info.span.of(&from_old).to_vec();
    if final_name != request.name {
        def_bytes = with_name(
            &from_tokens,
            &from_old,
            def_at,
            &def_block_info,
            &def_bytes,
            &final_name,
        )
        .map_err(|why| xml(&from.path, why))?;
    }
    let mut to_edits = vec![insert_edit(
        &to_tokens,
        &to_old,
        to_host,
        def_section,
        &def_block_info,
        &def_bytes,
    )
    .map_err(|why| xml(&to.path, why))?];
    let mut from_edits = Vec::new();
    if !request.copy {
        from_edits.push(Edit::new(def_block_info.span, Vec::new()));
    }
    let mut impl_block = None;
    if let Some((section, _)) = impl_section {
        if let Some(&impl_at) = impls.get(&request.name) {
            let info = block_of(
                &from_tokens,
                &from_old,
                find_section(&from_tokens, &from_old, from_host, section)?,
                impl_at,
            )
            .map_err(|error| xml(&from.path, error))?;
            let mut bytes = info.span.of(&from_old).to_vec();
            if final_name != request.name {
                bytes = with_name(&from_tokens, &from_old, impl_at, &info, &bytes, &final_name)
                    .map_err(|why| xml(&from.path, why))?;
            }
            to_edits.push(
                insert_edit(&to_tokens, &to_old, to_host, section, &info, &bytes)
                    .map_err(|why| xml(&to.path, why))?,
            );
            if !request.copy && !request.leave_delegate {
                from_edits.push(Edit::new(info.span, Vec::new()));
            }
            impl_block = Some(impl_at);
        }
    }
    if request.leave_delegate {
        // The definition stays, so the source's edit for it is undone; only the implementation changes.
        from_edits.retain(|edit| edit.span != def_block_info.span);
    }
    let to_new = splice::splice(&to_old, &to_edits).map_err(|error| xml(&to.path, error))?;
    let mut from_new = if from_edits.is_empty() {
        None
    } else {
        Some(splice::splice(&from_old, &from_edits).map_err(|error| xml(&from.path, error))?)
    };

    // A delegate: the source keeps the service, whose body now calls the moved one.
    let mut from_sidecar_after = None;
    if request.leave_delegate {
        let service = sidecar::extract_services(&from_old)
            .map_err(|error| xml(&from.path, error))?
            .into_iter()
            .find(|service| service.name == request.name);
        let Some(mut service) = service else {
            return Err(RelocateError::Refused(format!(
                "{} is not a script service, so a delegate cannot replace its body",
                request.name
            )));
        };
        service.script = delegate_script(&from_old, &from_tokens, def_at, &request.to, &final_name);
        let mut sidecars = std::collections::BTreeMap::new();
        sidecars.insert(service.name.clone(), service.clone());
        let (bytes, _) = sync::sync(
            &from_old,
            &sidecars,
            false,
            solution.format.indent_cdata_payload,
            false,
        )
        .map_err(|error| xml(&from.path, error))?;
        from_new = Some(bytes);
        from_sidecar_after = Some(service);
    }

    // Verify the result in memory before anything is written.
    if member == Member::Service {
        let moved = sidecar::extract_services(&to_new).map_err(|error| {
            RelocateError::Verification(format!("the target would not read back: {error}"))
        })?;
        let original =
            sidecar::extract_services(&from_old).map_err(|error| xml(&from.path, error))?;
        if let Some(source) = original.iter().find(|service| service.name == request.name) {
            let arrived = moved.iter().find(|service| service.name == final_name);
            if arrived.is_none_or(|service| service.script != source.script) {
                return Err(RelocateError::Verification(format!(
                    "the script of {} would not arrive unchanged on {}; nothing was written",
                    request.name, request.to
                )));
            }
        }
        if let Some(bytes) = &from_new {
            let left = sidecar::extract_services(bytes).map_err(|error| {
                RelocateError::Verification(format!("the source would not read back: {error}"))
            })?;
            if !request.leave_delegate && left.iter().any(|service| service.name == request.name) {
                return Err(RelocateError::Verification(format!(
                    "{} would still be on {}; nothing was written",
                    request.name, request.from
                )));
            }
        }
    } else if scan::tokenize(&to_new).is_err() {
        return Err(RelocateError::Verification(
            "the target would not read back; nothing was written".to_string(),
        ));
    }

    // Sidecars of the service.
    let mut to_sidecar = None;
    let mut to_sidecars = None;
    let mut from_sidecars = None;
    if member == Member::Service {
        let from_dir = workspace::services_dir(solution, &from);
        let to_dir = workspace::services_dir(solution, &to);
        let had_sidecar = from_dir.join(&request.name).is_dir();
        if had_sidecar || to_dir.is_dir() {
            to_sidecar = sidecar::extract_services(&to_new)
                .map_err(|error| xml(&to.path, error))?
                .into_iter()
                .find(|service| service.name == final_name);
            if to_sidecar.is_some() {
                if to_dir.join(&final_name).exists() {
                    return Err(RelocateError::Exists {
                        conflicts: vec![format!(
                            "sidecar directory {}",
                            to_dir.join(&final_name).display()
                        )],
                    });
                }
                to_sidecars = Some(to_dir);
            }
        }
        if had_sidecar && (!request.copy) {
            from_sidecars = Some(from_dir);
        }
    }
    let _ = impl_block;

    // What moving breaks.
    let mut notes = Vec::new();
    let mut callers = Callers::default();
    let stays_reachable =
        request.copy || is_ancestor(&catalog, &request.to, &request.from) || request.leave_delegate;
    if !stays_reachable {
        callers = find_callers(solution, request, &from, &mut notes);
    } else if !request.copy && is_ancestor(&catalog, &request.to, &request.from) {
        notes.push(format!(
            "{} inherits from {}, so calls on {} keep resolving.",
            request.from, request.to, request.from
        ));
    }
    if request.leave_delegate {
        notes.push(format!(
            "{} keeps {} as a delegate that calls Things[\"{}\"].{}; the moved service sits on {}.",
            request.from, request.name, request.to, final_name, request.to
        ));
    }
    if member == Member::Property {
        notes.push("Property values stored on Things are not moved; instances of the source that no longer have the property keep a value nothing reads.".to_string());
    }
    if final_name != request.name {
        notes.push(format!("{} was renamed to {final_name} on the way; references inside the moved script to its own old name are not changed.", request.name));
    }

    Ok(Plan {
        root: solution.root.clone(),
        request: request.clone(),
        final_name,
        from_file: from.path.clone(),
        to_file: to.path.clone(),
        from_old,
        to_old,
        from_new,
        to_new,
        to_sidecars,
        to_sidecar,
        from_sidecars,
        from_sidecar_after,
        callers,
        notes,
    })
}

fn find_section(
    tokens: &[Token],
    src: &[u8],
    host: usize,
    section: &str,
) -> Result<usize, RelocateError> {
    scan::child_tags(tokens, src, section, host)
        .first()
        .copied()
        .ok_or_else(|| RelocateError::Invalid(format!("the source has no {section}")))
}

/// `Things["T"].Name({ a: a, b: b })`, as a script, from the service's parameters.
fn delegate_script(
    src: &[u8],
    tokens: &[Token],
    definition: usize,
    target: &str,
    name: &str,
) -> String {
    let mut parameters = Vec::new();
    for fields in scan::child_tags(tokens, src, "ParameterDefinitions", definition) {
        for field in scan::child_tags(tokens, src, "FieldDefinition", fields) {
            if let Ok(Some(value)) = scan::attribute(src, &tokens[field], "name") {
                parameters.push(scan::decode_entities(&String::from_utf8_lossy(
                    value.of(src),
                )));
            }
        }
    }
    let arguments = if parameters.is_empty() {
        String::new()
    } else {
        format!(
            "{{ {} }}",
            parameters
                .iter()
                .map(|parameter| format!("{parameter}: {parameter}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let returns_value = scan::child_tags(tokens, src, "ResultType", definition)
        .first()
        .and_then(|&result| {
            scan::attribute(src, &tokens[result], "baseType")
                .ok()
                .flatten()
        })
        .is_some_and(|base| base.of(src) != b"NOTHING");
    let call = format!("Things[\"{target}\"].{name}({arguments})");
    if returns_value {
        format!("var result = {call};")
    } else {
        format!("{call};")
    }
}

/// The references that stop resolving, from the findings a rename of the member would make.
fn find_callers(
    solution: &Solution,
    request: &Request,
    from: &EntityFile,
    notes: &mut Vec<String>,
) -> Callers {
    let kind = match request.member {
        Member::Service => RenameKind::Service,
        Member::Property => RenameKind::Property,
    };
    let probe = Spec {
        kind,
        old: request.name.clone(),
        new: "TwacoProbeName".to_string(),
        scope: Some(request.from.clone()),
        service: None,
    };
    let planned = match rename::plan(solution, &probe) {
        Ok(planned) => planned,
        Err(error) => {
            notes.push(format!(
                "Callers could not be listed ({error}); search for {} before relying on {}.",
                request.name, request.from
            ));
            return Callers::default();
        }
    };
    let own_sidecars = workspace::services_dir(solution, from).join(&request.name);
    let mut files = BTreeSet::new();
    let mut references = 0;
    let mut first = Vec::new();
    for change in planned.changes.iter().chain(&planned.outside) {
        if change.path == from.path || change.path.starts_with(&own_sidecars) {
            continue;
        }
        for finding in change
            .findings
            .iter()
            .filter(|finding| matches!(finding.tier, refs::Tier::Exact | refs::Tier::Embedded))
        {
            references += 1;
            files.insert(change.path.clone());
            if first.len() < 10 {
                first.push(format!(
                    "{}:{}  {}",
                    change
                        .path
                        .strip_prefix(&solution.root)
                        .unwrap_or(&change.path)
                        .display()
                        .to_string()
                        .replace('\\', "/"),
                    finding.line,
                    finding.excerpt
                ));
            }
        }
    }
    if references > 0 {
        notes.push(format!("{} reference(s) in {} file(s) stop resolving once {} leaves {}; update them, or use --leave-delegate for a service.", references, files.len(), request.name, request.from));
    }
    Callers {
        files: files.len(),
        references,
        first,
    }
}

impl Plan {
    /// Where files would change, relative to the solution.
    pub fn files(&self, solution: &Solution) -> Vec<String> {
        let relative = |path: &Path| {
            path.strip_prefix(&solution.root)
                .unwrap_or(path)
                .display()
                .to_string()
                .replace('\\', "/")
        };
        let mut files = vec![relative(&self.to_file)];
        if self.from_new.is_some() {
            files.push(relative(&self.from_file));
        }
        if let (Some(dir), Some(sidecar)) = (&self.to_sidecars, &self.to_sidecar) {
            files.push(relative(&dir.join(&sidecar.name)));
        }
        if let Some(dir) = &self.from_sidecars {
            files.push(relative(&dir.join(&self.request.name)));
        }
        files
    }
}

/// The entities the plan touched whose sidecars no longer match their XML (empty when in step).
pub fn verify(solution: &Solution, plan: &Plan) -> Vec<String> {
    let discovered = workspace::discover(solution);
    discovered
        .entities
        .iter()
        .filter(|entity| entity.path == plan.from_file || entity.path == plan.to_file)
        .filter(|entity| {
            let outcome = super::workflow::sync(
                solution,
                std::slice::from_ref(entity),
                &[],
                super::workflow::SyncOptions {
                    check: true,
                    ..Default::default()
                },
            );
            outcome.changed > 0 || outcome.failed > 0
        })
        .map(|entity| format!("{}/{}", entity.info.collection, entity.info.name))
        .collect()
}

#[derive(Debug, Clone)]
pub struct Applied {
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
}

/// Write the plan as one journaled operation. Every file is checked against the plan first (a file
/// changed since is refused and nothing is written); a failure or a crash part-way leaves every
/// file as it was or finished, and the next command to take the workspace lock completes or undoes
/// an interrupted run.
pub fn apply(plan: &Plan, lock: &WorkspaceLock) -> Result<Applied, RelocateError> {
    let mut transaction = Transaction::new(&plan.root, "move or copy a member");
    transaction
        .replace_file(&plan.to_file, &plan.to_old, plan.to_new.clone())
        .map_err(RelocateError::from)?;
    if let Some(bytes) = &plan.from_new {
        transaction
            .replace_file(&plan.from_file, &plan.from_old, bytes.clone())
            .map_err(RelocateError::from)?;
    }
    let mut written = vec![plan.to_file.clone()];
    if plan.from_new.is_some() {
        written.push(plan.from_file.clone());
    }
    let mut removed = Vec::new();
    let mut emptied = None;
    if let (Some(dir), Some(sidecar)) = (&plan.to_sidecars, &plan.to_sidecar) {
        let folder = dir.join(&sidecar.name);
        put_sidecar(&mut transaction, &folder, sidecar)?;
        written.push(folder);
    }
    if let Some(dir) = &plan.from_sidecars {
        let folder = dir.join(&plan.request.name);
        if let Some(after) = &plan.from_sidecar_after {
            put_sidecar(&mut transaction, &folder, after)?;
            written.push(folder);
        } else {
            for entry in std::fs::read_dir(&folder)
                .map_err(|error| xml(&folder, error))?
                .flatten()
            {
                if entry.path().is_file() {
                    let bytes =
                        std::fs::read(entry.path()).map_err(|error| xml(&entry.path(), error))?;
                    transaction
                        .delete_file(&entry.path(), &bytes)
                        .map_err(RelocateError::from)?;
                }
            }
            removed.push(folder.clone());
            emptied = Some(folder);
        }
    }
    transaction.apply(lock).map_err(RelocateError::from)?;
    if let Some(folder) = emptied {
        // The service folder may have been the last thing under its entity's folder. Empty
        // folders only: whatever else a person put there stays.
        for parent in folder.ancestors().take(3) {
            if std::fs::remove_dir(parent).is_err() {
                break;
            }
        }
    }
    Ok(Applied { written, removed })
}

/// A service's sidecar files as the transaction's steps: created when absent, replaced when they
/// differ. Always LF, as `workspace::write_sidecars` writes them.
fn put_sidecar(
    transaction: &mut Transaction<'_>,
    folder: &Path,
    sidecar: &ServiceSidecar,
) -> Result<(), RelocateError> {
    for (file, text) in [
        ("definition.xml", &sidecar.definition),
        ("script.js", &sidecar.script),
    ] {
        let path = folder.join(file);
        let after = text.replace("\r\n", "\n").into_bytes();
        match std::fs::read(&path) {
            Ok(current) => transaction.replace_file(&path, &current, after),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                transaction.create_file(&path, after)
            }
            Err(error) => return Err(xml(&path, error)),
        }
        .map_err(RelocateError::from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
