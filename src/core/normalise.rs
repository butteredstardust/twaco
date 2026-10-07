//! Structural entity normalisation for conflict detection.
//!
//! The canonical bytes are intentionally an internal binary framing rather than pretty XML:
//! they are hash input, never an export. Length-prefixing makes element, attribute, text and
//! CDATA boundaries unambiguous without choosing an escaping style.
//!
//! Normalisation version 5 has exactly these non-semantic rules:
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
//!   and `ConfigurationTableDefinitions`; some live exports reorder these named sections;
//! - inside `DesignTimePermissions`, `RunTimePermissions` and `VisibilityPermissions`, and the
//!   `Instance...` forms of the three on a ThingShape or ThingTemplate, treat every list as the
//!   set it is: drop a permission kind or a `Permissions` resource that grants no principal, and
//!   sort the rest by canonical bytes. An import reorders principals and resources and fills in
//!   the kinds a resource left out (observed 2026-10-07; version 4, instance blocks version 5);
//! - compact the single JSON CDATA payload in `mashupContent`; mashup JSON differed only in layout,
//!   as did direct-child `content` in observed `StateDefinition` and `StyleTheme` entities;
//! - remove ASCII whitespace from a single CDATA payload in a `MediaEntity`'s direct-child
//!   `content`, matching the wrapped committed and single-line live base64 representations;
//! - for leaf elements, trim leading and trailing XML whitespace from their single CDATA payload,
//!   and treat whitespace-only text as empty; the full live comparison found this export
//!   indentation around leaf values throughout configuration rows, scripts, property values, and
//!   JSON content, while interior bytes (including service-script indentation) remained semantic.
//!
//! No generated id, default attribute, or other exception is included without a live/committed
//! observation. Apart from the rules above, CDATA bytes are never decoded, re-indented, or joined.

use super::scan::{self, Kind, ScanError, Token};
use sha2::{Digest, Sha256};
use std::fmt;

/// The normalisation version every hash carries; a baseline from another one says nothing.
pub(crate) const HASH_VERSION: &str = "v5";
const LIVE_ONLY_ELEMENTS: [&[u8]; 9] = [
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
const NAME_KEYED_CONTAINERS: [&[u8]; 6] = [
    b"FieldDefinitions",
    b"ParameterDefinitions",
    b"ServiceDefinitions",
    b"ServiceImplementations",
    b"ConfigurationTables",
    b"ConfigurationTableDefinitions",
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

#[derive(Debug)]
pub enum NormaliseError {
    Xml(ScanError),
    DeclaredEncoding(String),
    Malformed(String),
    EntityReference(String),
}

impl fmt::Display for NormaliseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NormaliseError::Xml(error) => write!(f, "{error}"),
            NormaliseError::DeclaredEncoding(name) => {
                write!(
                    f,
                    "{name} encoding is not supported; entity XML must be UTF-8"
                )
            }
            NormaliseError::Malformed(why) => write!(f, "malformed entity XML: {why}"),
            NormaliseError::EntityReference(reference) => {
                write!(f, "unsupported XML entity reference &{reference};")
            }
        }
    }
}

impl std::error::Error for NormaliseError {}

impl From<ScanError> for NormaliseError {
    fn from(value: ScanError) -> Self {
        NormaliseError::Xml(value)
    }
}

/// A parsed element. Crate-visible so `adopt` compares the same tree the hash is built from,
/// rather than a second parser that could read a document differently.
#[derive(Clone, Debug)]
pub(crate) struct Element {
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
pub fn differ_only_in_permissions(left: &[u8], right: &[u8]) -> bool {
    match (canonical(left, false), canonical(right, false)) {
        (Ok(left_rest), Ok(right_rest)) => {
            left_rest == right_rest && normalise(left).ok() != normalise(right).ok()
        }
        _ => false,
    }
}

fn canonical(src: &[u8], with_permissions: bool) -> Result<Vec<u8>, NormaliseError> {
    let tokens = scan::tokenize(src)?;
    reject_non_utf8_declaration(src, &tokens)?;
    let mut entity = unwrap_entity(parse(src, &tokens)?)?;
    strip_live_only(&mut entity);
    normalise_leaf_content(&mut entity);
    normalise_special_payloads(&mut entity);
    clean_indentation(&mut entity);
    sort_name_keyed_containers(&mut entity);
    if with_permissions {
        normalise_permissions(&mut entity);
    } else {
        entity.children.retain(|child| {
            !matches!(child, Node::Element(element)
                if PERMISSION_BLOCKS.contains(&element.name.as_slice()))
        });
    }
    let mut out = b"twaco-entity-normalise-v5\0".to_vec();
    write_element(&entity, &mut out);
    Ok(out)
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
pub fn hash(src: &[u8]) -> Result<String, NormaliseError> {
    let canonical = normalise(src)?;
    Ok(format!("{HASH_VERSION}:{}", hex(&sha256(&canonical))))
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
    element.children = vec![Node::Cdata(trim_xml_whitespace(&payload).to_vec())];
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
    let mut payload = None;
    for child in &element.children {
        match child {
            Node::Text(text) if xml_whitespace(text) => {}
            Node::Cdata(cdata) if payload.is_none() => payload = Some(cdata.clone()),
            _ => return None,
        }
    }
    payload
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

fn sort_name_keyed_containers(element: &mut Element) {
    for child in &mut element.children {
        if let Node::Element(child) = child {
            sort_name_keyed_containers(child);
        }
    }
    if !NAME_KEYED_CONTAINERS.contains(&element.name.as_slice()) {
        return;
    }

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

fn canonical_element_bytes(element: &Element) -> Vec<u8> {
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
    fn mashup_json_layout_is_ignored_but_values_and_key_order_are_not() {
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
        assert_ne!(hash(compact).unwrap(), hash(reordered).unwrap());
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
    fn hash_and_framing_are_version_four() {
        assert!(hash(b"<Thing/>").unwrap().starts_with("v5:"));
        assert!(normalise(b"<Thing/>")
            .unwrap()
            .starts_with(b"twaco-entity-normalise-v5\0"));
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
