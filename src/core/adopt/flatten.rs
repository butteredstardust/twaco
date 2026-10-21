//! An element as ElementTree sees it, flattened to paths and values the comparison can diff.

use super::{NOISE_ATTRS, NOISE_TAGS, ROW_KEY_TAGS};
use crate::core::normalise::{Element, Node};
use serde_json::Value;
use std::collections::BTreeMap;
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
pub(crate) fn glob_matches(pattern: &str, text: &str) -> bool {
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
