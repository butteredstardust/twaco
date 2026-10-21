//! Explicit server ownership: one selector policy for comparison and byte-preserving transfers.
//!
//! Transfers operate on scanner spans. Exported values stay in memory; only the existing backup
//! mechanism may persist them. The same transfer in reverse protects repository values in adopt.

use super::normalise::{self, Element, Node, NormaliseError};
use super::scan::{self, Kind, Span};
use super::splice::{self, Edit};
use serde::Deserialize;
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

const KINDS: &str = "members, run-time-permissions, visibility-permissions, property-values, property-values:<a,b>, configuration-table:<Table>, configuration-table:<Table>.<column>";

#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "String")]
pub(crate) enum Part {
    Members,
    RunTimePermissions,
    VisibilityPermissions,
    Properties(BTreeSet<String>),
    Table(String, Option<String>),
}

impl TryFrom<String> for Part {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let identifier = |s: &str| {
            !s.is_empty()
                && !s
                    .chars()
                    .any(|c| c.is_whitespace() || ":,./*\\".contains(c))
        };
        let part = match value.as_str() {
            "members" => Some(Self::Members),
            "run-time-permissions" => Some(Self::RunTimePermissions),
            "visibility-permissions" => Some(Self::VisibilityPermissions),
            "property-values" => Some(Self::Properties(BTreeSet::new())),
            _ => {
                if let Some(names) = value.strip_prefix("property-values:") {
                    names
                        .split(',')
                        .all(identifier)
                        .then(|| Self::Properties(names.split(',').map(str::to_string).collect()))
                } else if let Some(table) = value.strip_prefix("configuration-table:") {
                    let (table, column) = table
                        .split_once('.')
                        .map_or((table, None), |(t, c)| (t, Some(c)));
                    (identifier(table) && column.is_none_or(identifier))
                        .then(|| Self::Table(table.into(), column.map(str::to_string)))
                } else {
                    None
                }
            }
        };
        part.ok_or_else(|| format!("invalid server_owned part {value:?}; kinds: {KINDS}"))
    }
}

impl Part {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Members => "members".into(),
            Self::RunTimePermissions => "run-time-permissions".into(),
            Self::VisibilityPermissions => "visibility-permissions".into(),
            Self::Properties(names) if names.is_empty() => "property-values".into(),
            Self::Properties(names) => format!(
                "property-values:{}",
                names.iter().cloned().collect::<Vec<_>>().join(",")
            ),
            Self::Table(table, column) => format!(
                "configuration-table:{table}{}",
                column.as_ref().map_or(String::new(), |c| format!(".{c}"))
            ),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "RuleInput")]
pub(crate) struct Rule {
    pub(crate) entities: Vec<String>,
    pub(crate) parts: Vec<Part>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleInput {
    entities: Vec<String>,
    parts: Vec<Part>,
}

impl TryFrom<RuleInput> for Rule {
    type Error = String;

    fn try_from(input: RuleInput) -> Result<Self, Self::Error> {
        if input.entities.is_empty() || input.parts.is_empty() {
            return Err(format!(
                "server_owned entities and parts must be nonempty; kinds: {KINDS}"
            ));
        }
        for pattern in &input.entities {
            let valid = pattern.split_once('/').is_some_and(|(collection, name)| {
                super::bundle::COLLECTION_ORDER.contains(&collection)
                    && !name.is_empty()
                    && !name
                        .chars()
                        .any(|c| c.is_control() || c.is_whitespace() || "/\\?[]".contains(c))
                    && name != "."
                    && name != ".."
            });
            if !valid {
                return Err(format!("invalid server_owned entity pattern {pattern:?}; use Collection/Name with * within the name; kinds: {KINDS}"));
            }
        }
        Ok(Self {
            entities: input.entities,
            parts: input.parts,
        })
    }
}

impl Rule {
    pub(crate) fn matches(&self, collection: &str, name: &str) -> bool {
        self.entities
            .iter()
            .any(|p| super::adopt::glob_matches(p, &format!("{collection}/{name}")))
    }
}

pub(crate) fn parts(rules: &[Rule], collection: &str, name: &str) -> Vec<Part> {
    let mut selected: Vec<Part> = rules
        .iter()
        .filter(|r| r.matches(collection, name))
        .flat_map(|r| r.parts.clone())
        .collect();
    // Equivalent unions have the same fingerprint, independent of rule order and duplicates.
    let all_properties = selected
        .iter()
        .any(|p| matches!(p, Part::Properties(n) if n.is_empty()));
    let names: BTreeSet<String> = selected
        .iter()
        .filter_map(|p| match p {
            Part::Properties(n) => Some(n),
            _ => None,
        })
        .flatten()
        .cloned()
        .collect();
    let has_properties = selected.iter().any(|p| matches!(p, Part::Properties(_)));
    let tables: BTreeSet<String> = selected
        .iter()
        .filter_map(|p| match p {
            Part::Table(t, None) => Some(t.clone()),
            _ => None,
        })
        .collect();
    selected.retain(|p| {
        !matches!(p, Part::Properties(_))
            && !matches!(p, Part::Table(t, Some(_)) if tables.contains(t))
    });
    if has_properties {
        selected.push(Part::Properties(if all_properties {
            BTreeSet::new()
        } else {
            names
        }));
    }
    selected.sort_by_key(Part::label);
    selected.dedup_by_key(|p| p.label());
    selected
}

pub(crate) fn labels(parts: &[Part]) -> Vec<String> {
    parts.iter().map(Part::label).collect()
}

fn attr<'a>(element: &'a Element, name: &[u8]) -> Option<&'a [u8]> {
    element
        .attributes
        .iter()
        .find(|a| a.0 == name)
        .map(|a| a.1.as_slice())
}

fn children(element: &Element) -> impl Iterator<Item = &Element> {
    element.children.iter().filter_map(|n| {
        if let Node::Element(e) = n {
            Some(e)
        } else {
            None
        }
    })
}

/// Whether this exact node is owned. Ancestors are never selected by their tag alone.
fn owned(path: &[&Element], parts: &[Part]) -> bool {
    let node = path.last().unwrap();
    let tags: Vec<&[u8]> = path.iter().skip(1).map(|e| e.name.as_slice()).collect();
    parts.iter().any(|part| match part {
        Part::Members => {
            path[0].name == b"Group" && tags == [b"Members".as_slice()]
                || path[0].name == b"Organization"
                    && tags.len() >= 3
                    && tags[0] == b"OrganizationalUnits"
                    && tags.last() == Some(&b"Members".as_slice())
                    && tags[1..tags.len() - 1]
                        .iter()
                        .all(|tag| *tag == b"OrganizationalUnit")
        }
        Part::RunTimePermissions => tags == [b"RunTimePermissions".as_slice()],
        Part::VisibilityPermissions => tags == [b"VisibilityPermissions".as_slice()],
        Part::Properties(names) => {
            tags == [b"ThingProperties".as_slice()] && names.is_empty()
                || tags.len() == 2
                    && tags[0] == b"ThingProperties"
                    && names.contains(&String::from_utf8_lossy(&node.name).into_owned())
        }
        Part::Table(table, column) => {
            tags.len() >= 2
                && tags[0] == b"ConfigurationTables"
                && tags[1] == b"ConfigurationTable"
                && attr(path[2], b"name") == Some(table.as_bytes())
                && match column {
                    None => tags.len() == 2,
                    Some(c) => {
                        tags.len() == 5
                            && tags[2] == b"Rows"
                            && tags[3] == b"Row"
                            && tags[4] == c.as_bytes()
                    }
                }
        }
    })
}

/// Remove owned nodes on either raw adopt trees or the normaliser's comparison trees.
pub(crate) fn omit(element: &mut Element, parts: &[Part]) {
    fn visit(element: &mut Element, parents: &[&Element], parts: &[Part]) {
        let header = Element {
            source_start: element.source_start,
            unordered: element.unordered,
            name: element.name.clone(),
            attributes: element.attributes.clone(),
            children: Vec::new(),
        };
        let mut path = parents.to_vec();
        path.push(&header);
        element.children.retain_mut(|node| {
            let Node::Element(child) = node else {
                return true;
            };
            let mut child_path = path.clone();
            child_path.push(child);
            if owned(&child_path, parts) {
                return false;
            }
            visit(child, &path, parts);
            // Attribute-free empty scaffolding adds no source content; container attributes
            // and payloads remain compared even when all of its entries are server-owned.
            !(path.len() == 1
                && child.attributes.is_empty()
                && children(child).next().is_none()
                && child
                    .children
                    .iter()
                    .all(|n| matches!(n, Node::Text(t) if t.iter().all(u8::is_ascii_whitespace)))
                && (child.name == b"ConfigurationTables"
                    && parts.iter().any(|p| matches!(p, Part::Table(_, _)))
                    || child.name == b"ThingProperties"
                        && parts.iter().any(|p| matches!(p, Part::Properties(_)))))
        });
    }
    if !parts.is_empty() {
        visit(element, &[], parts);
    }
}

/// Copy selected donor subtrees into the recipient without serialising either entity.
/// Missing donor nodes remove recipient nodes; missing recipient ancestors are inserted with
/// only selected descendants. Unkeyed repeated rows correspond by their position.
pub(crate) fn preserve(
    recipient: &[u8],
    donor: &[u8],
    parts: &[Part],
) -> Result<Vec<u8>, NormaliseError> {
    if parts.is_empty() {
        return Ok(recipient.to_vec());
    }
    let ours = normalise::entity_of(recipient)?;
    let theirs = normalise::entity_of(donor)?;
    if ours.name != theirs.name || attr(&ours, b"name") != attr(&theirs, b"name") {
        return Err(NormaliseError::Malformed(
            "server-owned transfer requires the same entity".into(),
        ));
    }
    struct Source<'a> {
        bytes: &'a [u8],
        tokens: Vec<scan::Token>,
    }
    impl Source<'_> {
        fn bounds(&self, e: &Element) -> (Span, usize, usize, bool) {
            let at = self
                .tokens
                .iter()
                .position(|t| t.span.start == e.source_start)
                .unwrap();
            let end = scan::element_end_in(&self.tokens, self.bytes, at).unwrap();
            (
                Span::new(e.source_start, self.tokens[end].span.end),
                self.tokens[at].span.end,
                self.tokens[end].span.start,
                self.tokens[at].kind == Kind::Empty,
            )
        }
    }
    fn key(e: &Element, parents: &[&Element], parts: &[Part]) -> Option<Vec<u8>> {
        attr(e, b"name").map(<[u8]>::to_vec).or_else(|| {
            if e.name != b"Row" {
                return None;
            }
            let mut path = parents.to_vec();
            path.push(e);
            let mut row = e.clone();
            row.children.retain(|n| {
                let Node::Element(child) = n else {
                    return true;
                };
                let mut child_path = path.clone();
                child_path.push(child);
                !owned(&child_path, parts)
            });
            super::adopt::row_key(&row).map(String::into_bytes)
        })
    }
    fn transfer(
        a: &Source<'_>,
        left: Option<&Element>,
        b: &Source<'_>,
        right: Option<&Element>,
        parents: &[&Element],
        parts: &[Part],
    ) -> Result<Vec<u8>, NormaliseError> {
        let element = left.or(right).unwrap();
        let mut path = parents.to_vec();
        path.push(element);
        if owned(&path, parts) {
            return Ok(right.map_or(Vec::new(), |r| b.bounds(r).0.of(b.bytes).to_vec()));
        }
        let mut edits = Vec::new();
        let lc: Vec<_> = left.map_or(Vec::new(), |l| children(l).collect());
        let rc: Vec<_> = right.map_or(Vec::new(), |r| children(r).collect());
        let mut used = BTreeSet::new();
        let mut additions = Vec::new();
        for child in &lc {
            let partner = rc.iter().enumerate().find(|(i, c)| {
                !used.contains(i)
                    && c.name == child.name
                    && key(c, &path, parts) == key(child, &path, parts)
            });
            if let Some((i, _)) = partner {
                used.insert(i);
            }
            let replacement = transfer(a, Some(child), b, partner.map(|(_, c)| *c), &path, parts)?;
            let span = a.bounds(child).0;
            if replacement != span.of(a.bytes) {
                edits.push(Edit::new(span, replacement));
            }
        }
        for (_, child) in rc.iter().enumerate().filter(|(i, _)| !used.contains(i)) {
            additions.extend(transfer(a, None, b, Some(child), &path, parts)?);
        }
        if let Some(left) = left {
            let (span, open_end, close_start, empty) = a.bounds(left);
            if !additions.is_empty() {
                if empty {
                    let mut expanded = a.bytes[span.start..open_end - 2].to_vec();
                    expanded.push(b'>');
                    expanded.extend(additions);
                    expanded
                        .extend(format!("</{}>", String::from_utf8_lossy(&left.name)).as_bytes());
                    return Ok(expanded);
                }
                edits.push(Edit::new(Span::new(close_start, close_start), additions));
            }
            let local: Vec<_> = edits
                .into_iter()
                .map(|e| {
                    Edit::new(
                        Span::new(e.span.start - span.start, e.span.end - span.start),
                        e.replacement,
                    )
                })
                .collect();
            splice::splice(span.of(a.bytes), &local)
                .map_err(|e| NormaliseError::Malformed(e.to_string()))
        } else if additions.is_empty() {
            Ok(Vec::new())
        } else {
            let right = right.unwrap();
            let (span, open_end, close_start, _) = b.bounds(right);
            Ok([
                &b.bytes[span.start..open_end],
                &additions,
                &b.bytes[close_start..span.end],
            ]
            .concat())
        }
    }
    let a = Source {
        bytes: recipient,
        tokens: scan::tokenize(recipient)?,
    };
    let b = Source {
        bytes: donor,
        tokens: scan::tokenize(donor)?,
    };
    let replacement = transfer(&a, Some(&ours), &b, Some(&theirs), &[], parts)?;
    let result = splice::splice(recipient, &[Edit::new(a.bounds(&ours).0, replacement)])
        .map_err(|e| NormaliseError::Malformed(e.to_string()))?;
    if normalise::hash_with_parts(recipient, parts)? != normalise::hash_with_parts(&result, parts)?
    {
        return Err(NormaliseError::Malformed(format!(
            "preserving {} would change unowned row or unit structure; keep matching identities in source or own the whole configuration table",
            labels(parts).join(", ")
        )));
    }
    Ok(result)
}

/// Replace just the matching entity in an import bundle, retaining its envelope and peers.
pub(crate) fn preserve_in_bundle(
    bundle: &[u8],
    donor: &[u8],
    parts: &[Part],
) -> Result<Vec<u8>, NormaliseError> {
    let entity = normalise::entity_of(donor)?;
    let tokens = scan::tokenize(bundle)?;
    let roots = normalise::parse_document(bundle)?;
    let root = roots
        .iter()
        .find_map(|n| {
            if let Node::Element(e) = n {
                Some(e)
            } else {
                None
            }
        })
        .ok_or_else(|| NormaliseError::Malformed("bundle has no Entities element".into()))?;
    let found: Vec<_> = children(root)
        .flat_map(children)
        .filter(|e| e.name == entity.name && attr(e, b"name") == attr(&entity, b"name"))
        .collect();
    if found.len() != 1 {
        return Err(NormaliseError::Malformed(
            "bundle must contain exactly one entity for server-owned transfer".into(),
        ));
    }
    let at = tokens
        .iter()
        .position(|t| t.span.start == found[0].source_start)
        .unwrap();
    let end = scan::element_end_in(&tokens, bundle, at).unwrap();
    let span = Span::new(tokens[at].span.start, tokens[end].span.end);
    let replacement = preserve(span.of(bundle), donor, parts)?;
    splice::splice(bundle, &[Edit::new(span, replacement)])
        .map_err(|e| NormaliseError::Malformed(e.to_string()))
}

/// The owned parts whose value is encrypted on the server, which an import cannot carry: the
/// Importer encrypts the Exporter's ciphertext a second time, and the Thing is left with a wrong
/// password (observed on 10.1, 2026-10-20). Such a part is still left out of every comparison,
/// but it is sent as the repository has it and has to be set again after the import.
///
/// A PASSWORD column of an owned table column, a whole owned table with a PASSWORD column, and
/// owned PASSWORD properties the entity itself defines are encrypted.
pub(crate) fn encrypted(donor: &[u8], parts: &[Part]) -> Result<Vec<Part>, NormaliseError> {
    let entity = normalise::entity_of(donor)?;
    let password = |field: &Element| attr(field, b"baseType") == Some(b"PASSWORD".as_slice());
    let table_fields = |table: &str| -> Vec<&Element> {
        children(&entity)
            .filter(|e| e.name == b"ConfigurationTables")
            .flat_map(children)
            .filter(|t| attr(t, b"name") == Some(table.as_bytes()))
            .flat_map(children)
            .filter(|e| e.name == b"DataShape")
            .flat_map(children)
            .filter(|e| e.name == b"FieldDefinitions")
            .flat_map(children)
            .collect()
    };
    let secret_properties: BTreeSet<String> = children(&entity)
        .filter(|e| e.name == b"ThingShape")
        .flat_map(children)
        .filter(|e| e.name == b"PropertyDefinitions")
        .flat_map(children)
        .filter(|p| password(p))
        .filter_map(|p| attr(p, b"name"))
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect();
    Ok(parts
        .iter()
        .filter_map(|part| match part {
            Part::Table(table, Some(column)) => table_fields(table)
                .iter()
                .any(|f| password(f) && attr(f, b"name") == Some(column.as_bytes()))
                .then(|| part.clone()),
            Part::Table(table, None) => table_fields(table)
                .iter()
                .any(|f| password(f))
                .then(|| part.clone()),
            Part::Properties(names) if names.is_empty() => {
                (!secret_properties.is_empty()).then(|| part.clone())
            }
            Part::Properties(names) => {
                let secret: BTreeSet<String> =
                    names.intersection(&secret_properties).cloned().collect();
                (!secret.is_empty()).then_some(Part::Properties(secret))
            }
            _ => None,
        })
        .collect())
}

/// `parts` without the encrypted ones, which are sent as the repository has them.
pub(crate) fn carried(parts: &[Part], encrypted: &[Part]) -> Vec<Part> {
    parts
        .iter()
        .filter_map(
            |part| match (part, encrypted.iter().find(|e| same_kind(e, part))) {
                (_, None) => Some(part.clone()),
                (Part::Properties(names), Some(Part::Properties(secret))) if !names.is_empty() => {
                    let rest: BTreeSet<String> = names.difference(secret).cloned().collect();
                    (!rest.is_empty()).then_some(Part::Properties(rest))
                }
                _ => None,
            },
        )
        .collect()
}

fn same_kind(a: &Part, b: &Part) -> bool {
    match (a, b) {
        (Part::Properties(_), Part::Properties(_)) => true,
        _ => a.label() == b.label(),
    }
}

/// Encrypted Exporter cells require the Importer's universal-password envelope.
pub(crate) fn preserve_for_import(
    recipient: &[u8],
    donor: &[u8],
    parts: &[Part],
) -> Result<Vec<u8>, NormaliseError> {
    let bytes = preserve(recipient, donor, parts)?;
    let tokens = scan::tokenize(&bytes)?;
    let root = tokens
        .iter()
        .find(|t| matches!(t.kind, Kind::Start | Kind::Empty))
        .unwrap();
    if root.name.of(&bytes) != b"Entities" {
        return Err(NormaliseError::Malformed(
            "server-owned import requires an Entities export envelope".into(),
        ));
    }
    let attr = scan::attributes(&bytes, root)?
        .into_iter()
        .find(|a| a.name.of(&bytes) == b"universal");
    let edit = if let Some(a) = attr {
        Edit::new(a.value, b"password".to_vec())
    } else {
        Edit::new(
            Span::new(root.span.end - 1, root.span.end - 1),
            b" universal=\"password\"".to_vec(),
        )
    };
    splice::splice(&bytes, &[edit]).map_err(|e| NormaliseError::Malformed(e.to_string()))
}
