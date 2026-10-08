//! A mashup's content as editable files.
//!
//! A mashup is a large JSON document buried in one CDATA section: widgets, bindings, styles and
//! the page's own CSS, all on however many lines the exporter felt like. It extracts to
//! `mashup/content.json` and `mashup/custom.css`, which is what lets a diff show that one
//! binding changed rather than that the mashup changed.
//!
//! **Key order is preserved.** The JSON is re-serialised on the way back, so a sorted map would
//! reorder every object in every mashup on first sync — 64 files at once in the reference
//! project, with nothing to show for it.

use super::sync;

/// A mashup's two sidecar files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assets {
    /// The whole content document, formatted.
    pub content: String,
    /// `CustomMashupCss`, newline-terminated. Empty when the mashup has none.
    pub css: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MashupError {
    #[error("{0}")]
    Scan(super::scan::ScanError),
    #[error("no <mashupContent> section")]
    NoContent,
    /// The payload is not JSON. The runtime shows an empty page rather than an error.
    #[error("mashupContent will not parse: {0}")]
    NotJson(String),
    #[error("{0}")]
    Malformed(String),
}

/// The content payload of a mashup document, dedented as extraction sees it.
fn payload(src: &[u8]) -> Result<(String, super::scan::Span), MashupError> {
    let tokens = super::scan::tokenize(src).map_err(MashupError::Scan)?;
    let content = tokens
        .iter()
        .position(|t| t.kind == super::scan::Kind::Start && t.name.of(src) == b"mashupContent")
        .ok_or(MashupError::NoContent)?;
    let end = super::scan::element_end_in(&tokens, src, content).ok_or(MashupError::NoContent)?;

    let cdata: Vec<usize> = (content + 1..end)
        .filter(|&i| tokens[i].kind == super::scan::Kind::Cdata)
        .collect();
    let (Some(&first), Some(&last)) = (cdata.first(), cdata.last()) else {
        return Err(MashupError::NoContent);
    };
    let joined: String = cdata
        .iter()
        .map(|&i| String::from_utf8_lossy(tokens[i].inner.of(src)).into_owned())
        .collect();
    let span = super::scan::Span::new(tokens[first].span.start, tokens[last].span.end);
    Ok((super::sidecar::dedent(&joined), span))
}

/// Split a mashup document into its sidecar files.
pub fn extract(src: &[u8]) -> Result<Assets, MashupError> {
    let (text, _) = payload(src)?;
    assets_from_payload(&text)
}

/// The sidecar files for a mashup's content JSON, wherever the text came from: an entity file
/// here, or a designer's export being adopted. One rendering, so the two cannot drift apart.
pub fn assets_from_payload(text: &str) -> Result<Assets, MashupError> {
    let value: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|e| MashupError::NotJson(e.to_string()))?;

    let css = value
        .get("CustomMashupCss")
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default();

    Ok(Assets {
        content: to_sidecar(&value),
        css: format!("{css}\n"),
    })
}

/// The `content.json` text: two-space indent, key order intact, one trailing newline.
pub fn to_sidecar(value: &serde_json::Value) -> String {
    format!("{}\n", pretty(value))
}

/// Write the sidecars back into a mashup document.
///
/// `custom.css` wins over whatever `content.json` holds for `CustomMashupCss`, so editing the
/// stylesheet in the file named after it is the thing that takes effect.
pub fn sync(src: &[u8], assets: &Assets) -> Result<(Vec<u8>, Vec<String>), MashupError> {
    let mut wanted: serde_json::Value = serde_json::from_str(assets.content.trim())
        .map_err(|e| MashupError::Malformed(format!("content.json will not parse: {e}")))?;
    if !wanted.is_object() {
        return Err(MashupError::Malformed(
            "content.json is not an object".into(),
        ));
    }
    let css = without_terminator(&assets.css);
    if let Some(object) = wanted.as_object_mut() {
        match object.get("CustomMashupCss") {
            // A mashup that has never had a stylesheet, and still has nothing in one: the key
            // stays absent rather than appearing as an empty string.
            None if css.is_empty() => {}
            // Null is a state the platform writes, and it is not the same value as "". An
            // empty custom.css is exactly what a null extracts to, so it stays null.
            Some(serde_json::Value::Null) if css.is_empty() => {}
            _ => {
                object.insert(
                    "CustomMashupCss".to_string(),
                    serde_json::Value::String(css.clone()),
                );
            }
        }
    }

    let (text, span) = payload(src)?;
    let current: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|e| MashupError::NotJson(e.to_string()))?;

    let mut changes = Vec::new();
    let current_css = current
        .get("CustomMashupCss")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current_css != css {
        changes.push("custom.css".to_string());
    }
    if current != wanted {
        changes.push("content.json".to_string());
    }
    if changes.is_empty() {
        return Ok((src.to_vec(), changes));
    }

    // The write replaces every CDATA section of the payload as one; markup between them would go.
    let tokens = super::scan::tokenize(src).map_err(MashupError::Scan)?;
    if !super::scan::only_cdata_and_text(&tokens, src, span, true) {
        return Err(MashupError::Malformed(
            "<mashupContent> holds markup between its CDATA sections, which a sync would drop; take it out of the entity file".to_string(),
        ));
    }

    // Re-indented to sit inside its `<code>`-style element the way the document already does,
    // and written as one CDATA section, split if the content contains the terminator.
    let existing = String::from_utf8_lossy(span.of(src));
    let inner = inner_of(&existing);
    let newline = sync::newline_of(&existing);
    let rendered = sync::render_payload(inner, &pretty(&wanted), newline, true);
    let replacement = super::scan::render_cdata(rendered.as_bytes());

    let out = super::splice::splice(src, &[super::splice::Edit::new(span, replacement)])
        .map_err(|e| MashupError::Malformed(e.to_string()))?;
    Ok((out, changes))
}

/// A stylesheet without the newline that `extract` added to end the file.
///
/// One newline, not every trailing one. Stripping them all made a stylesheet that genuinely
/// ends in a blank line come back one character shorter than it went out, so a mashup nobody
/// had touched reported drift.
fn without_terminator(css: &str) -> String {
    css.strip_suffix("\r\n")
        .or_else(|| css.strip_suffix('\n'))
        .unwrap_or(css)
        .to_string()
}

/// The text between the CDATA markers of a region that may hold several sections.
fn inner_of(region: &str) -> &str {
    region
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(region)
}

/// Serialise the way `json.dumps(indent=2, ensure_ascii=False)` does.
fn pretty(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_value(value, 0, &mut out);
    out
}

fn write_value(value: &serde_json::Value, depth: usize, out: &mut String) {
    match value {
        serde_json::Value::Object(map) if map.is_empty() => out.push_str("{}"),
        serde_json::Value::Object(map) => {
            out.push_str("{\n");
            let last = map.len() - 1;
            for (index, (key, child)) in map.iter().enumerate() {
                indent(depth + 1, out);
                out.push_str(&string_literal(key));
                out.push_str(": ");
                write_value(child, depth + 1, out);
                out.push_str(if index == last { "\n" } else { ",\n" });
            }
            indent(depth, out);
            out.push('}');
        }
        serde_json::Value::Array(items) if items.is_empty() => out.push_str("[]"),
        serde_json::Value::Array(items) => {
            out.push_str("[\n");
            let last = items.len() - 1;
            for (index, child) in items.iter().enumerate() {
                indent(depth + 1, out);
                write_value(child, depth + 1, out);
                out.push_str(if index == last { "\n" } else { ",\n" });
            }
            indent(depth, out);
            out.push(']');
        }
        serde_json::Value::String(text) => out.push_str(&string_literal(text)),
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(true) => out.push_str("true"),
        serde_json::Value::Bool(false) => out.push_str("false"),
        serde_json::Value::Number(number) => out.push_str(&number.to_string()),
    }
}

fn indent(depth: usize, out: &mut String) {
    for _ in 0..depth * 2 {
        out.push(' ');
    }
}

/// A JSON string, escaped as `ensure_ascii=False` leaves it: non-ASCII stays as itself.
fn string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn document(content: &str) -> Vec<u8> {
        format!(
            "<Entities><Mashups><Mashup name=\"M\" projectName=\"P\">\n            \
             <mashupContent>\n                <![CDATA[\n{content}\n                ]]>\
             </mashupContent>\n        </Mashup></Mashups></Entities>"
        )
        .into_bytes()
    }

    #[test]
    fn the_css_comes_out_in_its_own_file() {
        let src = document("{\"UI\": {}, \"CustomMashupCss\": \".a { color: red; }\"}");
        let assets = extract(&src).unwrap();
        assert_eq!(assets.css, ".a { color: red; }\n");
        assert!(assets.content.contains("\"CustomMashupCss\""));
    }

    #[test]
    fn a_mashup_with_no_css_gets_an_empty_file() {
        let src = document("{\"UI\": {}}");
        assert_eq!(extract(&src).unwrap().css, "\n");
    }

    #[test]
    fn key_order_survives() {
        // A sorted map would reorder every object in every mashup on the first sync.
        let src = document("{\"zebra\": 1, \"apple\": 2, \"middle\": 3}");
        let content = extract(&src).unwrap().content;
        let zebra = content.find("zebra").unwrap();
        let apple = content.find("apple").unwrap();
        assert!(zebra < apple, "keys were reordered:\n{content}");
    }

    #[test]
    fn markup_between_content_sections_is_refused_rather_than_dropped() {
        let src = b"<Entities><Mashups><Mashup name=\"M\" projectName=\"P\"><mashupContent>                    <![CDATA[{\"UI\": ]]><!-- note --><![CDATA[{}}]]></mashupContent>                    </Mashup></Mashups></Entities>"
            .to_vec();
        let mut assets = extract(&src).unwrap();
        assets.css = ".a { color: red; }
"
        .to_string();
        let error = sync(&src, &assets).unwrap_err().to_string();
        assert!(
            error.contains("markup between its CDATA sections"),
            "{error}"
        );
    }

    #[test]
    fn syncing_unchanged_assets_changes_nothing() {
        let src = document("{\n  \"UI\": {},\n  \"CustomMashupCss\": \"\"\n}");
        let assets = extract(&src).unwrap();
        let (out, changes) = sync(&src, &assets).unwrap();
        assert!(changes.is_empty(), "got {changes:?}");
        assert_eq!(out, src, "a no-op sync must be the identity");
    }

    #[test]
    fn editing_the_stylesheet_reaches_the_document() {
        let src = document("{\n  \"UI\": {},\n  \"CustomMashupCss\": \"\"\n}");
        let mut assets = extract(&src).unwrap();
        assets.css = ".b { color: blue; }\n".to_string();
        let (out, changes) = sync(&src, &assets).unwrap();
        assert_eq!(changes, vec!["custom.css", "content.json"]);
        assert_eq!(extract(&out).unwrap().css, ".b { color: blue; }\n");
    }

    #[test]
    fn the_stylesheet_file_wins_over_the_content_copy() {
        let src = document("{\n  \"UI\": {},\n  \"CustomMashupCss\": \"old\"\n}");
        let mut assets = extract(&src).unwrap();
        // content.json still says "old"; the css file is what a person edited.
        assets.css = "new\n".to_string();
        let (out, _) = sync(&src, &assets).unwrap();
        assert_eq!(extract(&out).unwrap().css, "new\n");
    }

    #[test]
    fn a_payload_that_only_differs_in_layout_is_left_alone() {
        // Changes are judged on the parsed document, not its text, so a compact payload is not
        // rewritten merely to reformat it. This avoids rewriting every mashup solely for layout.
        let src = document("{\"UI\":{},\"CustomMashupCss\":\"\"}");
        let (out, changes) = sync(&src, &extract(&src).unwrap()).unwrap();
        assert!(changes.is_empty(), "got {changes:?}");
        assert_eq!(out, src);
    }

    #[test]
    fn a_real_change_settles_after_one_pass() {
        let src = document("{\n  \"UI\": {},\n  \"CustomMashupCss\": \"\"\n}");
        let mut assets = extract(&src).unwrap();
        assets.css = "body { margin: 0; }\n".to_string();
        let (once, first) = sync(&src, &assets).unwrap();
        assert!(!first.is_empty());
        // The same assets a second time, not what extract makes of the first output: re-reading
        // lets a wrong-but-stable first write agree with itself and pass.
        let (twice, second) = sync(&once, &assets).unwrap();
        assert_eq!(once, twice);
        assert!(second.is_empty(), "the second pass must change nothing");
        assert_eq!(extract(&once).unwrap().css, assets.css);
    }

    #[test]
    fn a_stylesheet_ending_in_a_blank_line_survives_the_round_trip() {
        // extract adds one newline to end the file; sync takes one back. Taking every trailing
        // newline shortened a stylesheet that genuinely ends in a blank line, and the mashup
        // reported drift with nobody having touched it.
        let src = document("{\n  \"UI\": {},\n  \"CustomMashupCss\": \"body {}\\n\"\n}");
        let assets = extract(&src).unwrap();
        assert_eq!(assets.css, "body {}\n\n");
        let (out, changes) = sync(&src, &assets).unwrap();
        assert!(changes.is_empty(), "got {changes:?}");
        assert_eq!(out, src);
    }

    #[test]
    fn a_null_stylesheet_stays_null() {
        // null and "" are different values in the payload the runtime reads, and extract maps
        // both to an empty file, so an empty file must not turn one into the other.
        let src = document("{\n  \"UI\": {},\n  \"CustomMashupCss\": null\n}");
        let (out, changes) = sync(&src, &extract(&src).unwrap()).unwrap();
        assert!(changes.is_empty(), "got {changes:?}");
        assert_eq!(out, src);
    }

    #[test]
    fn a_mashup_that_never_had_a_stylesheet_does_not_gain_an_empty_one() {
        let src = document("{\n  \"UI\": {}\n}");
        let (out, changes) = sync(&src, &extract(&src).unwrap()).unwrap();
        assert!(changes.is_empty(), "got {changes:?}");
        assert_eq!(out, src);
    }

    #[test]
    fn a_payload_that_is_not_json_is_refused() {
        let src = document("{\"UI\":}");
        assert!(matches!(extract(&src), Err(MashupError::NotJson(_))));
    }

    #[test]
    fn the_json_uses_the_canonical_shape() {
        let value: serde_json::Value =
            serde_json::from_str("{\"a\":[1,2],\"b\":{},\"c\":[],\"d\":\"x\"}").unwrap();
        assert_eq!(
            pretty(&value),
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {},\n  \"c\": [],\n  \"d\": \"x\"\n}"
        );
    }
}
