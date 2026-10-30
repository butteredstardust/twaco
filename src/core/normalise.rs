//! Structural entity normalisation for conflict detection.
//!
//! The canonical bytes are intentionally an internal binary framing rather than pretty XML:
//! they are hash input, never an export. Length-prefixing makes element, attribute, text and
//! CDATA boundaries unambiguous without choosing an escaping style.
//!
//! Normalisation version 10 has exactly these non-semantic rules:
//!
//! - apply XML 1.0 line-end and CDATA-attribute whitespace normalisation to literal XML input
//!   before unescaping character references or applying payload-specific rules;
//! - unwrap Composer's `<Entities><Collection>` document envelope, because the live entity GET
//!   returns the entity element directly;
//! - remove `effectiveShape` and `Owner` elements when they are direct children of the entity,
//!   inherited/fetch metadata which that exporter demonstrably strips from live GETs;
//! - remove the exact computed sections `EffectiveImplementedShapes`,
//!   `effectiveAlertConfiguration`, `effectiveLocalPropertyBindings`,
//!   `effectiveRemoteEventBindings`, `effectiveRemotePropertyBindings`, and
//!   `effectiveRemoteServiceBindings`, observed only as direct entity children;
//! - remove direct-child `ConfigurationChanges`, observed on live MediaEntities and
//!   containing audit action, user, and timestamp rather than entity source;
//! - remove `lastModifiedDate` attributes, fetch-time metadata stripped by the same exporter;
//! - discard indentation-only XML text between child elements and sort attributes, because the
//!   observed live response is flattened while the committed export is indented and XML
//!   attribute order is not semantic;
//! - sort element children by `name` (canonical bytes break ties) only in `FieldDefinitions`,
//!   `ParameterDefinitions`, `ServiceDefinitions`, `ServiceImplementations`, `ConfigurationTables`,
//!   `ConfigurationTableDefinitions`, and `PropertyDefinitions`; some live exports reorder these
//!   named sections (PropertyDefinitions observed 2026-10-29; version 8);
//! - sort the entries of a direct-child `ThingProperties` by element name, and drop each entry's
//!   `Timestamp` and `Quality`: a 10.1 export orders a Thing's stored values by where each property
//!   comes from and by hash, not as imported, and an import re-stamps a value whose definition
//!   changed with the import time; the value itself is what the repository says (observed
//!   2026-10-16; version 6);
//! - inside `DesignTimePermissions`, `RunTimePermissions` and `VisibilityPermissions`, and the
//!   `Instance...` forms of the three on a ThingShape or ThingTemplate, treat every list as the
//!   set it is: drop a permission kind or a `Permissions` resource that grants no principal, and
//!   sort the rest by canonical bytes. An import reorders principals and resources and fills in
//!   the kinds a resource left out (observed 2026-10-07; version 4, instance blocks version 5);
//! - compact the single JSON CDATA payload in `mashupContent`; mashup JSON differed only in layout,
//!   as did direct-child `content` in observed `StateDefinition` and `StyleTheme` entities;
//! - in `mashupContent`, also sort every JSON object's keys: a 10.1 server returns a mashup's
//!   `Data` map (its data sources, keyed by name) in another order than imported (observed
//!   2026-10-19; version 7). Array order stays semantic;
//! - in an `Organization`, treat the direct-child `Connections` and `OrganizationalUnits` lists as
//!   sets and sort them by canonical bytes: the server returns both in another order than imported
//!   (observed 2026-10-19; version 7);
//! - remove ASCII whitespace from a single CDATA payload in a `MediaEntity`'s direct-child
//!   `content`, matching the wrapped committed and single-line live base64 representations;
//! - for leaf elements, trim leading and trailing XML whitespace from their single CDATA payload,
//!   and treat whitespace-only text as empty; the full live comparison found this export
//!   indentation around leaf values throughout configuration rows, scripts, property values, and
//!   JSON content, while interior bytes (including service-script indentation) remained semantic.
//! - where an entity declares the local base type, canonicalise a PropertyDefinition or
//!   FieldDefinition's `aspect.defaultValue` (and numeric minimum/maximum), ConfigurationTable
//!   row cells using that table's DataShape, and ThingProperties values using a local property
//!   definition: NUMBER follows Java `Double.toString`, INTEGER and LONG are parsed as i64, and
//!   JSON and QUERY are parsed, key-sorted and compacted, also as a stored `<Value><json>`;
//!   LOCATION and VEC2-VEC4 components compare as numbers; a TIMESPAN or VARIANT default in the
//!   form a server exports compares as the form it imports (see `typed_default`). A table's
//!   cells take only its own DataShape's types, not a nested INFOTABLE's (observed 2026-10-30;
//!   version 10). Unparseable values and values with no local type remain byte-significant;
//! - adjacent CDATA sections separated only by whitespace are one payload; XML uses that spelling
//!   for a literal `]]>` in a script (version 9);
//! - a leaf holding only an empty or whitespace-only CDATA is the same as an empty element, as
//!   the server stores an empty service script as `<code/>` (observed 2026-10-29; version 8).
//!
//! No generated id, default attribute, or other exception is included without a live/committed
//! observation. Apart from the rules above, CDATA bytes are never decoded, re-indented, or joined.
//!
//! Without server ownership, hashes remain `v10:<sha256>` byte for byte. With ownership they are
//! `v10:so1:<policy-sha256>:<content-sha256>`: the policy digest covers the sorted, deduplicated
//! union of applicable part labels (newline-delimited). `so1` versions selector semantics.
//! Content uses the same comparison tree with owned nodes omitted. Baselines must compare the
//! entire prefix before the final colon; adding, removing or changing ownership invalidates
//! only that entity's ancestor, even when its content happens to hash identically.

use super::scan::{self, Kind, ScanError, Token};
use sha2::{Digest, Sha256};

/// The normalisation version every hash carries; a baseline from another one says nothing.
pub(crate) const HASH_VERSION: &str = "v10";
pub(crate) const LIVE_ONLY_ELEMENTS: [&[u8]; 9] = [
    b"effectiveShape",
    b"Owner",
    b"EffectiveImplementedShapes",
    b"effectiveAlertConfiguration",
    b"effectiveLocalPropertyBindings",
    b"effectiveRemoteEventBindings",
    b"effectiveRemotePropertyBindings",
    b"effectiveRemoteServiceBindings",
    b"ConfigurationChanges",
];
const LIVE_ONLY_ATTRIBUTE: &[u8] = b"lastModifiedDate";
// Live exports reorder these name-keyed containers while preserving their child sets.
const NAME_KEYED_CONTAINERS: [&[u8]; 9] = [
    b"FieldDefinitions",
    b"ParameterDefinitions",
    b"ServiceDefinitions",
    b"ServiceImplementations",
    b"ConfigurationTables",
    b"ConfigurationTableDefinitions",
    b"PropertyDefinitions",
    b"Subscriptions",
    b"EventDefinitions",
];
const JSON_CONTENT_ENTITIES: [&[u8]; 2] = [b"StateDefinition", b"StyleTheme"];
const PERMISSION_BLOCKS: [&[u8]; 6] = [
    b"DesignTimePermissions",
    b"RunTimePermissions",
    b"VisibilityPermissions",
    b"InstanceDesignTimePermissions",
    b"InstanceRunTimePermissions",
    b"InstanceVisibilityPermissions",
];

#[derive(Debug, thiserror::Error)]
pub enum NormaliseError {
    #[error("{0}")]
    Xml(ScanError),
    #[error("{0} encoding is not supported; entity XML must be UTF-8")]
    DeclaredEncoding(String),
    #[error("malformed entity XML: {0}")]
    Malformed(String),
    #[error("unsupported XML entity reference &{0};")]
    EntityReference(String),
}

impl From<ScanError> for NormaliseError {
    fn from(value: ScanError) -> Self {
        NormaliseError::Xml(value)
    }
}

/// A parsed element. Crate-visible so `adopt` compares the same tree the hash is built from,
/// rather than a second parser that could read a document differently.
#[derive(Clone, Debug)]
pub(crate) struct Element {
    /// Scanner offset retained through normalisation for byte-preserving adopt edits.
    pub(crate) source_start: usize,
    pub(crate) unordered: bool,
    pub(crate) name: Vec<u8>,
    /// Unescaped values, sorted by name.
    pub(crate) attributes: Vec<(Vec<u8>, Vec<u8>)>,
    pub(crate) children: Vec<Node>,
}

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Element(Element),
    Text(Vec<u8>),
    Cdata(Vec<u8>),
    Comment(Vec<u8>),
    Pi(Vec<u8>),
}

/// Canonical structural bytes for one live or committed entity export.
pub fn normalise(src: &[u8]) -> Result<Vec<u8>, NormaliseError> {
    canonical(src, true)
}

/// Whether two exports differ only in their permission blocks: equal with every permission block
/// (instance blocks included) left out, and not
/// equal with them. An import never removes a grant, so such an entity needs a permissions push
/// rather than another import.
pub(crate) fn differ_only_in_permissions(left: &[u8], right: &[u8]) -> bool {
    match (canonical(left, false), canonical(right, false)) {
        (Ok(left_rest), Ok(right_rest)) => {
            left_rest == right_rest && normalise(left).ok() != normalise(right).ok()
        }
        _ => false,
    }
}

/// The projects a Project entity's `dependsOn` names: `{"projects": "A:1.0.0,B:0.0.0"}`.
pub(crate) fn project_dependencies(project: &Element) -> Vec<String> {
    #[derive(serde::Deserialize, Default)]
    struct Depends {
        #[serde(default)]
        projects: String,
    }
    let Some(text) = project
        .attributes
        .iter()
        .find(|(name, _)| name == b"dependsOn")
        .map(|(_, value)| String::from_utf8_lossy(value).into_owned())
    else {
        return Vec::new();
    };
    let depends: Depends = serde_json::from_str(&text).unwrap_or_default();
    depends
        .projects
        .split(',')
        .filter_map(|item| item.trim().split(':').next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// The projects the server added to a Project's `dependsOn`, when that is the only difference
/// between what was sent and what was read back. An import of an entity that needs another
/// project adds that project to its own Project; the server keeps what the file declares
/// otherwise, so taking the server's copy once settles it.
pub(crate) fn added_project_dependencies(sent: &[u8], read_back: &[u8]) -> Vec<String> {
    let (Ok(mut sent), Ok(mut read_back)) = (
        comparison_tree(sent, true),
        comparison_tree(read_back, true),
    ) else {
        return Vec::new();
    };
    if sent.name != b"Project" || read_back.name != b"Project" {
        return Vec::new();
    }
    let declared = project_dependencies(&sent);
    let added: Vec<String> = project_dependencies(&read_back)
        .into_iter()
        .filter(|name| !declared.contains(name))
        .collect();
    if added.is_empty() {
        return Vec::new();
    }
    for element in [&mut sent, &mut read_back] {
        element.attributes.retain(|(name, _)| name != b"dependsOn");
    }
    let (mut left, mut right) = (Vec::new(), Vec::new());
    write_element(&sent, &mut left);
    write_element(&read_back, &mut right);
    if left == right {
        added
    } else {
        Vec::new()
    }
}

fn canonical(src: &[u8], with_permissions: bool) -> Result<Vec<u8>, NormaliseError> {
    let entity = comparison_tree(src, with_permissions)?;
    let mut out = b"twaco-entity-normalise-v9\0".to_vec();
    write_element(&entity, &mut out);
    Ok(out)
}

/// The hash's exact comparison tree, retaining source offsets and set-list membership.
pub(crate) fn comparison_tree(
    src: &[u8],
    with_permissions: bool,
) -> Result<Element, NormaliseError> {
    let tokens = scan::tokenize(src)?;
    reject_non_utf8_declaration(src, &tokens)?;
    let entity = unwrap_entity(parse(src, &tokens)?)?;
    Ok(comparison_element(entity, with_permissions))
}

/// Compare an already parsed entity using the hash and layout policy, without serialising it.
pub(crate) fn comparison_element(mut entity: Element, with_permissions: bool) -> Element {
    strip_live_only(&mut entity);
    normalise_typed_values(&mut entity);
    normalise_leaf_content(&mut entity);
    normalise_special_payloads(&mut entity);
    clean_indentation(&mut entity);
    sort_name_keyed_containers(&mut entity);
    sort_thing_properties(&mut entity);
    sort_organization_sets(&mut entity);
    if with_permissions {
        normalise_permissions(&mut entity);
    } else {
        entity.children.retain(|child| {
            !matches!(child, Node::Element(element)
                if PERMISSION_BLOCKS.contains(&element.name.as_slice()))
        });
    }
    entity
}

/// The document's top-level nodes, parsed but not normalised: text unescaped and line ends
/// folded (XML 1.0 2.11), CDATA kept as it was written, nothing stripped or reordered.
pub(crate) fn parse_document(src: &[u8]) -> Result<Vec<Node>, NormaliseError> {
    let tokens = scan::tokenize(src)?;
    reject_non_utf8_declaration(src, &tokens)?;
    parse(src, &tokens)
}

/// The one entity element of an export, unwrapped from Composer's envelope exactly as the hash
/// unwraps it; a document holding several entities is refused.
pub(crate) fn entity_of(src: &[u8]) -> Result<Element, NormaliseError> {
    unwrap_entity(parse_document(src)?)
}

/// Versioned SHA-256 over [`normalise`]'s canonical bytes.
pub(crate) fn hash(src: &[u8]) -> Result<String, NormaliseError> {
    let canonical = normalise(src)?;
    Ok(format!("{HASH_VERSION}:{}", hex(&sha256(&canonical))))
}

pub(crate) fn hash_with_parts(
    src: &[u8],
    parts: &[super::server_owned::Part],
) -> Result<String, NormaliseError> {
    if parts.is_empty() {
        return hash(src);
    }
    let tree = owned_comparison_tree(src, parts)?;
    let mut canonical = b"twaco-entity-normalise-v9\0".to_vec();
    write_element(&tree, &mut canonical);
    Ok(format!(
        "{}:{}",
        hash_scope(parts),
        hex(&sha256(&canonical))
    ))
}

/// The tree [`hash_with_parts`] digests: the comparison tree with server-owned parts left out.
fn owned_comparison_tree(
    src: &[u8],
    parts: &[super::server_owned::Part],
) -> Result<Element, NormaliseError> {
    let mut tree = comparison_tree(src, true)?;
    if parts.is_empty() {
        return Ok(tree);
    }
    super::server_owned::omit(&mut tree, parts);
    // Removing owned descendants can change a set member's canonical sort key.
    Ok(comparison_element(tree, true))
}

const MAX_DIFFERENCES: usize = 5;

/// Where a read-back differs from what was sent, as the hash sees it: up to five paths such as
/// `Project/@dependsOn: changed` or `Thing/ThingShape/.../PropertyDefinition[a]: dropped by the
/// server`. Paths carry element names and `name` attributes only, never a value or text, which
/// may be a password.
pub(crate) fn read_back_differences(
    sent: &[u8],
    read_back: &[u8],
    parts: &[super::server_owned::Part],
) -> Vec<String> {
    let (Ok(sent), Ok(read_back)) = (
        owned_comparison_tree(sent, parts),
        owned_comparison_tree(read_back, parts),
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    differences(&sent, &read_back, &element_label(&sent), &mut out);
    // The walk compares text and elements apart; only how they interleave is left.
    if out.is_empty() && canonical_element_bytes(&sent) != canonical_element_bytes(&read_back) {
        out.push(format!(
            "{}: its text and elements interleave differently",
            element_label(&sent)
        ));
    }
    out
}

fn differences(sent: &Element, back: &Element, path: &str, out: &mut Vec<String>) {
    let names: std::collections::BTreeSet<&[u8]> = sent
        .attributes
        .iter()
        .chain(&back.attributes)
        .map(|(name, _)| name.as_slice())
        .collect();
    for name in names {
        let value = |element: &Element| {
            element
                .attributes
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let what = match (value(sent), value(back)) {
            (Some(left), Some(right)) if left == right => continue,
            (Some(_), Some(_)) => "changed",
            (Some(_), None) => "dropped by the server",
            (None, Some(_)) => "added by the server",
            (None, None) => continue,
        };
        note(
            out,
            format!("{path}/@{}: {what}", String::from_utf8_lossy(name)),
        );
    }
    let content = |element: &Element| {
        let mut bytes = Vec::new();
        for child in &element.children {
            match child {
                Node::Element(_) => {}
                Node::Text(text) => field(b'T', text, &mut bytes),
                Node::Cdata(cdata) => field(b'C', cdata, &mut bytes),
                Node::Comment(comment) => field(b'M', comment, &mut bytes),
                Node::Pi(pi) => field(b'P', pi, &mut bytes),
            }
        }
        bytes
    };
    if content(sent) != content(back) {
        note(out, format!("{path}: text changed"));
    }
    let (left, right) = (labelled_children(sent), labelled_children(back));
    let unique = |children: &[(String, &Element)]| {
        let labels: std::collections::BTreeSet<&String> =
            children.iter().map(|(label, _)| label).collect();
        labels.len() == children.len()
    };
    if unique(&left) && unique(&right) {
        // Children told apart by name: match them by it, so one the server dropped or added is
        // named rather than shifting every sibling after it.
        let find = |children: &[(String, &'_ Element)], label: &str| {
            children.iter().any(|(other, _)| other == label)
        };
        for (label, _) in &left {
            if !find(&right, label) {
                note(out, format!("{path}/{label}: dropped by the server"));
            }
        }
        for (label, _) in &right {
            if !find(&left, label) {
                note(out, format!("{path}/{label}: added by the server"));
            }
        }
        // Outside the sets the hash sorts, order is content.
        if !sent.unordered {
            fn common<'a>(
                children: &'a [(String, &Element)],
                other: &[(String, &Element)],
            ) -> Vec<&'a str> {
                children
                    .iter()
                    .map(|(label, _)| label.as_str())
                    .filter(|label| other.iter().any(|(other, _)| other == label))
                    .collect()
            }
            if common(&left, &right) != common(&right, &left) {
                note(out, format!("{path}: its elements are in another order"));
            }
        }
        for (label, child) in &left {
            if let Some((_, other)) = right.iter().find(|(other, _)| other == label) {
                differences(child, other, &format!("{path}/{label}"), out);
            }
        }
        return;
    }
    for (index, ((label, child), (other_label, other))) in left.iter().zip(&right).enumerate() {
        if label == other_label {
            differences(child, other, &format!("{path}/{label}[{}]", index + 1), out);
        } else {
            note(
                out,
                format!(
                    "{path}: element {} is {label} as sent, {other_label} as read back",
                    index + 1
                ),
            );
            return;
        }
    }
    if left.len() != right.len() {
        note(
            out,
            format!(
                "{path}: {} child elements sent, {} read back",
                left.len(),
                right.len()
            ),
        );
    }
}

fn note(out: &mut Vec<String>, what: String) {
    if out.len() < MAX_DIFFERENCES {
        out.push(what);
    }
}

/// An element's name, with its `name` attribute when it has one: `PropertyDefinition[a]`.
fn element_label(element: &Element) -> String {
    let tag = String::from_utf8_lossy(&element.name);
    match name_attribute(element) {
        [] => tag.into_owned(),
        name => format!("{tag}[{}]", String::from_utf8_lossy(name)),
    }
}

fn labelled_children(element: &Element) -> Vec<(String, &Element)> {
    element
        .children
        .iter()
        .filter_map(|child| match child {
            Node::Element(child) => Some((element_label(child), child)),
            _ => None,
        })
        .collect()
}

/// Hash version and selector policy, excluding the content digest.
pub(crate) fn hash_scope(parts: &[super::server_owned::Part]) -> String {
    if parts.is_empty() {
        return HASH_VERSION.into();
    }
    let policy = super::server_owned::labels(parts).join("\n");
    format!("{HASH_VERSION}:so1:{}", hex(&sha256(policy.as_bytes())))
}

fn parse(src: &[u8], tokens: &[Token]) -> Result<Vec<Node>, NormaliseError> {
    let mut roots = Vec::new();
    let mut stack: Vec<Element> = Vec::new();
    for token in tokens {
        match token.kind {
            Kind::Start | Kind::Empty => {
                let mut attributes = Vec::new();
                for attribute in scan::attributes(src, token)? {
                    attributes.push((
                        attribute.name.of(src).to_vec(),
                        unescape(&normalise_attribute_value(attribute.value.of(src)))?,
                    ));
                }
                attributes.sort();
                if attributes.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                    return Err(NormaliseError::Malformed(format!(
                        "duplicate attribute on <{}>",
                        String::from_utf8_lossy(token.name.of(src))
                    )));
                }
                let element = Element {
                    source_start: token.span.start,
                    unordered: false,
                    name: token.name.of(src).to_vec(),
                    attributes,
                    children: Vec::new(),
                };
                if token.kind == Kind::Empty {
                    append(&mut roots, &mut stack, Node::Element(element));
                } else {
                    stack.push(element);
                }
            }
            Kind::End => {
                let element = stack.pop().ok_or_else(|| {
                    NormaliseError::Malformed(format!(
                        "closing </{}> has no open element",
                        String::from_utf8_lossy(token.name.of(src))
                    ))
                })?;
                if element.name != token.name.of(src) {
                    return Err(NormaliseError::Malformed(format!(
                        "closing </{}> does not match <{}>",
                        String::from_utf8_lossy(token.name.of(src)),
                        String::from_utf8_lossy(&element.name)
                    )));
                }
                append(&mut roots, &mut stack, Node::Element(element));
            }
            Kind::Text => {
                let bytes = token.span.of(src);
                // BOM and document-level formatting are not part of the entity tree.
                if !stack.is_empty() || !xml_whitespace(bytes) {
                    append(
                        &mut roots,
                        &mut stack,
                        Node::Text(unescape(&normalise_line_ends(bytes))?),
                    );
                }
            }
            Kind::Cdata => append(
                &mut roots,
                &mut stack,
                Node::Cdata(normalise_line_ends(token.inner.of(src))),
            ),
            Kind::Comment => append(
                &mut roots,
                &mut stack,
                Node::Comment(normalise_line_ends(token.span.of(src))),
            ),
            Kind::Pi => {
                // The XML declaration describes its document container, not the entity.
                let raw = token.span.of(src);
                if !is_xml_declaration(raw) {
                    append(&mut roots, &mut stack, Node::Pi(raw.to_vec()));
                }
            }
            Kind::DocType => {
                if !stack.is_empty() {
                    return Err(NormaliseError::Malformed(
                        "DOCTYPE inside an element".to_string(),
                    ));
                }
            }
        }
    }
    if let Some(open) = stack.last() {
        return Err(NormaliseError::Malformed(format!(
            "<{}> is not closed",
            String::from_utf8_lossy(&open.name)
        )));
    }
    Ok(roots)
}

fn append(roots: &mut Vec<Node>, stack: &mut [Element], node: Node) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else {
        roots.push(node);
    }
}

fn unwrap_entity(roots: Vec<Node>) -> Result<Element, NormaliseError> {
    let mut elements: Vec<Element> = roots
        .into_iter()
        .filter_map(|node| match node {
            Node::Element(element) => Some(element),
            _ => None,
        })
        .collect();
    if elements.len() != 1 {
        return Err(NormaliseError::Malformed(format!(
            "expected one document element, found {}",
            elements.len()
        )));
    }
    let root = elements.pop().expect("length checked");
    if root.name != b"Entities" {
        return Ok(root);
    }
    let mut collections = child_elements(root.children);
    if collections.len() != 1 {
        return Err(NormaliseError::Malformed(format!(
            "<Entities> contains {} collections, expected one",
            collections.len()
        )));
    }
    let collection = collections.pop().expect("length checked");
    let mut entities = child_elements(collection.children);
    if entities.len() != 1 {
        return Err(NormaliseError::Malformed(format!(
            "<{}> contains {} entities, expected one",
            String::from_utf8_lossy(&collection.name),
            entities.len()
        )));
    }
    Ok(entities.pop().expect("length checked"))
}

fn child_elements(children: Vec<Node>) -> Vec<Element> {
    children
        .into_iter()
        .filter_map(|node| match node {
            Node::Element(element) => Some(element),
            _ => None,
        })
        .collect()
}

fn strip_live_only(element: &mut Element) {
    // The evidence supports removing these elements only at the entity boundary.
    element.children.retain(|child| {
        !matches!(child, Node::Element(child) if LIVE_ONLY_ELEMENTS.contains(&child.name.as_slice()))
    });
    strip_live_only_attributes(element);
}

fn strip_live_only_attributes(element: &mut Element) {
    element
        .attributes
        .retain(|(name, _)| name != LIVE_ONLY_ATTRIBUTE);
    for child in &mut element.children {
        if let Node::Element(child) = child {
            strip_live_only_attributes(child);
        }
    }
}

fn normalise_special_payloads(entity: &mut Element) {
    normalise_mashup_payloads(entity);
    if JSON_CONTENT_ENTITIES.contains(&entity.name.as_slice()) {
        for child in &mut entity.children {
            if let Node::Element(child) = child {
                if child.name == b"content" {
                    normalise_json_payload(child);
                }
            }
        }
    }
    if entity.name == b"MediaEntity" {
        for child in &mut entity.children {
            let Node::Element(child) = child else {
                continue;
            };
            if child.name == b"content" {
                if let Some(payload) = single_cdata_payload(child) {
                    let compact = payload
                        .into_iter()
                        .filter(|byte| !byte.is_ascii_whitespace())
                        .collect();
                    child.children = vec![Node::Cdata(compact)];
                }
            }
        }
    }
}

/// Values only acquire a type from their own entity.  In particular, an inherited property is
/// deliberately not guessed: the local document does not prove which definition the server uses.
fn normalise_typed_values(entity: &mut Element) {
    normalise_definition_aspects(entity);
    normalise_configuration_tables(entity);

    let definitions = definitions_named(entity, b"PropertyDefinition");
    for child in &mut entity.children {
        let Node::Element(properties) = child else {
            continue;
        };
        if properties.name != b"ThingProperties" {
            continue;
        }
        for value in &mut properties.children {
            let Node::Element(value) = value else {
                continue;
            };
            let Some(base_type) = definitions.get(&value.name) else {
                continue;
            };
            for part in &mut value.children {
                if let Node::Element(part) = part {
                    if part.name == b"Value" {
                        normalise_typed_leaf(part, base_type);
                    }
                }
            }
        }
    }
}

fn normalise_definition_aspects(element: &mut Element) {
    if matches!(
        element.name.as_slice(),
        b"PropertyDefinition" | b"FieldDefinition"
    ) {
        let base_type = attribute_value(element, b"baseType").map(|value| value.to_vec());
        if let Some(base_type) = base_type {
            for (name, value) in &mut element.attributes {
                if name == b"aspect.defaultValue"
                    || matches!(base_type.as_slice(), b"NUMBER" | b"INTEGER" | b"LONG")
                        && (name == b"aspect.minimumValue" || name == b"aspect.maximumValue")
                {
                    if let Some(canonical) = canonical_typed_value(&base_type, value) {
                        *value = canonical;
                    }
                }
            }
        }
    }
    for child in &mut element.children {
        if let Node::Element(child) = child {
            normalise_definition_aspects(child);
        }
    }
}

fn normalise_configuration_tables(element: &mut Element) {
    if element.name == b"ConfigurationTable" {
        let fields = table_fields(element);
        for child in &mut element.children {
            let Node::Element(rows) = child else {
                continue;
            };
            if rows.name != b"Rows" {
                continue;
            }
            for row in &mut rows.children {
                let Node::Element(row) = row else {
                    continue;
                };
                if row.name != b"Row" {
                    continue;
                }
                for cell in &mut row.children {
                    let Node::Element(cell) = cell else {
                        continue;
                    };
                    if let Some(base_type) = fields.get(&cell.name) {
                        normalise_typed_leaf(cell, base_type);
                    }
                }
            }
        }
    }
    for child in &mut element.children {
        if let Node::Element(child) = child {
            normalise_configuration_tables(child);
        }
    }
}

/// The table's own `DataShape/FieldDefinitions`: an INFOTABLE cell's inner shape may reuse a
/// field name with another type.
fn table_fields(table: &Element) -> std::collections::BTreeMap<Vec<u8>, Vec<u8>> {
    fn children<'a>(element: &'a Element, wanted: &'a [u8]) -> impl Iterator<Item = &'a Element> {
        element
            .children
            .iter()
            .filter_map(move |child| match child {
                Node::Element(child) if child.name == wanted => Some(child),
                _ => None,
            })
    }
    let mut out = std::collections::BTreeMap::new();
    for shape in children(table, b"DataShape") {
        for list in children(shape, b"FieldDefinitions") {
            for field in children(list, b"FieldDefinition") {
                if let (Some(name), Some(base_type)) = (
                    attribute_value(field, b"name"),
                    attribute_value(field, b"baseType"),
                ) {
                    out.insert(name.to_vec(), base_type.to_vec());
                }
            }
        }
    }
    out
}

fn definitions_named(
    element: &Element,
    wanted: &[u8],
) -> std::collections::BTreeMap<Vec<u8>, Vec<u8>> {
    fn collect(
        element: &Element,
        wanted: &[u8],
        out: &mut std::collections::BTreeMap<Vec<u8>, Vec<u8>>,
    ) {
        if element.name == wanted {
            if let (Some(name), Some(base_type)) = (
                attribute_value(element, b"name"),
                attribute_value(element, b"baseType"),
            ) {
                out.insert(name.to_vec(), base_type.to_vec());
            }
        }
        for child in &element.children {
            if let Node::Element(child) = child {
                collect(child, wanted, out);
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    collect(element, wanted, &mut out);
    out
}

fn attribute_value<'a>(element: &'a Element, wanted: &[u8]) -> Option<&'a [u8]> {
    element
        .attributes
        .iter()
        .find(|(name, _)| name == wanted)
        .map(|(_, value)| value.as_slice())
}

fn canonical_typed_value(base_type: &[u8], value: &[u8]) -> Option<Vec<u8>> {
    let base_type = std::str::from_utf8(base_type).ok()?;
    let value = std::str::from_utf8(trim_xml_whitespace(value)).ok()?;
    match base_type {
        "NUMBER" | "INTEGER" | "LONG" => {
            crate::core::number::java_text(base_type, value).map(String::into_bytes)
        }
        "JSON" | "QUERY" => serde_json::from_str(value)
            .ok()
            .and_then(|value| serde_json::to_vec(&sorted_keys(value)).ok()),
        "LOCATION" => {
            let values: Option<Vec<String>> = value
                .split(',')
                .map(|component| crate::core::number::java_text("NUMBER", component.trim()))
                .collect();
            let values = values?;
            matches!(values.len(), 2 | 3).then(|| values.join(",").into_bytes())
        }
        // Exported in another form than imported; see `typed_default`.
        "TIMESPAN" | "VARIANT" => {
            crate::core::typed_default::importable(base_type, value).map(String::into_bytes)
        }
        // Components the server writes as doubles: `1,2` is stored `1.0,2.0` (10.2, 2026-10-30).
        "VEC2" | "VEC3" | "VEC4" => {
            let values: Option<Vec<String>> = value
                .split(',')
                .map(|component| crate::core::number::java_text("NUMBER", component.trim()))
                .collect();
            let values = values?;
            let wanted = usize::from(base_type.as_bytes()[3] - b'0');
            (values.len() == wanted).then(|| values.join(",").into_bytes())
        }
        _ => None,
    }
}

fn normalise_typed_leaf(element: &mut Element, base_type: &[u8]) {
    // A stored JSON or QUERY value is `<Value><json>…</json></Value>`; the server sorts its keys.
    if matches!(base_type, b"JSON" | b"QUERY") {
        if let [Node::Element(json)] = element
            .children
            .iter_mut()
            .filter(|child| !matches!(child, Node::Text(text) if xml_whitespace(text)))
            .collect::<Vec<_>>()
            .as_mut_slice()
        {
            if json.name == b"json" {
                if let Some(canonical) = single_text_or_cdata_payload(json)
                    .and_then(|payload| canonical_typed_value(base_type, &payload))
                {
                    json.children = vec![Node::Cdata(canonical)];
                }
                return;
            }
        }
    }
    let payload = single_text_or_cdata_payload(element);
    let Some(payload) = payload else {
        return;
    };
    let Some(canonical) = canonical_typed_value(base_type, &payload) else {
        return;
    };
    // Values are semantically text; picking one framing also makes an escaped value and CDATA
    // value compare equal when their locally-declared type proves they mean the same thing.
    element.children = vec![Node::Text(canonical)];
}

fn single_text_or_cdata_payload(element: &Element) -> Option<Vec<u8>> {
    let mut payload = Vec::new();
    let mut framing = None;
    for child in &element.children {
        match child {
            Node::Text(text) if xml_whitespace(text) => {}
            Node::Text(text) => {
                if framing.replace(0).is_some_and(|kind| kind != 0) {
                    return None;
                }
                payload.extend_from_slice(text);
            }
            Node::Cdata(text) => {
                if framing.replace(1).is_some_and(|kind| kind != 1) {
                    return None;
                }
                payload.extend_from_slice(text);
            }
            _ => return None,
        }
    }
    framing.map(|_| payload)
}

fn normalise_leaf_content(element: &mut Element) {
    for child in &mut element.children {
        if let Node::Element(child) = child {
            normalise_leaf_content(child);
        }
    }
    if element
        .children
        .iter()
        .any(|child| matches!(child, Node::Element(_)))
    {
        return;
    }
    if element
        .children
        .iter()
        .all(|child| matches!(child, Node::Text(text) if xml_whitespace(text)))
    {
        element.children.clear();
        return;
    }
    let Some(payload) = single_cdata_payload(element) else {
        return;
    };
    let payload = trim_xml_whitespace(&payload);
    if payload.is_empty() {
        element.children.clear();
    } else {
        element.children = vec![Node::Cdata(payload.to_vec())];
    }
}

fn trim_xml_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    {
        bytes = &bytes[1..];
    }
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn normalise_mashup_payloads(element: &mut Element) {
    for child in &mut element.children {
        if let Node::Element(child) = child {
            normalise_mashup_payloads(child);
        }
    }
    if element.name == b"mashupContent" {
        normalise_json_payload(element);
        sort_json_payload_keys(element);
    }
}

/// Object keys in key order, recursively; arrays keep their order.
fn sorted_keys(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted_keys(value)))
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(sorted_keys).collect())
        }
        other => other,
    }
}

fn sort_json_payload_keys(element: &mut Element) {
    let Some(payload) = single_cdata_payload(element) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) else {
        return;
    };
    let Ok(sorted) = serde_json::to_vec(&sorted_keys(value)) else {
        return;
    };
    element.children = vec![Node::Cdata(sorted)];
}

/// An Organization's tree of connections and its units are sets; see the module rules.
fn sort_organization_sets(entity: &mut Element) {
    if entity.name != b"Organization" {
        return;
    }
    for child in &mut entity.children {
        let Node::Element(list) = child else {
            continue;
        };
        if list.name != b"Connections" && list.name != b"OrganizationalUnits" {
            continue;
        }
        list.unordered = true;
        if list
            .children
            .iter()
            .all(|child| matches!(child, Node::Element(_)))
        {
            list.children.sort_by_cached_key(|child| match child {
                Node::Element(child) => canonical_element_bytes(child),
                _ => unreachable!("only elements are sorted"),
            });
        }
    }
}

fn normalise_json_payload(element: &mut Element) {
    let Some(payload) = single_cdata_payload(element) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) else {
        return;
    };
    let Ok(compact) = serde_json::to_vec(&value) else {
        return;
    };
    element.children = vec![Node::Cdata(compact)];
}

fn single_cdata_payload(element: &Element) -> Option<Vec<u8>> {
    let mut payload = Vec::new();
    let mut found = false;
    for child in &element.children {
        match child {
            Node::Text(text) if xml_whitespace(text) => {}
            Node::Cdata(cdata) => {
                found = true;
                payload.extend_from_slice(cdata);
            }
            _ => return None,
        }
    }
    found.then_some(payload)
}

fn clean_indentation(element: &mut Element) {
    let has_element = element
        .children
        .iter()
        .any(|child| matches!(child, Node::Element(_)));
    if has_element {
        element
            .children
            .retain(|child| !matches!(child, Node::Text(text) if xml_whitespace(text)));
    }
    for child in &mut element.children {
        if let Node::Element(child) = child {
            clean_indentation(child);
        }
    }
}

/// The entity's stored property values, in element-name order.
fn sort_thing_properties(entity: &mut Element) {
    for child in &mut entity.children {
        let Node::Element(block) = child else {
            continue;
        };
        if block.name != b"ThingProperties" {
            continue;
        }
        block.unordered = true;
        let slots: Vec<usize> = block
            .children
            .iter()
            .enumerate()
            .filter_map(|(index, node)| matches!(node, Node::Element(_)).then_some(index))
            .collect();
        let mut values: Vec<Element> = slots
            .iter()
            .map(|index| match &block.children[*index] {
                Node::Element(value) => {
                    let mut value = value.clone();
                    value.children.retain(|part| {
                        !matches!(part, Node::Element(part)
                            if part.name == b"Timestamp" || part.name == b"Quality")
                    });
                    value
                }
                _ => unreachable!("slots contain only elements"),
            })
            .collect();
        values.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| canonical_element_bytes(left).cmp(&canonical_element_bytes(right)))
        });
        for (index, value) in slots.into_iter().zip(values) {
            block.children[index] = Node::Element(value);
        }
    }
}

fn sort_name_keyed_containers(element: &mut Element) {
    for child in &mut element.children {
        if let Node::Element(child) = child {
            sort_name_keyed_containers(child);
        }
    }
    if !NAME_KEYED_CONTAINERS.contains(&element.name.as_slice()) {
        return;
    }
    element.unordered = true;

    let slots: Vec<usize> = element
        .children
        .iter()
        .enumerate()
        .filter_map(|(index, child)| matches!(child, Node::Element(_)).then_some(index))
        .collect();
    let mut children: Vec<Element> = slots
        .iter()
        .map(|index| match &element.children[*index] {
            Node::Element(child) => child.clone(),
            _ => unreachable!("slots contain only elements"),
        })
        .collect();
    children.sort_by(|left, right| {
        name_attribute(left)
            .cmp(name_attribute(right))
            .then_with(|| canonical_element_bytes(left).cmp(&canonical_element_bytes(right)))
    });
    for (index, child) in slots.into_iter().zip(children) {
        element.children[index] = Node::Element(child);
    }
}

fn normalise_permissions(element: &mut Element) {
    if PERMISSION_BLOCKS.contains(&element.name.as_slice()) {
        as_set(element);
        return;
    }
    for child in &mut element.children {
        if let Node::Element(child) = child {
            normalise_permissions(child);
        }
    }
}

/// A permission block's lists as sets: a kind or resource granting nobody is the same as one left
/// out, and order carries nothing.
fn as_set(element: &mut Element) {
    element.unordered = true;
    for child in &mut element.children {
        if let Node::Element(child) = child {
            as_set(child);
        }
    }
    element.children.retain(|child| match child {
        Node::Element(child) => child.name == b"Principal" || !child.children.is_empty(),
        _ => true,
    });
    if element
        .children
        .iter()
        .all(|child| matches!(child, Node::Element(_)))
    {
        element.children.sort_by_cached_key(|child| match child {
            Node::Element(child) => canonical_element_bytes(child),
            _ => unreachable!("only elements are sorted"),
        });
    }
}

fn name_attribute(element: &Element) -> &[u8] {
    element
        .attributes
        .iter()
        .find(|(name, _)| name == b"name")
        .map(|(_, value)| value.as_slice())
        .unwrap_or_default()
}

pub(crate) fn canonical_element_bytes(element: &Element) -> Vec<u8> {
    let mut out = Vec::new();
    write_element(element, &mut out);
    out
}

fn write_element(element: &Element, out: &mut Vec<u8>) {
    field(b'E', &element.name, out);
    number(element.attributes.len(), out);
    for (name, value) in &element.attributes {
        field(b'A', name, out);
        field(b'V', value, out);
    }
    number(element.children.len(), out);
    for child in &element.children {
        match child {
            Node::Element(child) => write_element(child, out),
            Node::Text(text) => field(b'T', text, out),
            Node::Cdata(cdata) => field(b'C', cdata, out),
            Node::Comment(comment) => field(b'M', comment, out),
            Node::Pi(pi) => field(b'P', pi, out),
        }
    }
    out.push(b'Z');
}

fn field(kind: u8, bytes: &[u8], out: &mut Vec<u8>) {
    out.push(kind);
    number(bytes.len(), out);
    out.extend_from_slice(bytes);
}

fn number(value: usize, out: &mut Vec<u8>) {
    out.extend_from_slice(&(value as u64).to_be_bytes());
}

fn xml_whitespace(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
}

fn normalise_line_ends(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            out.push(b'\n');
            index += 1;
            if bytes.get(index) == Some(&b'\n') {
                index += 1;
            }
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn normalise_attribute_value(bytes: &[u8]) -> Vec<u8> {
    normalise_line_ends(bytes)
        .into_iter()
        .map(|byte| {
            if matches!(byte, b'\t' | b'\n') {
                b' '
            } else {
                byte
            }
        })
        .collect()
}

fn is_xml_declaration(raw: &[u8]) -> bool {
    raw.get(0..5)
        .is_some_and(|head| head.eq_ignore_ascii_case(b"<?xml"))
}

fn reject_non_utf8_declaration(src: &[u8], tokens: &[Token]) -> Result<(), NormaliseError> {
    let Some(raw) = tokens
        .iter()
        .filter(|token| token.kind == Kind::Pi)
        .map(|token| token.span.of(src))
        .find(|raw| is_xml_declaration(raw))
    else {
        return Ok(());
    };
    let text = std::str::from_utf8(raw).expect("the scanner validated UTF-8");
    let lower = text.to_ascii_lowercase();
    let Some(at) = lower.find("encoding") else {
        return Ok(());
    };
    let after = &text[at + "encoding".len()..];
    let Some(equal) = after.find('=') else {
        return Err(NormaliseError::Malformed(
            "XML encoding declaration has no value".to_string(),
        ));
    };
    let value = after[equal + 1..].trim_start();
    let quote = value.as_bytes().first().copied().ok_or_else(|| {
        NormaliseError::Malformed("XML encoding declaration has no value".to_string())
    })?;
    if quote != b'\'' && quote != b'"' {
        return Err(NormaliseError::Malformed(
            "XML encoding declaration is unquoted".to_string(),
        ));
    }
    let rest = &value[1..];
    let end = rest.find(quote as char).ok_or_else(|| {
        NormaliseError::Malformed("XML encoding declaration is unterminated".to_string())
    })?;
    let encoding = &rest[..end];
    if !encoding.eq_ignore_ascii_case("utf-8") && !encoding.eq_ignore_ascii_case("utf8") {
        return Err(NormaliseError::DeclaredEncoding(encoding.to_string()));
    }
    Ok(())
}

fn unescape(bytes: &[u8]) -> Result<Vec<u8>, NormaliseError> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some(at) = rest.iter().position(|byte| *byte == b'&') {
        out.extend_from_slice(&rest[..at]);
        rest = &rest[at + 1..];
        let end = rest.iter().position(|byte| *byte == b';').ok_or_else(|| {
            NormaliseError::Malformed("unterminated entity reference".to_string())
        })?;
        let name = std::str::from_utf8(&rest[..end]).expect("the scanner validated UTF-8");
        match name {
            "amp" => out.push(b'&'),
            "lt" => out.push(b'<'),
            "gt" => out.push(b'>'),
            "quot" => out.push(b'"'),
            "apos" => out.push(b'\''),
            numeric if numeric.starts_with("#x") || numeric.starts_with("#X") => {
                push_codepoint(&mut out, &numeric[2..], 16, name)?
            }
            numeric if numeric.starts_with('#') => {
                push_codepoint(&mut out, &numeric[1..], 10, name)?
            }
            _ => return Err(NormaliseError::EntityReference(name.to_string())),
        }
        rest = &rest[end + 1..];
    }
    out.extend_from_slice(rest);
    Ok(out)
}

fn push_codepoint(
    out: &mut Vec<u8>,
    digits: &str,
    radix: u32,
    original: &str,
) -> Result<(), NormaliseError> {
    let value = u32::from_str_radix(digits, radix)
        .ok()
        .and_then(char::from_u32)
        .ok_or_else(|| NormaliseError::EntityReference(original.to_string()))?;
    let mut encoded = [0u8; 4];
    out.extend_from_slice(value.encode_utf8(&mut encoded).as_bytes());
    Ok(())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn sha256(input: &[u8]) -> [u8; 32] {
    Sha256::digest(input).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Project sent with an empty `dependsOn`, and the 10.2 server's export of it after an
    /// import whose Thing implements another project's ThingShape.
    #[test]
    fn the_projects_a_server_added_to_dependson_are_named_when_nothing_else_differs() {
        let sent = "<Entities majorVersion=\"10\" minorVersion=\"1\" universal=\"password\">
    <Projects>
        <Project
         artifactId=\"\"
         aspect.projectType=\"Component\"
         dependsOn=\"{&quot;extensions&quot;:&quot;&quot;,&quot;projects&quot;:&quot;&quot;}\"
         description=\"\"
         name=\"ZZ.B\"
         packageVersion=\"1.0.0\"
         projectName=\"ZZ.B\"
         state=\"DRAFT\">
            <avatar></avatar>
            <DesignTimePermissions>
                <Create></Create>
                <Read></Read>
            </DesignTimePermissions>
            <RunTimePermissions></RunTimePermissions>
        </Project>
    </Projects>
</Entities>
";
        let read_back = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?><Entities build=\"b75\" majorVersion=\"10\" minorVersion=\"2\" universal=\"\"><Projects><Project artifactId=\"\" aspect.projectType=\"Component\" dependsOn=\"{&quot;extensions&quot;:&quot;&quot;,&quot;projects&quot;:&quot;ZZ.A:1.0.0&quot;}\" description=\"\" lastModifiedDate=\"2026-10-28T08:25:06.565Z\" name=\"ZZ.B\" packageVersion=\"1.0.0\" projectName=\"ZZ.B\" state=\"DRAFT\"><Owner name=\"Administrator\" type=\"User\"/><avatar/><DesignTimePermissions><Read/><Create/></DesignTimePermissions><RunTimePermissions/><ConfigurationChanges><ConfigurationChange changeAction=\"CREATE\" changeReason=\"\" timestamp=\"2026-10-28T08:25:06.487Z\" user=\"Administrator\"/></ConfigurationChanges></Project></Projects></Entities>";
        assert_ne!(
            hash(sent.as_bytes()).unwrap(),
            hash(read_back.as_bytes()).unwrap()
        );
        assert_eq!(
            added_project_dependencies(sent.as_bytes(), read_back.as_bytes()),
            ["ZZ.A"]
        );
        // Another difference as well: not only the dependencies, so nothing is named.
        let described = read_back.replace("description=\"\"", "description=\"changed\"");
        assert!(added_project_dependencies(sent.as_bytes(), described.as_bytes()).is_empty());
        // A dependency the server dropped is not one it added.
        assert!(added_project_dependencies(read_back.as_bytes(), sent.as_bytes()).is_empty());
    }

    #[test]
    fn a_read_back_names_where_it_differs_without_values() {
        let sent = r#"<Entities><Things><Thing name="P.T" description="d" password="s3cret"><ThingShape><PropertyDefinitions><PropertyDefinition name="a" baseType="NUMBER"/><PropertyDefinition name="b" baseType="STRING"/></PropertyDefinitions></ThingShape><ThingProperties><a><Value>1</Value></a></ThingProperties></Thing></Things></Entities>"#;
        let back = r#"<Thing name="P.T" description="other" password="t0ps" lastModifiedDate="2026-10-29T08:00:00.000Z"><Owner name="Administrator" type="User"/><ThingShape><PropertyDefinitions><PropertyDefinition name="c" baseType="STRING"/><PropertyDefinition name="a" baseType="INTEGER"/></PropertyDefinitions></ThingShape><ThingProperties><a><Value>2</Value><Timestamp>2026-10-29T08:00:00.000Z</Timestamp></a></ThingProperties></Thing>"#;
        let found = read_back_differences(sent.as_bytes(), back.as_bytes(), &[]);
        assert_eq!(
            found,
            [
                "Thing[P.T]/@description: changed",
                "Thing[P.T]/@password: changed",
                "Thing[P.T]/ThingShape/PropertyDefinitions/PropertyDefinition[b]: dropped by the server",
                "Thing[P.T]/ThingShape/PropertyDefinitions/PropertyDefinition[c]: added by the server",
                "Thing[P.T]/ThingShape/PropertyDefinitions/PropertyDefinition[a]/@baseType: changed",
            ]
        );
        assert!(!found
            .iter()
            .any(|line| line.contains("s3cret") || line.contains("t0ps")));
        // The sixth difference, the stored value, is past the limit; on its own it is text.
        let only_value = sent.replace("<Value>1</Value>", "<Value>2</Value>");
        assert_eq!(
            read_back_differences(sent.as_bytes(), only_value.as_bytes(), &[]),
            ["Thing[P.T]/ThingProperties/a/Value: text changed"]
        );
        // What the hash ignores is no difference.
        let reindented = sent.replace(
            "><", ">
  <",
        );
        assert!(read_back_differences(sent.as_bytes(), reindented.as_bytes(), &[]).is_empty());
    }

    #[test]
    fn named_siblings_in_another_order_differ_unless_the_hash_sorts_them() {
        let events = |first: &str, second: &str| {
            format!(
                r#"<Thing name="T"><ThingShape><EventHandlers><EventHandler name="{first}"/><EventHandler name="{second}"/></EventHandlers></ThingShape></Thing>"#
            )
        };
        let (sent, back) = (events("A", "B"), events("B", "A"));
        assert_ne!(
            hash(sent.as_bytes()).unwrap(),
            hash(back.as_bytes()).unwrap()
        );
        assert_eq!(
            read_back_differences(sent.as_bytes(), back.as_bytes(), &[]),
            ["Thing[T]/ThingShape/EventHandlers: its elements are in another order"]
        );
        // ServiceDefinitions is a set the hash sorts by name.
        let services = sent
            .replace("EventHandlers", "ServiceDefinitions")
            .replace("EventHandler", "ServiceDefinition");
        let swapped = back
            .replace("EventHandlers", "ServiceDefinitions")
            .replace("EventHandler", "ServiceDefinition");
        assert_eq!(
            hash(services.as_bytes()).unwrap(),
            hash(swapped.as_bytes()).unwrap()
        );
        assert!(read_back_differences(services.as_bytes(), swapped.as_bytes(), &[]).is_empty());
        let mixed = r#"<Thing name="T"><a>x<b/></a></Thing>"#;
        let moved = r#"<Thing name="T"><a><b/>x</a></Thing>"#;
        assert_eq!(
            read_back_differences(mixed.as_bytes(), moved.as_bytes(), &[]),
            ["Thing[T]: its text and elements interleave differently"]
        );
    }

    #[test]
    fn unnamed_siblings_are_compared_in_order() {
        let sent = r#"<Mashup name="M"><Things><Thing>A</Thing><Thing>B</Thing></Things></Mashup>"#;
        let fewer = r#"<Mashup name="M"><Things><Thing>A</Thing></Things></Mashup>"#;
        assert_eq!(
            read_back_differences(sent.as_bytes(), fewer.as_bytes(), &[]),
            ["Mashup[M]/Things: 2 child elements sent, 1 read back"]
        );
        let changed =
            r#"<Mashup name="M"><Things><Thing>A</Thing><Thing>C</Thing></Things></Mashup>"#;
        assert_eq!(
            read_back_differences(sent.as_bytes(), changed.as_bytes(), &[]),
            ["Mashup[M]/Things/Thing[2]: text changed"]
        );
    }

    const LIVE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?><Thing projectName="P" name="P.T" lastModifiedDate="now"><Owner name="admin"></Owner><effectiveShape><x/></effectiveShape><effectiveRemoteServiceBindings><Binding/></effectiveRemoteServiceBindings><ConfigurationChanges><ConfigurationChange timestamp="now"/></ConfigurationChanges><ServiceDefinitions><ServiceDefinition name="S"><Implementation><code><![CDATA[var x = 1;
]]></code></Implementation></ServiceDefinition></ServiceDefinitions></Thing>"#;
    const COMMITTED: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Entities minorVersion="1" majorVersion="10">
    <Things>
        <Thing name="P.T" projectName="P">
            <ServiceDefinitions>
                <ServiceDefinition name="S">
                    <Implementation>
                        <code><![CDATA[var x = 1;
]]></code>
                    </Implementation>
                </ServiceDefinition>
            </ServiceDefinitions>
        </Thing>
    </Things>
</Entities>"#;

    #[test]
    fn live_and_committed_shapes_normalise_equal() {
        assert_eq!(normalise(LIVE).unwrap(), normalise(COMMITTED).unwrap());
        assert_eq!(hash(LIVE).unwrap(), hash(COMMITTED).unwrap());
    }

    #[test]
    fn a_one_character_cdata_edit_changes_the_hash() {
        let changed = String::from_utf8(LIVE.to_vec())
            .unwrap()
            .replace("x = 1", "x = 2");
        assert_ne!(hash(LIVE).unwrap(), hash(changed.as_bytes()).unwrap());
    }

    #[test]
    fn whitespace_inside_cdata_is_significant() {
        let changed = String::from_utf8(LIVE.to_vec())
            .unwrap()
            .replace("x = 1", "x  = 1");
        assert_ne!(hash(LIVE).unwrap(), hash(changed.as_bytes()).unwrap());
    }

    #[test]
    fn only_name_keyed_containers_are_reordered() {
        let fields_ab = br#"<DataShape><FieldDefinitions><FieldDefinition name="A"/><FieldDefinition name="B"/></FieldDefinitions></DataShape>"#;
        let fields_ba = br#"<DataShape><FieldDefinitions><FieldDefinition name="B"/><FieldDefinition name="A"/></FieldDefinitions></DataShape>"#;
        assert_eq!(hash(fields_ab).unwrap(), hash(fields_ba).unwrap());

        let rows_ab = br#"<DataTable><Rows><Row name="A"/><Row name="B"/></Rows></DataTable>"#;
        let rows_ba = br#"<DataTable><Rows><Row name="B"/><Row name="A"/></Rows></DataTable>"#;
        assert_ne!(hash(rows_ab).unwrap(), hash(rows_ba).unwrap());
    }

    #[test]
    fn mashup_json_layout_and_key_order_are_ignored_but_values_and_array_order_are_not() {
        let pretty = br#"<Mashup><mashupContent>
  <![CDATA[
    {"first": 1, "second": {"value": true}}
  ]]>
</mashupContent></Mashup>"#;
        let compact = br#"<Mashup><mashupContent><![CDATA[{"first":1,"second":{"value":true}}]]></mashupContent></Mashup>"#;
        let changed = br#"<Mashup><mashupContent><![CDATA[{"first":2,"second":{"value":true}}]]></mashupContent></Mashup>"#;
        let reordered = br#"<Mashup><mashupContent><![CDATA[{"second":{"value":true},"first":1}]]></mashupContent></Mashup>"#;
        assert_eq!(hash(pretty).unwrap(), hash(compact).unwrap());
        assert_ne!(hash(compact).unwrap(), hash(changed).unwrap());
        assert_eq!(hash(compact).unwrap(), hash(reordered).unwrap());
        // The observed case: the data sources inside the Data map, one level down.
        let data_ab = br#"<Mashup><mashupContent><![CDATA[{"Data":{"A":{"x":1},"B":{}}}]]></mashupContent></Mashup>"#;
        let data_ba = br#"<Mashup><mashupContent><![CDATA[{"Data":{"B":{},"A":{"x":1}}}]]></mashupContent></Mashup>"#;
        assert_eq!(hash(data_ab).unwrap(), hash(data_ba).unwrap());
        let items_ab = br#"<Mashup><mashupContent><![CDATA[{"Widgets":[{"a":1},{"b":2}]}]]></mashupContent></Mashup>"#;
        let items_ba = br#"<Mashup><mashupContent><![CDATA[{"Widgets":[{"b":2},{"a":1}]}]]></mashupContent></Mashup>"#;
        assert_ne!(hash(items_ab).unwrap(), hash(items_ba).unwrap());
    }

    #[test]
    fn an_organizations_connections_and_units_compare_as_sets() {
        let sent = br#"<Organization name="O"><Connections><Connection from="Root" to="A"/><Connection from="Root" to="B"/></Connections><OrganizationalUnits><OrganizationalUnit name="A"><Members/></OrganizationalUnit><OrganizationalUnit name="B"><Members/></OrganizationalUnit></OrganizationalUnits></Organization>"#;
        let back = br#"<Organization name="O"><Connections><Connection from="Root" to="B"/><Connection from="Root" to="A"/></Connections><OrganizationalUnits><OrganizationalUnit name="B"><Members/></OrganizationalUnit><OrganizationalUnit name="A"><Members/></OrganizationalUnit></OrganizationalUnits></Organization>"#;
        let changed = br#"<Organization name="O"><Connections><Connection from="Root" to="B"/><Connection from="A" to="A"/></Connections><OrganizationalUnits><OrganizationalUnit name="B"><Members/></OrganizationalUnit><OrganizationalUnit name="A"><Members/></OrganizationalUnit></OrganizationalUnits></Organization>"#;
        assert_eq!(hash(sent).unwrap(), hash(back).unwrap());
        assert_ne!(hash(sent).unwrap(), hash(changed).unwrap());
        // The same lists elsewhere keep their order.
        let thing_ab = br#"<Thing name="T"><Connections><Connection from="Root" to="A"/><Connection from="Root" to="B"/></Connections></Thing>"#;
        let thing_ba = br#"<Thing name="T"><Connections><Connection from="Root" to="B"/><Connection from="Root" to="A"/></Connections></Thing>"#;
        assert_ne!(hash(thing_ab).unwrap(), hash(thing_ba).unwrap());
    }

    #[test]
    fn invalid_mashup_json_is_compared_after_leaf_trimming_without_error() {
        let original =
            br#"<Mashup><mashupContent><![CDATA[{invalid json}]]></mashupContent></Mashup>"#;
        let changed =
            br#"<Mashup><mashupContent><![CDATA[{invalid  json}]]></mashupContent></Mashup>"#;
        assert_ne!(hash(original).unwrap(), hash(changed).unwrap());
    }

    #[test]
    fn media_base64_layout_is_ignored_but_payload_changes_are_not() {
        let wrapped = br#"<MediaEntity><content>
  <![CDATA[YWJj
    ZGVm]]>
</content></MediaEntity>"#;
        let compact = br#"<MediaEntity><content><![CDATA[YWJjZGVm]]></content></MediaEntity>"#;
        let changed = br#"<MediaEntity><content><![CDATA[YWJjZGVn]]></content></MediaEntity>"#;
        assert_eq!(hash(wrapped).unwrap(), hash(compact).unwrap());
        assert_ne!(hash(compact).unwrap(), hash(changed).unwrap());
    }

    #[test]
    fn nested_live_only_names_are_not_stripped() {
        let nested = br#"<Thing><Container><Owner name="Administrator"/></Container></Thing>"#;
        let absent = br#"<Thing><Container/></Thing>"#;
        assert_ne!(hash(nested).unwrap(), hash(absent).unwrap());
    }

    #[test]
    fn indented_cdata_leaf_equals_trimmed_but_interior_indentation_is_significant() {
        let wrapped = b"<Thing><code>\n<![CDATA[\n    first\n    second\n]]>\n</code></Thing>";
        let compact = b"<Thing><code><![CDATA[first\n    second]]></code></Thing>";
        let interior_changed = b"<Thing><code><![CDATA[first\n     second]]></code></Thing>";
        assert_eq!(hash(wrapped).unwrap(), hash(compact).unwrap());
        assert_ne!(hash(compact).unwrap(), hash(interior_changed).unwrap());
    }

    #[test]
    fn whitespace_only_leaf_equals_empty_element() {
        let whitespace = b"<Thing><ParameterDefinitions>\n    </ParameterDefinitions></Thing>";
        let empty = b"<Thing><ParameterDefinitions></ParameterDefinitions></Thing>";
        assert_eq!(hash(whitespace).unwrap(), hash(empty).unwrap());
    }

    #[test]
    fn state_definition_content_json_layout_is_ignored_but_values_are_not() {
        let pretty = br#"<StateDefinition><content><![CDATA[
  {"stateType": "numeric", "defaultValue": 1}
]]></content></StateDefinition>"#;
        let compact = br#"<StateDefinition><content><![CDATA[{"stateType":"numeric","defaultValue":1}]]></content></StateDefinition>"#;
        let changed = br#"<StateDefinition><content><![CDATA[{"stateType":"numeric","defaultValue":2}]]></content></StateDefinition>"#;
        assert_eq!(hash(pretty).unwrap(), hash(compact).unwrap());
        assert_ne!(hash(compact).unwrap(), hash(changed).unwrap());
    }

    #[test]
    fn content_json_rule_is_limited_to_named_entity_types() {
        let spaced = br#"<Thing><content><![CDATA[{"value": 1}]]></content></Thing>"#;
        let compact = br#"<Thing><content><![CDATA[{"value":1}]]></content></Thing>"#;
        assert_ne!(hash(spaced).unwrap(), hash(compact).unwrap());
    }

    #[test]
    fn hash_and_framing_are_version_nine() {
        assert!(hash(b"<Thing/>").unwrap().starts_with("v10:"));
        assert!(normalise(b"<Thing/>")
            .unwrap()
            .starts_with(b"twaco-entity-normalise-v9\0"));
    }

    #[test]
    fn locally_typed_values_use_server_text_but_untyped_values_do_not() {
        let sent = br#"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name="rate" baseType="NUMBER" aspect.defaultValue="10800000.0"/><PropertyDefinition name="json" baseType="JSON" aspect.defaultValue="{&quot;b&quot;: 1, &quot;a&quot;: [1,2]}"/><PropertyDefinition name="big" baseType="LONG" aspect.defaultValue="9007199254740993"/></PropertyDefinitions></ThingShape><ConfigurationTables><ConfigurationTable name="Settings"><DataShape><FieldDefinitions><FieldDefinition name="rate" baseType="NUMBER"/></FieldDefinitions></DataShape><Rows><Row><rate>10800000.0</rate></Row></Rows></ConfigurationTable></ConfigurationTables><ThingProperties><rate><Value>10800000.0</Value></rate></ThingProperties><plain><Value>1E7</Value></plain></Thing>"#;
        let returned = br#"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name="rate" baseType="NUMBER" aspect.defaultValue="1.08E7"/><PropertyDefinition name="json" baseType="JSON" aspect.defaultValue="{&quot;a&quot;:[1,2],&quot;b&quot;:1}"/><PropertyDefinition name="big" baseType="LONG" aspect.defaultValue="9007199254740993"/></PropertyDefinitions></ThingShape><ConfigurationTables><ConfigurationTable name="Settings"><DataShape><FieldDefinitions><FieldDefinition name="rate" baseType="NUMBER"/></FieldDefinitions></DataShape><Rows><Row><rate>1.08E7</rate></Row></Rows></ConfigurationTable></ConfigurationTables><ThingProperties><rate><Value>1.08E7</Value></rate></ThingProperties><plain><Value>10000000</Value></plain></Thing>"#;
        assert_ne!(hash(sent).unwrap(), hash(returned).unwrap());
        let same_without_plain = std::str::from_utf8(sent)
            .unwrap()
            .replace("<plain><Value>1E7</Value></plain>", "");
        let returned_without_plain = std::str::from_utf8(returned)
            .unwrap()
            .replace("<plain><Value>10000000</Value></plain>", "");
        assert_eq!(
            hash(same_without_plain.as_bytes()).unwrap(),
            hash(returned_without_plain.as_bytes()).unwrap()
        );
        let rounded_long = returned_without_plain.replace("9007199254740993", "9007199254740992");
        assert_ne!(
            hash(same_without_plain.as_bytes()).unwrap(),
            hash(rounded_long.as_bytes()).unwrap()
        );
    }

    #[test]
    fn empty_cdata_leaf_equals_empty_element() {
        assert_eq!(
            hash(b"<Thing><code><![CDATA[\n\n]]></code></Thing>").unwrap(),
            hash(b"<Thing><code/></Thing>").unwrap()
        );
    }

    #[test]
    fn property_definitions_are_name_keyed() {
        let inserted = b"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name=\"z\" baseType=\"STRING\"/><PropertyDefinition name=\"a\" baseType=\"STRING\"/></PropertyDefinitions></ThingShape></Thing>";
        let returned = b"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name=\"a\" baseType=\"STRING\"/><PropertyDefinition name=\"z\" baseType=\"STRING\"/></PropertyDefinitions></ThingShape></Thing>";
        assert_eq!(hash(inserted).unwrap(), hash(returned).unwrap());
    }

    #[test]
    fn split_cdata_is_one_payload_but_its_contents_still_matter() {
        let split = b"<Thing><code>\n<![CDATA[first]]]]><![CDATA[>second]]>\n</code></Thing>";
        let joined = b"<Thing><code><![CDATA[first]]]]><![CDATA[>second]]></code></Thing>";
        let changed = b"<Thing><code><![CDATA[first]]]]><![CDATA[>third]]></code></Thing>";
        assert_eq!(hash(split).unwrap(), hash(joined).unwrap());
        assert_ne!(hash(joined).unwrap(), hash(changed).unwrap());
    }

    #[test]
    fn subscriptions_and_events_are_name_keyed() {
        for container in ["Subscriptions", "EventDefinitions"] {
            let first = format!(
                "<Thing><{container}><Item name=\"A\"/><Item name=\"B\"/></{container}></Thing>"
            );
            let second = first.replace("A\"/><Item name=\"B", "B\"/><Item name=\"A");
            assert_eq!(
                hash(first.as_bytes()).unwrap(),
                hash(second.as_bytes()).unwrap()
            );
        }
    }

    #[test]
    fn query_and_location_values_compare_by_declared_type() {
        let sent = br#"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name="where" baseType="LOCATION" aspect.defaultValue="45.5,9.2,100"/><PropertyDefinition name="filter" baseType="QUERY" aspect.defaultValue="{&quot;b&quot;:1,&quot;a&quot;:2}"/></PropertyDefinitions></ThingShape></Thing>"#;
        let returned = br#"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name="where" baseType="LOCATION" aspect.defaultValue="45.5,9.2,100.0"/><PropertyDefinition name="filter" baseType="QUERY" aspect.defaultValue="{&quot;a&quot;:2,&quot;b&quot;:1}"/></PropertyDefinitions></ThingShape></Thing>"#;
        assert_eq!(hash(sent).unwrap(), hash(returned).unwrap());
    }

    #[test]
    fn timespan_and_variant_defaults_compare_with_their_exported_form() {
        let thing = |span: &str, any: &str| {
            format!("<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name=\"span\" baseType=\"TIMESPAN\" aspect.defaultValue=\"{span}\"/><PropertyDefinition name=\"any\" baseType=\"VARIANT\" aspect.defaultValue=\"{any}\"/></PropertyDefinitions></ThingShape></Thing>")
        };
        // As imported, and as a 10.2 server exported them (2026-10-30).
        let sent = thing(",,00:00:00:01:00:00", "abc");
        let exported = thing(
            "{period=00:00:00:01:00:00}",
            "{&quot;value&quot;:&quot;abc&quot;,&quot;baseType&quot;:&quot;STRING&quot;}",
        );
        let other = thing(",,00:00:00:02:00:00", "abc");
        assert_eq!(
            hash(sent.as_bytes()).unwrap(),
            hash(exported.as_bytes()).unwrap()
        );
        assert_ne!(
            hash(sent.as_bytes()).unwrap(),
            hash(other.as_bytes()).unwrap()
        );
    }

    #[test]
    fn vector_values_compare_as_the_doubles_the_server_writes() {
        let sent = br#"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name="v" baseType="VEC2" aspect.defaultValue="1,2"/></PropertyDefinitions></ThingShape><ThingProperties><v><Value>1,2</Value></v></ThingProperties></Thing>"#;
        let stored = br#"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name="v" baseType="VEC2" aspect.defaultValue="1.0,2.0"/></PropertyDefinitions></ThingShape><ThingProperties><v><Value>1.0,2.0</Value></v></ThingProperties></Thing>"#;
        let other = br#"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name="v" baseType="VEC2" aspect.defaultValue="1.0,3.0"/></PropertyDefinitions></ThingShape><ThingProperties><v><Value>1.0,2.0</Value></v></ThingProperties></Thing>"#;
        assert_eq!(hash(sent).unwrap(), hash(stored).unwrap());
        assert_ne!(hash(stored).unwrap(), hash(other).unwrap());
    }

    #[test]
    fn stored_json_and_query_values_compare_with_sorted_keys() {
        // As written, and as a 10.2 server stored them (2026-10-30).
        let definitions = r#"<ThingShape><PropertyDefinitions><PropertyDefinition name="cfg" baseType="JSON" isPersistent="true"/><PropertyDefinition name="flt" baseType="QUERY" isPersistent="true"/></PropertyDefinitions></ThingShape>"#;
        let thing = |cfg: &str, flt: &str| {
            format!("<Thing>{definitions}<ThingProperties><cfg><Value><json><![CDATA[{cfg}]]></json></Value></cfg><flt><Value><json><![CDATA[{flt}]]></json></Value></flt></ThingProperties></Thing>")
        };
        let sent = thing(
            r#"{"b":1,"a":{"d":2,"c":3}}"#,
            r#"{"filters":{"type":"EQ","fieldName":"x"}}"#,
        );
        let stored = thing(
            r#"{"a":{"c":3,"d":2},"b":1}"#,
            r#"{"filters":{"fieldName":"x","type":"EQ"}}"#,
        );
        let other = thing(
            r#"{"a":{"c":3,"d":2},"b":2}"#,
            r#"{"filters":{"fieldName":"x","type":"EQ"}}"#,
        );
        assert_eq!(
            hash(sent.as_bytes()).unwrap(),
            hash(stored.as_bytes()).unwrap()
        );
        assert_ne!(
            hash(stored.as_bytes()).unwrap(),
            hash(other.as_bytes()).unwrap()
        );
    }

    #[test]
    fn a_nested_infotable_field_does_not_retype_the_table_field_of_its_name() {
        let thing = |x: &str| {
            format!("<Thing><ConfigurationTables><ConfigurationTable name=\"Settings\"><DataShape><FieldDefinitions><FieldDefinition name=\"x\" baseType=\"STRING\"/><FieldDefinition name=\"nested\" baseType=\"INFOTABLE\"/></FieldDefinitions></DataShape><Rows><Row><x>{x}</x><nested><infoTable><DataShape><FieldDefinitions><FieldDefinition name=\"x\" baseType=\"NUMBER\"/></FieldDefinitions></DataShape><Rows></Rows></infoTable></nested></Row></Rows></ConfigurationTable></ConfigurationTables></Thing>")
        };
        assert_ne!(
            hash(thing("1").as_bytes()).unwrap(),
            hash(thing("1.0").as_bytes()).unwrap()
        );
    }

    #[test]
    fn stored_values_compare_in_any_order_but_not_with_other_values() {
        // As written, and as a 10.1 server exported the same Thing (2026-10-16).
        let sent = br#"<Thing name="T"><ThingProperties><Speed><Value>12.5</Value><Quality>UNKNOWN</Quality></Speed><Label><Value><![CDATA[hi]]></Value><Quality>UNKNOWN</Quality></Label></ThingProperties></Thing>"#;
        let read_back = br#"<Thing name="T"><ThingProperties><Label><Value><![CDATA[hi]]></Value><Quality>UNKNOWN</Quality></Label><Speed><Value>12.5</Value><Quality>UNKNOWN</Quality></Speed></ThingProperties></Thing>"#;
        let other = br#"<Thing name="T"><ThingProperties><Label><Value><![CDATA[hi]]></Value><Quality>UNKNOWN</Quality></Label><Speed><Value>13.0</Value><Quality>UNKNOWN</Quality></Speed></ThingProperties></Thing>"#;
        assert_eq!(hash(sent).unwrap(), hash(read_back).unwrap());
        assert_ne!(hash(read_back).unwrap(), hash(other).unwrap());
        // Re-stamped by an import: the same stored value.
        let restamped = br#"<Thing name="T"><ThingProperties><Label><Value><![CDATA[hi]]></Value><Timestamp>2026-10-16T16:57:57.455Z</Timestamp><Quality>GOOD</Quality></Label><Speed><Value>12.5</Value><Quality>UNKNOWN</Quality></Speed></ThingProperties></Thing>"#;
        assert_eq!(hash(read_back).unwrap(), hash(restamped).unwrap());
        // Only the entity's own stored values: elsewhere order still counts.
        let rows = br#"<Thing name="T"><X><B/><A/></X></Thing>"#;
        let swapped = br#"<Thing name="T"><X><A/><B/></X></Thing>"#;
        assert_ne!(hash(rows).unwrap(), hash(swapped).unwrap());
    }

    /// What was sent, and what a 10.1 server read back after importing it (2026-10-07).
    const PERMISSIONS_SENT: &[u8] = br#"<Thing name="T"><DesignTimePermissions><Create/><Read><Principal isPermitted="true" name="Users" type="Group"/><Principal isPermitted="false" name="Administrators" type="Group"/></Read><Update/><Delete/><Metadata/></DesignTimePermissions><RunTimePermissions><Permissions resourceName="*"><PropertyRead><Principal isPermitted="true" name="Users" type="Group"/><Principal isPermitted="true" name="Administrators" type="Group"/></PropertyRead><PropertyWrite/><ServiceInvoke><Principal isPermitted="true" name="Users" type="Group"/></ServiceInvoke><EventInvoke/><EventSubscribe/></Permissions><Permissions resourceName="GetPropertyValues"><ServiceInvoke><Principal isPermitted="false" name="Users" type="Group"/></ServiceInvoke></Permissions></RunTimePermissions><VisibilityPermissions><Visibility><Principal isPermitted="true" name="O:U" type="OrganizationalUnit"/><Principal isPermitted="true" name="O" type="Organization"/></Visibility></VisibilityPermissions></Thing>"#;
    const PERMISSIONS_READ_BACK: &[u8] = br#"<Thing name="T"><DesignTimePermissions><Create/><Read><Principal isPermitted="false" name="Administrators" type="Group"/><Principal isPermitted="true" name="Users" type="Group"/></Read><Update/><Delete/><Metadata/></DesignTimePermissions><RunTimePermissions><Permissions resourceName="GetPropertyValues"><PropertyRead/><PropertyWrite/><ServiceInvoke><Principal isPermitted="false" name="Users" type="Group"/></ServiceInvoke><EventInvoke/><EventSubscribe/></Permissions><Permissions resourceName="*"><PropertyRead><Principal isPermitted="true" name="Administrators" type="Group"/><Principal isPermitted="true" name="Users" type="Group"/></PropertyRead><PropertyWrite/><ServiceInvoke><Principal isPermitted="true" name="Users" type="Group"/></ServiceInvoke><EventInvoke/><EventSubscribe/></Permissions></RunTimePermissions><VisibilityPermissions><Visibility><Principal isPermitted="true" name="O" type="Organization"/><Principal isPermitted="true" name="O:U" type="OrganizationalUnit"/></Visibility></VisibilityPermissions></Thing>"#;

    #[test]
    fn an_imported_permission_block_reads_back_equal() {
        assert_eq!(
            hash(PERMISSIONS_SENT).unwrap(),
            hash(PERMISSIONS_READ_BACK).unwrap()
        );
    }

    #[test]
    fn an_instance_permission_block_is_a_set_too() {
        // A ThingShape as exported (empty kinds, two principals in one order), and as an import
        // reads it back: kinds filled in, `*` added, principals reordered.
        let sent = br#"<ThingShape name="S"><InstanceRunTimePermissions><Permissions resourceName="GetX"><ServiceInvoke><Principal isPermitted="true" name="B" type="Group"/><Principal isPermitted="true" name="A" type="Group"/></ServiceInvoke></Permissions></InstanceRunTimePermissions></ThingShape>"#;
        let back = br#"<ThingShape name="S"><InstanceRunTimePermissions><Permissions resourceName="*"><PropertyRead/><PropertyWrite/><ServiceInvoke/><EventInvoke/><EventSubscribe/></Permissions><Permissions resourceName="GetX"><PropertyRead/><PropertyWrite/><ServiceInvoke><Principal isPermitted="true" name="A" type="Group"/><Principal isPermitted="true" name="B" type="Group"/></ServiceInvoke><EventInvoke/><EventSubscribe/></Permissions></InstanceRunTimePermissions></ThingShape>"#;
        assert_eq!(hash(sent).unwrap(), hash(back).unwrap());
        let denied = std::str::from_utf8(sent).unwrap().replacen(
            r#"isPermitted="true" name="A""#,
            r#"isPermitted="false" name="A""#,
            1,
        );
        assert_ne!(hash(denied.as_bytes()).unwrap(), hash(sent).unwrap());
        assert!(differ_only_in_permissions(sent, denied.as_bytes()));
    }

    #[test]
    fn a_changed_grant_still_changes_the_hash() {
        let sent = std::str::from_utf8(PERMISSIONS_SENT).unwrap();
        for changed in [
            sent.replacen(
                r#"isPermitted="false" name="Users""#,
                r#"isPermitted="true" name="Users""#,
                1,
            ),
            sent.replacen(
                r#"resourceName="GetPropertyValues""#,
                r#"resourceName="GetProperties""#,
                1,
            ),
            sent.replacen(
                r#"name="O" type="Organization""#,
                r#"name="P" type="Organization""#,
                1,
            ),
            sent.replacen(
                "<Update/>",
                r#"<Update><Principal isPermitted="true" name="Users" type="Group"/></Update>"#,
                1,
            ),
        ] {
            assert_ne!(changed, sent);
            assert_ne!(
                hash(changed.as_bytes()).unwrap(),
                hash(PERMISSIONS_SENT).unwrap(),
                "{changed}"
            );
        }
        // A grant moved from one kind to another is a different permission.
        let moved = sent.replacen(
            r#"<PropertyWrite/><ServiceInvoke><Principal isPermitted="true" name="Users" type="Group"/></ServiceInvoke>"#,
            r#"<PropertyWrite><Principal isPermitted="true" name="Users" type="Group"/></PropertyWrite><ServiceInvoke/>"#,
            1,
        );
        assert_ne!(moved, sent);
        assert_ne!(
            hash(moved.as_bytes()).unwrap(),
            hash(PERMISSIONS_SENT).unwrap()
        );
    }

    #[test]
    fn a_difference_only_in_permissions_is_told_apart_from_any_other() {
        let sent = std::str::from_utf8(PERMISSIONS_SENT).unwrap();
        let extra_grant = sent.replacen(
            "<Update/>",
            r#"<Update><Principal isPermitted="true" name="Users" type="Group"/></Update>"#,
            1,
        );
        assert!(differ_only_in_permissions(
            PERMISSIONS_SENT,
            extra_grant.as_bytes()
        ));
        // Equal is not "differs only in permissions".
        assert!(!differ_only_in_permissions(
            PERMISSIONS_SENT,
            PERMISSIONS_READ_BACK
        ));
        let other_change = extra_grant.replacen(r#"name="T""#, r#"name="T" description="x""#, 1);
        assert!(!differ_only_in_permissions(
            PERMISSIONS_SENT,
            other_change.as_bytes()
        ));
    }

    #[test]
    fn order_outside_a_permission_block_still_counts() {
        let first = br#"<Thing><PropertyBindings><A/><B/></PropertyBindings></Thing>"#;
        let second = br#"<Thing><PropertyBindings><B/><A/></PropertyBindings></Thing>"#;
        assert_ne!(hash(first).unwrap(), hash(second).unwrap());
    }

    #[test]
    fn crlf_and_lf_scripts_hash_equal() {
        let crlf = b"<Thing><code><![CDATA[first\r\nsecond]]></code></Thing>";
        let lf = b"<Thing><code><![CDATA[first\nsecond]]></code></Thing>";
        assert_eq!(hash(crlf).unwrap(), hash(lf).unwrap());
    }

    #[test]
    fn a_lone_cr_equals_lf() {
        let cr = b"<Thing><code><![CDATA[first\rsecond]]></code></Thing>";
        let lf = b"<Thing><code><![CDATA[first\nsecond]]></code></Thing>";
        assert_eq!(hash(cr).unwrap(), hash(lf).unwrap());
    }

    #[test]
    fn literal_line_ends_are_folded_but_explicit_cr_references_are_not() {
        let attribute_literal_cr = b"<Thing value=\"first\rsecond\"/>";
        let attribute_space = br#"<Thing value="first second"/>"#;
        let attribute_cr = br#"<Thing value="first&#13;second"/>"#;
        let text_literal_cr = b"<Thing>first\rsecond</Thing>";
        let text_cr = b"<Thing>first&#13;second</Thing>";
        let text_lf = b"<Thing>first&#10;second</Thing>";

        assert_eq!(
            hash(attribute_literal_cr).unwrap(),
            hash(attribute_space).unwrap()
        );
        assert_ne!(hash(attribute_cr).unwrap(), hash(attribute_space).unwrap());
        assert_eq!(hash(text_literal_cr).unwrap(), hash(text_lf).unwrap());
        assert_ne!(hash(text_cr).unwrap(), hash(text_lf).unwrap());
    }

    #[test]
    fn a_literal_attribute_newline_equals_a_space_but_a_reference_does_not() {
        let newline = b"<Thing value=\"first\nsecond\"/>";
        let space = b"<Thing value=\"first second\"/>";
        let reference = br#"<Thing value="first&#10;second"/>"#;
        assert_eq!(hash(newline).unwrap(), hash(space).unwrap());
        assert_ne!(hash(reference).unwrap(), hash(space).unwrap());
    }

    #[test]
    fn a_changed_attribute_value_changes_the_hash() {
        let changed = String::from_utf8(LIVE.to_vec())
            .unwrap()
            .replace("projectName=\"P\"", "projectName=\"Q\"");
        assert_ne!(hash(LIVE).unwrap(), hash(changed.as_bytes()).unwrap());
    }

    #[test]
    fn sha256_matches_the_published_abc_vector() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_matches_the_published_two_block_vector() {
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn a_non_utf8_declaration_is_named_even_when_the_bytes_are_ascii() {
        let xml = br#"<?xml version="1.0" encoding="ISO-8859-1"?><Thing name="T"/>"#;
        assert!(matches!(
            normalise(xml),
            Err(NormaliseError::DeclaredEncoding(_))
        ));
    }
}
