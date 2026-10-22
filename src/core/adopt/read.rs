//! Reading the entities to compare: from an export file, and from the repository's files.

use super::apply::io_error;
use super::flatten::{attribute, child_elements, text_of};
use super::{AdoptError, EntityRef};
use crate::core::config::Solution;
use crate::core::normalise::{self, Element, Node};
use crate::core::{scan, splice, workspace};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
/// Every named entity in a flat export, keyed and ordered by `Collection/Name`.
pub(crate) fn export_entities(path: &Path) -> Result<BTreeMap<EntityRef, Element>, AdoptError> {
    let bad = |why: String| AdoptError::Export {
        path: path.to_path_buf(),
        why,
    };
    let bytes = std::fs::read(path).map_err(|e| bad(e.to_string()))?;
    export_entities_bytes(&bytes, path)
}

/// Parse the entities in an Exporter response without first putting that response on disk.
pub(crate) fn export_entities_bytes(
    bytes: &[u8],
    source: &Path,
) -> Result<BTreeMap<EntityRef, Element>, AdoptError> {
    let bad = |why: String| AdoptError::Export {
        path: source.to_path_buf(),
        why,
    };
    let roots = normalise::parse_document(bytes).map_err(|e| bad(e.to_string()))?;
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

pub(super) fn entity_from_bytes(bytes: &[u8], wanted: &EntityRef) -> Option<Element> {
    normalise::parse_document(bytes)
        .ok()?
        .into_iter()
        .find_map(|node| match node {
            Node::Element(root) if root.name == b"Entities" => entities_in(root).remove(wanted),
            _ => None,
        })
}
pub(super) fn entity_document(collection: &str, entity: &[u8]) -> Vec<u8> {
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

pub(super) fn raw_entity(path: &Path, wanted: &EntityRef) -> Result<Vec<u8>, AdoptError> {
    let bytes = std::fs::read(path).map_err(|e| io_error(path, e))?;
    raw_entity_bytes(&bytes, wanted)
}

/// The original bytes of one entity in an Exporter document.  Server-starting init needs this
/// rather than the comparison tree: the layout pass must start with the server's actual XML.
pub(crate) fn raw_entity_bytes(bytes: &[u8], wanted: &EntityRef) -> Result<Vec<u8>, AdoptError> {
    let tokens = scan::tokenize(bytes).map_err(|e| io_error(Path::new("export"), e))?;
    entity_span(&tokens, bytes, wanted)
        .map(|span| span.of(bytes).to_vec())
        .ok_or_else(|| {
            io_error(
                Path::new("export"),
                format!("no {} in export", wanted.path()),
            )
        })
}

pub(crate) fn replace_entity_bytes(
    src: &[u8],
    wanted: &EntityRef,
    entity: &[u8],
    indent_cdata_payload: bool,
) -> Result<Vec<u8>, AdoptError> {
    let tokens = scan::tokenize(src).map_err(|e| io_error(Path::new("entity"), e))?;
    let span = entity_span(&tokens, src, wanted).ok_or_else(|| {
        io_error(
            Path::new("entity"),
            format!("no {} in entity file", wanted.path()),
        )
    })?;
    let entity = super::super::layout::entity(entity, Some(span.of(src)), indent_cdata_payload)
        .map_err(|e| io_error(Path::new("entity"), e))?;
    splice::splice(src, &[splice::Edit::new(span, entity)])
        .map_err(|e| io_error(Path::new("entity"), e))
}

pub(super) fn entity_span(
    tokens: &[scan::Token],
    src: &[u8],
    wanted: &EntityRef,
) -> Option<scan::Span> {
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
