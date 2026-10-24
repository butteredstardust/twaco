//! An element as ElementTree sees it, flattened to paths and values the comparison can diff.

use super::{NOISE_ATTRS, NOISE_TAGS, ROW_KEY_TAGS};
use crate::core::normalise::{Element, Node};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
// ---- ElementTree's view of a tree ---------------------------------------------------------

pub(super) fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub(super) fn child_elements(element: &Element) -> impl Iterator<Item = &Element> {
    element.children.iter().filter_map(|node| match node {
        Node::Element(child) => Some(child),
        _ => None,
    })
}

pub(super) fn attribute(element: &Element, name: &str) -> Option<String> {
    element
        .attributes
        .iter()
        .find(|(key, _)| key == name.as_bytes())
        .map(|(_, value)| text_of(value))
}

pub(super) fn is_named(element: &Element, name: &str) -> bool {
    element.name == name.as_bytes()
}

/// ElementTree's `.text`: character data before the first child element. CDATA is character
/// data; comments and processing instructions are not, and do not end the run.
pub(super) fn et_text(element: &Element) -> Option<String> {
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
pub(super) fn et_itertext(element: &Element, out: &mut Vec<String>) {
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
pub(super) fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// A value as a comparison should see it: whitespace runs collapsed, and JSON re-serialised with
/// sorted keys, so one object written two ways is one value.
pub(crate) fn canonical(value: &str) -> String {
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
///
/// Lists whose members have an identity are keyed by it, not by position, so a reorder is not a
/// change of every member: a widget by its `Properties.Id`, with the order of a container's
/// widgets kept as one entry of its own because it is the layout; a binding or an event by what
/// it connects, as the set it is. Their own `Id` is no identity: Composer mints new ones.
fn flatten_json(value: &Value, prefix: &str, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let path = format!("{prefix}.{key}");
                match (key.as_str(), item) {
                    ("Widgets", Value::Array(items)) => flatten_widgets(items, &path, out),
                    ("DataBindings" | "Events", Value::Array(items)) => {
                        flatten_connections(key, items, &path, out)
                    }
                    _ => flatten_json(item, &path, out),
                }
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

fn flatten_widgets(items: &[Value], prefix: &str, out: &mut BTreeMap<String, String>) {
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let id = item
            .pointer("/Properties/Id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let key = match id {
            Some(id) => numbered(&mut seen, id),
            None => numbered(&mut seen, &format!("#{index}")),
        };
        order.push(key.clone());
        flatten_json(item, &format!("{prefix}[{key}]"), out);
    }
    if !order.is_empty() {
        out.insert(format!("{prefix}#order"), order.join(", "));
    }
}

fn flatten_connections(
    kind: &str,
    items: &[Value],
    prefix: &str,
    out: &mut BTreeMap<String, String>,
) {
    let text = |item: &Value, field: &str| {
        item.get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut seen = BTreeSet::new();
    for item in items {
        let label = if kind == "Events" {
            format!(
                "{}.{}>{}.{}",
                text(item, "EventTriggerId"),
                text(item, "EventTriggerEvent"),
                text(item, "EventHandlerId"),
                text(item, "EventHandlerService")
            )
        } else {
            format!("{}>{}", text(item, "SourceId"), text(item, "TargetId"))
        };
        let mut content = item.clone();
        if let Some(map) = content.as_object_mut() {
            map.remove("Id");
        }
        let key = numbered(
            &mut seen,
            &format!("{label}~{:08x}", fnv1a(py_dumps(&content, true).as_bytes())),
        );
        flatten_json(item, &format!("{prefix}[{key}]"), out);
    }
}

/// `key`, or for a repeat the first `key#n` no sibling has taken, so every member keeps a path of
/// its own even when a name already looks like a numbered one.
fn numbered(seen: &mut BTreeSet<String>, key: &str) -> String {
    let mut claimed = key.to_string();
    let mut n = 1;
    while !seen.insert(claimed.clone()) {
        n += 1;
        claimed = format!("{key}#{n}");
    }
    claimed
}

/// A short, stable digest that tells two connections between the same members apart.
fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |hash: u32, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

/// What a child element is known by among its siblings. A principal is a user, group or
/// organization of that name, and a Group and an Organization may share one; a permissions block
/// is the resource it grants on.
fn identity(element: &Element) -> Option<String> {
    let name = attribute(element, "name").filter(|n| !n.is_empty());
    if is_named(element, "Principal") {
        return name.map(|name| match attribute(element, "type") {
            Some(kind) if !kind.is_empty() => format!("{kind}:{name}"),
            _ => name,
        });
    }
    if is_named(element, "Permissions") {
        if let Some(resource) = attribute(element, "resourceName").filter(|r| !r.is_empty()) {
            return Some(resource);
        }
    }
    name.or_else(|| row_key(element))
}

/// The value a `<Row>` identifies itself by, if it has one.
pub(crate) fn row_key(element: &Element) -> Option<String> {
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
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for child in child_elements(element) {
        let tag = text_of(&child.name);
        if NOISE_TAGS.contains(&tag.as_str()) {
            continue;
        }
        let step = match identity(child) {
            Some(key) => format!(
                "{tag}[{}]",
                numbered(seen.entry(tag.clone()).or_default(), &key)
            ),
            None if tally[child.name.as_slice()] > 1 => {
                let n = counts.entry(child.name.as_slice()).or_default();
                *n += 1;
                let n = n.to_string();
                format!(
                    "{tag}[{}]",
                    numbered(seen.entry(tag.clone()).or_default(), &n)
                )
            }
            None => tag,
        };
        flatten_into(child, &format!("{prefix}/{step}"), out);
    }
}

/// Composer mints a fresh id for a binding or an event handler whenever it rewrites a mashup:
/// `...Events[<member>].Id` or `...DataBindings[<member>].Id`. Counted, never listed.
pub(super) fn is_volatile_id(path: &str) -> bool {
    let Some(rest) = path.strip_suffix(".Id") else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(']') else {
        return false;
    };
    let Some(open) = rest.rfind('[') else {
        return false;
    };
    if rest[open + 1..].is_empty() {
        return false;
    }
    let head = &rest[..open];
    ["Events", "DataBindings"].iter().any(|word| {
        head.strip_suffix(word)
            .is_some_and(|before| before.is_empty() || before.ends_with('.'))
    })
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
