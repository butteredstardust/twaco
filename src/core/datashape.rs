//! DataShape fields as an editable file.
//!
//! A DataShape's `FieldDefinitions` are the shape of its rows, and for the persisted shapes they
//! are the database's column list too. They belong in a file a person can read and diff rather
//! than in a wall of attributes, so they extract to `fields.json` and sync back.
//!
//! The JSON is written to match the toolchain this replaces exactly, because every committed
//! sidecar was produced by it and a formatting difference would rewrite all of them at once.

use super::scan::{self, Kind, ScanError, Token};
use std::collections::BTreeMap;

/// One field of a DataShape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub base_type: String,
    /// Kept as written. ThingWorx emits it as an attribute and the sidecar carries the string,
    /// so a leading zero or an empty value survives a round trip.
    pub ordinal: String,
    pub description: String,
    /// `aspect.*` attributes, without the prefix. `true`/`false` become booleans; the rest stay
    /// strings, which is what the reference does and what the committed sidecars hold.
    pub aspects: BTreeMap<String, Aspect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aspect {
    Bool(bool),
    Text(String),
}

impl Aspect {
    fn from_attribute(value: &str) -> Aspect {
        match value {
            "true" => Aspect::Bool(true),
            "false" => Aspect::Bool(false),
            other => Aspect::Text(other.to_string()),
        }
    }

    fn to_attribute(&self) -> String {
        match self {
            Aspect::Bool(true) => "true".to_string(),
            Aspect::Bool(false) => "false".to_string(),
            Aspect::Text(text) => text.clone(),
        }
    }

    fn to_json(&self) -> String {
        match self {
            Aspect::Bool(value) => value.to_string(),
            Aspect::Text(text) => json_string(text),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FieldError {
    #[error("{0}")]
    Scan(ScanError),
    #[error("not a DataShape export")]
    NotADataShape,
    /// More than one `FieldDefinitions` section, so there is no single place to write.
    #[error("expected one <FieldDefinitions> section, found {count}")]
    Ambiguous { count: usize },
    #[error("{why}")]
    Malformed { why: String },
}

impl From<ScanError> for FieldError {
    fn from(e: ScanError) -> Self {
        FieldError::Scan(e)
    }
}

/// Read a DataShape's fields, in the order the document declares them.
pub fn extract(src: &[u8]) -> Result<Vec<Field>, FieldError> {
    let tokens = scan::tokenize(src)?;
    let Some(section) = sole_section(&tokens, src)? else {
        // A DataShape with no fields is legitimate and extracts to an empty list.
        return Ok(Vec::new());
    };
    if tokens[section].kind == Kind::Empty {
        return Ok(Vec::new());
    }
    let end = scan::element_end_in(&tokens, src, section).ok_or_else(|| FieldError::Malformed {
        why: "<FieldDefinitions> is not closed".into(),
    })?;

    let mut fields = Vec::new();
    let mut index = section + 1;
    while index < end {
        if matches!(tokens[index].kind, Kind::Start | Kind::Empty)
            && tokens[index].name.of(src) == b"FieldDefinition"
        {
            fields.push(read_field(&tokens[index], src)?);
            index = scan::element_end_in(&tokens, src, index).map_or(end, |e| e + 1);
        } else {
            index += 1;
        }
    }
    Ok(fields)
}

fn read_field(tag: &Token, src: &[u8]) -> Result<Field, FieldError> {
    let text = |key: &str| -> Result<String, FieldError> {
        Ok(scan::attribute(src, tag, key)
            .map_err(FieldError::Scan)?
            .map(|s| scan::decode_entities(&String::from_utf8_lossy(s.of(src))))
            .unwrap_or_default())
    };
    Ok(Field {
        name: text("name")?,
        base_type: text("baseType")?,
        ordinal: text("ordinal")?,
        description: text("description")?,
        aspects: aspects_of(tag, src)?,
    })
}

/// Every `aspect.*` attribute on one tag.
fn aspects_of(tag: &Token, src: &[u8]) -> Result<BTreeMap<String, Aspect>, FieldError> {
    let mut out = BTreeMap::new();
    for key in attribute_names(tag, src) {
        if let Some(aspect) = key.strip_prefix("aspect.") {
            let value = scan::attribute(src, tag, &key)
                .map_err(FieldError::Scan)?
                .map(|s| scan::decode_entities(&String::from_utf8_lossy(s.of(src))))
                .unwrap_or_default();
            out.insert(aspect.to_string(), Aspect::from_attribute(&value));
        }
    }
    Ok(out)
}

/// The attribute names on one tag, in document order.
///
/// The scanner can fetch a value by name but has no way to ask what names are present, and an
/// aspect can be called anything.
fn attribute_names(tag: &Token, src: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let limit = tag.span.end.min(src.len());
    let mut i = tag.name.end.min(limit);
    while i < limit {
        while i < limit && src[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= limit || src[i] == b'>' || src[i] == b'/' {
            break;
        }
        let start = i;
        while i < limit
            && src[i] != b'='
            && !src[i].is_ascii_whitespace()
            && src[i] != b'>'
            && src[i] != b'/'
        {
            i += 1;
        }
        if start == i {
            break;
        }
        names.push(String::from_utf8_lossy(&src[start..i]).into_owned());
        while i < limit && src[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < limit && src[i] == b'=' {
            i += 1;
            while i < limit && src[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < limit && (src[i] == b'"' || src[i] == b'\'') {
                let quote = src[i];
                i += 1;
                while i < limit && src[i] != quote {
                    i += 1;
                }
                i += 1;
            }
        }
    }
    names
}

/// The `FieldDefinitions` section belonging to the DataShape itself.
///
/// A direct child of the `<DataShape>` element, not any descendant: a `FieldDefinitions` nested
/// inside some other element is not this DataShape's field list, and overwriting it would
/// corrupt the document while reporting success.
fn sole_section(tokens: &[Token], src: &[u8]) -> Result<Option<usize>, FieldError> {
    let wrapper = tokens
        .iter()
        .position(|t| t.kind == Kind::Start && t.name.of(src) == b"Entities")
        .ok_or(FieldError::NotADataShape)?;
    let wrapper_end =
        scan::element_end_in(tokens, src, wrapper).ok_or(FieldError::NotADataShape)?;

    // <Entities> -> <DataShapes> -> <DataShape>
    let collection = (wrapper + 1..wrapper_end)
        .find(|&i| tokens[i].kind == Kind::Start && tokens[i].name.of(src) == b"DataShapes")
        .ok_or(FieldError::NotADataShape)?;
    let shape = scan::child_tags(tokens, src, "DataShape", collection)
        .first()
        .copied()
        .ok_or(FieldError::NotADataShape)?;

    let found = scan::child_tags(tokens, src, "FieldDefinitions", shape);
    match found.len() {
        0 => Ok(None),
        // `<FieldDefinitions/>` is an empty section, not an absent one: it is still the place a
        // first field is written, and returning None made that impossible.
        1 => Ok(Some(found[0])),
        // A DataShape has one field list. More than one means this is not the document we
        // think it is, and writing to a guess would corrupt it.
        n => Err(FieldError::Ambiguous { count: n }),
    }
}

/// The `fields.json` text for a set of fields.
///
/// Two spaces per level, keys in a fixed order, aspects sorted, and a trailing newline: exactly
/// what `json.dumps(indent=2, ensure_ascii=False)` produces, because 47 committed sidecars were
/// produced by it.
pub fn to_sidecar(fields: &[Field]) -> String {
    if fields.is_empty() {
        return "[]\n".to_string();
    }
    let mut out = String::from("[\n");
    for (index, field) in fields.iter().enumerate() {
        out.push_str("  {\n");
        out.push_str(&format!("    \"name\": {},\n", json_string(&field.name)));
        out.push_str(&format!(
            "    \"baseType\": {},\n",
            json_string(&field.base_type)
        ));
        out.push_str(&format!(
            "    \"ordinal\": {},\n",
            json_string(&field.ordinal)
        ));
        out.push_str(&format!(
            "    \"description\": {},\n",
            json_string(&field.description)
        ));
        if field.aspects.is_empty() {
            out.push_str("    \"aspects\": {}\n");
        } else {
            out.push_str("    \"aspects\": {\n");
            let last = field.aspects.len() - 1;
            for (position, (key, value)) in field.aspects.iter().enumerate() {
                let comma = if position == last { "" } else { "," };
                out.push_str(&format!(
                    "      {}: {}{comma}\n",
                    json_string(key),
                    value.to_json()
                ));
            }
            out.push_str("    }\n");
        }
        out.push_str(if index == fields.len() - 1 {
            "  }\n"
        } else {
            "  },\n"
        });
    }
    out.push_str("]\n");
    out
}

/// Parse a `fields.json` sidecar.
pub fn from_sidecar(text: &str) -> Result<Vec<Field>, FieldError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| FieldError::Malformed {
            why: format!("fields.json is not valid JSON: {e}"),
        })?;
    let array = value.as_array().ok_or_else(|| FieldError::Malformed {
        why: "fields.json is not a list".into(),
    })?;

    let mut fields = Vec::new();
    for entry in array {
        let object = entry.as_object().ok_or_else(|| FieldError::Malformed {
            why: "a field is not an object".into(),
        })?;
        // Strings must be strings. Defaulting a wrong type to "" would quietly rewrite a
        // field's baseType to nothing, which for a persisted shape is a dropped column.
        let text_of = |key: &str| -> Result<String, FieldError> {
            match object.get(key) {
                None => Ok(String::new()),
                Some(serde_json::Value::String(value)) => Ok(value.clone()),
                Some(other) => Err(FieldError::Malformed {
                    why: format!("{key} must be a string, found {other}"),
                }),
            }
        };
        let mut aspects = BTreeMap::new();
        match object.get("aspects") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Object(map)) => {
                for (key, value) in map {
                    let aspect = match value {
                        serde_json::Value::Bool(b) => Aspect::Bool(*b),
                        serde_json::Value::String(s) => Aspect::Text(s.clone()),
                        other => {
                            return Err(FieldError::Malformed {
                                why: format!(
                                    "aspect {key} must be a string or a boolean, found {other}"
                                ),
                            })
                        }
                    };
                    aspects.insert(key.clone(), aspect);
                }
            }
            Some(other) => {
                return Err(FieldError::Malformed {
                    why: format!("aspects must be an object, found {other}"),
                })
            }
        }
        let name = text_of("name")?;
        if name.is_empty() {
            return Err(FieldError::Malformed {
                why: "a field has no name".into(),
            });
        }
        if fields.iter().any(|f: &Field| f.name == name) {
            return Err(FieldError::Malformed {
                why: format!("two fields are named {name}"),
            });
        }
        fields.push(Field {
            name,
            base_type: text_of("baseType")?,
            ordinal: text_of("ordinal")?,
            description: text_of("description")?,
            aspects,
        });
    }
    Ok(fields)
}

/// Write fields back into a DataShape document.
///
/// Only the `FieldDefinitions` section is replaced; every other byte is copied through. Adding
/// or removing a field is refused unless asked for, because a DataShape's field list is a
/// database column list for the persisted shapes and losing one silently is a migration nobody
/// asked for.
pub fn sync(
    src: &[u8],
    desired: &[Field],
    allow_add_remove: bool,
) -> Result<(Vec<u8>, Vec<String>), FieldError> {
    let current = extract(src)?;
    if !allow_add_remove {
        // Counted rather than compared as sets: two fields named A in the XML against one in
        // the sidecar is a removal, and a set sees no difference at all.
        let mut tally: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for field in &current {
            tally.entry(field.name.as_str()).or_default().0 += 1;
        }
        for field in desired {
            tally.entry(field.name.as_str()).or_default().1 += 1;
        }
        let added: Vec<&str> = tally
            .iter()
            .filter(|(_, (before, after))| after > before)
            .map(|(n, _)| *n)
            .collect();
        let removed: Vec<&str> = tally
            .iter()
            .filter(|(_, (before, after))| after < before)
            .map(|(n, _)| *n)
            .collect();
        if !added.is_empty() || !removed.is_empty() {
            return Err(FieldError::Malformed {
                why: format!(
                    "field set mismatch: sidecar-only {added:?}, xml-only {removed:?}; \
                     say --allow-add-remove to mean it"
                ),
            });
        }
    }

    let tokens = scan::tokenize(src)?;
    let Some(section) = sole_section(&tokens, src)? else {
        return Err(FieldError::Malformed {
            why: "this DataShape has no <FieldDefinitions> section to write into".into(),
        });
    };
    // Layout is not content, as for scripts. Fields that already say
    // what the sidecar says are left byte for byte, whatever their layout. Rendering them anyway
    // could replace an unchanged section's existing layout while reporting no field changes.
    // After the structural check, so a document with no section of its own is still refused.
    if current == desired {
        return Ok((src.to_vec(), Vec::new()));
    }
    // `<FieldDefinitions/>` with nothing to write stays as it is. Expanding it into a pair of
    // tags would make a sync that changes nothing change the document.
    if tokens[section].kind == Kind::Empty && desired.is_empty() {
        return Ok((src.to_vec(), Vec::new()));
    }
    let section_end = if tokens[section].kind == Kind::Empty {
        section
    } else {
        scan::element_end_in(&tokens, src, section).ok_or_else(|| FieldError::Malformed {
            why: "<FieldDefinitions> is not closed".into(),
        })?
    };

    // The rendered section carries its own indentation, so the document's is replaced along
    // with it -- but only when what precedes the tag on its line really is indentation. On
    // `<DataShape name="D"><FieldDefinitions>` the prefix is another element, and swallowing it
    // produced a document with the DataShape's opening tag written twice.
    let (indent, span_start) = match own_line_indent(src, tokens[section].span.start) {
        Some(indent) => {
            let start = tokens[section].span.start - indent.len();
            (indent, start)
        }
        None => (String::new(), tokens[section].span.start),
    };
    let span = scan::Span::new(span_start, tokens[section_end].span.end);
    let newline = super::sync::newline_of(&String::from_utf8_lossy(src));
    let rendered = render_section(desired, &indent, newline);

    let changes = summarise(&current, desired);
    if rendered.as_bytes() == span.of(src) {
        return Ok((src.to_vec(), changes));
    }
    let out = super::splice::splice(
        src,
        &[super::splice::Edit::new(span, rendered.into_bytes())],
    )
    .map_err(|e| FieldError::Malformed { why: e.to_string() })?;
    Ok((out, changes))
}

/// The indentation before an offset, or `None` when the tag does not start its own line.
fn own_line_indent(src: &[u8], at: usize) -> Option<String> {
    let line_start = src[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let prefix = &src[line_start..at];
    if prefix.iter().all(|b| *b == b' ' || *b == b'\t') {
        Some(String::from_utf8_lossy(prefix).into_owned())
    } else {
        None
    }
}

/// Render a whole `FieldDefinitions` section in the canonical Composer-compatible layout.
fn render_section(fields: &[Field], indent: &str, newline: &str) -> String {
    let child = format!("{indent}    ");
    let attribute = format!("{child} ");
    let mut lines = vec![format!("{indent}<FieldDefinitions>")];
    for field in fields {
        // Aspects first and alphabetically, then the four fixed attributes alphabetically. The
        // stable order minimizes diffs against Composer exports.
        let mut attributes: Vec<(String, String)> = field
            .aspects
            .iter()
            .map(|(key, value)| (format!("aspect.{key}"), value.to_attribute()))
            .collect();
        attributes.push(("baseType".into(), field.base_type.clone()));
        attributes.push(("description".into(), field.description.clone()));
        attributes.push(("name".into(), field.name.clone()));
        attributes.push(("ordinal".into(), field.ordinal.clone()));

        lines.push(format!("{child}<FieldDefinition"));
        let last = attributes.len() - 1;
        for (index, (key, value)) in attributes.iter().enumerate() {
            let suffix = if index == last {
                "></FieldDefinition>"
            } else {
                ""
            };
            lines.push(format!(
                "{attribute}{key}=\"{}\"{suffix}",
                super::scan::escape_attribute(value)
            ));
        }
    }
    lines.push(format!("{indent}</FieldDefinitions>"));
    lines.join(newline)
}

/// What changed, for a person reading a sync report.
fn summarise(current: &[Field], desired: &[Field]) -> Vec<String> {
    let mut out = Vec::new();
    for field in desired {
        match current.iter().find(|f| f.name == field.name) {
            None => out.push(format!("added {}", field.name)),
            Some(before) if before != field => out.push(format!("changed {}", field.name)),
            Some(_) => {}
        }
    }
    for field in current {
        if !desired.iter().any(|f| f.name == field.name) {
            out.push(format!("removed {}", field.name));
        }
    }
    out
}

/// A JSON string literal, escaped as `json.dumps(ensure_ascii=False)` would.
fn json_string(value: &str) -> String {
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
            // Non-ASCII stays as itself: ensure_ascii=False, and the committed sidecars carry
            // degree signs and the like unescaped.
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

    #[test]
    fn unchanged_fields_keep_their_layout_even_when_it_is_not_ours() {
        // A supported export style: one FieldDefinition per line, tab-indented, CRLF.
        let src = "<Entities>\r\n\t<DataShapes>\r\n\t\t<DataShape name=\"D\">\r\n\t\t\t<FieldDefinitions>\r\n\t\t\t\t<FieldDefinition baseType=\"STRING\" description=\"d\" name=\"a\" ordinal=\"0\"/>\r\n\t\t\t</FieldDefinitions>\r\n\t\t</DataShape>\r\n\t</DataShapes>\r\n</Entities>\r\n";
        let fields = extract(src.as_bytes()).unwrap();
        let (out, changes) = sync(src.as_bytes(), &fields, false).unwrap();
        assert_eq!(
            out,
            src.as_bytes(),
            "a sync with nothing to change is the identity"
        );
        assert!(changes.is_empty());

        // A real change is still written.
        let mut edited = fields.clone();
        edited[0].description = "changed".to_string();
        let (out, changes) = sync(src.as_bytes(), &edited, false).unwrap();
        assert_ne!(out, src.as_bytes());
        assert!(!changes.is_empty());
        assert_eq!(extract(&out).unwrap()[0].description, "changed");
        // A CRLF document stays CRLF: a lone LF among CRLFs is the mixed file the line-endings
        // gate refuses.
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text.matches('\n').count(),
            text.matches("\r\n").count(),
            "mixed line endings: {text:?}"
        );
    }

    const SHAPE: &[u8] = br#"<Entities>
    <DataShapes>
        <DataShape name="My_DS" projectName="P">
            <FieldDefinitions>
                <FieldDefinition
                 aspect.isPrimaryKey="true"
                 baseType="STRING"
                 description="A name"
                 name="Name"
                 ordinal="1"></FieldDefinition>
                <FieldDefinition
                 aspect.isPrimaryKey="false"
                 baseType="NUMBER"
                 description=""
                 name="Count"
                 ordinal="2"></FieldDefinition>
            </FieldDefinitions>
        </DataShape>
    </DataShapes>
</Entities>"#;

    #[test]
    fn fields_come_back_in_document_order() {
        let fields = extract(SHAPE).unwrap();
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["Name", "Count"]);
        assert_eq!(fields[0].base_type, "STRING");
        assert_eq!(fields[0].ordinal, "1");
        assert_eq!(
            fields[0].aspects.get("isPrimaryKey"),
            Some(&Aspect::Bool(true))
        );
        assert_eq!(
            fields[1].aspects.get("isPrimaryKey"),
            Some(&Aspect::Bool(false))
        );
    }

    #[test]
    fn the_sidecar_round_trips() {
        let fields = extract(SHAPE).unwrap();
        let text = to_sidecar(&fields);
        assert_eq!(from_sidecar(&text).unwrap(), fields);
    }

    #[test]
    fn the_sidecar_is_shaped_the_way_the_reference_writes_it() {
        let fields = extract(SHAPE).unwrap();
        let text = to_sidecar(&fields);
        assert!(
            text.starts_with("[\n  {\n    \"name\": \"Name\",\n"),
            "got:\n{text}"
        );
        assert!(text.contains("    \"aspects\": {\n      \"isPrimaryKey\": true\n    }\n"));
        assert!(text.ends_with("]\n"));
    }

    #[test]
    fn a_field_with_no_aspects_writes_an_empty_object() {
        let field = Field {
            name: "N".into(),
            base_type: "STRING".into(),
            ordinal: "1".into(),
            description: String::new(),
            aspects: BTreeMap::new(),
        };
        assert!(to_sidecar(&[field]).contains("\"aspects\": {}\n"));
    }

    #[test]
    fn syncing_unchanged_fields_changes_nothing() {
        let fields = extract(SHAPE).unwrap();
        let (out, changes) = sync(SHAPE, &fields, false).unwrap();
        assert_eq!(out, SHAPE.to_vec(), "a no-op sync must be the identity");
        assert!(changes.is_empty());
    }

    #[test]
    fn an_edited_description_is_written_back() {
        let mut fields = extract(SHAPE).unwrap();
        fields[0].description = "Renamed".to_string();
        let (out, changes) = sync(SHAPE, &fields, false).unwrap();
        assert_eq!(changes, vec!["changed Name"]);
        assert_eq!(extract(&out).unwrap()[0].description, "Renamed");
    }

    #[test]
    fn adding_a_field_is_refused_unless_asked_for() {
        let mut fields = extract(SHAPE).unwrap();
        fields.push(Field {
            name: "Extra".into(),
            base_type: "STRING".into(),
            ordinal: "3".into(),
            description: String::new(),
            aspects: BTreeMap::new(),
        });
        assert!(
            sync(SHAPE, &fields, false).is_err(),
            "a column is not added by accident"
        );
        let (out, changes) = sync(SHAPE, &fields, true).unwrap();
        assert_eq!(changes, vec!["added Extra"]);
        assert_eq!(extract(&out).unwrap().len(), 3);
    }

    #[test]
    fn removing_a_field_is_refused_unless_asked_for() {
        let mut fields = extract(SHAPE).unwrap();
        fields.pop();
        assert!(sync(SHAPE, &fields, false).is_err());
        let (_, changes) = sync(SHAPE, &fields, true).unwrap();
        assert_eq!(changes, vec!["removed Count"]);
    }

    #[test]
    fn a_quote_in_a_description_survives_both_directions() {
        let mut fields = extract(SHAPE).unwrap();
        fields[0].description = "joined with \", \"".to_string();
        let (out, _) = sync(SHAPE, &fields, false).unwrap();
        assert!(String::from_utf8_lossy(&out).contains("&quot;, &quot;"));
        assert_eq!(extract(&out).unwrap()[0].description, "joined with \", \"");
        // And through the JSON, where a quote is escaped rather than entity-encoded.
        let text = to_sidecar(&extract(&out).unwrap());
        assert!(text.contains(r#"joined with \", \""#), "got: {text}");
        assert_eq!(
            from_sidecar(&text).unwrap()[0].description,
            "joined with \", \""
        );
    }

    #[test]
    fn an_inline_section_is_not_swallowed_by_its_own_line() {
        // Regression: everything before <FieldDefinitions> on the line was taken for
        // indentation, so the DataShape's opening tag was written out a second time.
        let src = br#"<Entities><DataShapes><DataShape name="D"><FieldDefinitions><FieldDefinition baseType="STRING" description="" name="A" ordinal="1"></FieldDefinition></FieldDefinitions></DataShape></DataShapes></Entities>"#;
        let fields = extract(src).unwrap();
        let (out, _) = sync(src, &fields, false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text.matches("<DataShape ").count(),
            1,
            "the DataShape tag was duplicated:\n{text}"
        );
        assert_eq!(extract(text.as_bytes()).unwrap(), fields);
    }

    #[test]
    fn an_empty_section_element_can_receive_a_first_field() {
        let src = br#"<Entities><DataShapes><DataShape name="D"><FieldDefinitions/></DataShape></DataShapes></Entities>"#;
        assert!(extract(src).unwrap().is_empty());
        // The identity case must work.
        let (same, _) = sync(src, &[], false).unwrap();
        assert_eq!(same, src.to_vec());

        let field = Field {
            name: "First".into(),
            base_type: "STRING".into(),
            ordinal: "1".into(),
            description: String::new(),
            aspects: BTreeMap::new(),
        };
        let (out, changes) = sync(src, &[field], true).unwrap();
        assert_eq!(changes, vec!["added First"]);
        assert_eq!(extract(&out).unwrap().len(), 1);
    }

    #[test]
    fn a_field_definitions_elsewhere_in_the_document_is_not_the_datashapes() {
        let src = br#"<Entities><DataShapes><DataShape name="D"><Metadata><FieldDefinitions><FieldDefinition baseType="STRING" description="" name="Nested" ordinal="1"></FieldDefinition></FieldDefinitions></Metadata></DataShape></DataShapes></Entities>"#;
        // The DataShape itself declares none, so there is nothing to extract and nothing to write.
        assert!(extract(src).unwrap().is_empty());
        assert!(
            sync(src, &[], false).is_err(),
            "a nested section is not ours to overwrite"
        );
    }

    #[test]
    fn a_duplicate_field_name_is_refused_by_the_sidecar_reader() {
        let text = r#"[{"name":"A","baseType":"STRING","ordinal":"1","description":"","aspects":{}},
                       {"name":"A","baseType":"NUMBER","ordinal":"2","description":"","aspects":{}}]"#;
        assert!(from_sidecar(text).is_err());
    }

    #[test]
    fn a_wrongly_typed_value_is_refused_rather_than_defaulted() {
        // A numeric ordinal must not become "": for a persisted shape that is a dropped column.
        let text =
            r#"[{"name":"A","baseType":"STRING","ordinal":3,"description":"","aspects":{}}]"#;
        assert!(from_sidecar(text).is_err());
        let bad_aspect = r#"[{"name":"A","baseType":"STRING","ordinal":"1","description":"","aspects":{"x":1}}]"#;
        assert!(from_sidecar(bad_aspect).is_err());
    }

    #[test]
    fn a_datashape_with_no_fields_extracts_to_an_empty_list() {
        let src =
            br#"<Entities><DataShapes><DataShape name="E"></DataShape></DataShapes></Entities>"#;
        assert!(extract(src).unwrap().is_empty());
        assert_eq!(to_sidecar(&[]), "[]\n");
    }

    #[test]
    fn a_non_boolean_aspect_stays_a_string() {
        let src = br#"<Entities><DataShapes><DataShape name="D"><FieldDefinitions>
            <FieldDefinition aspect.dataShape="Other_DS" baseType="INFOTABLE" description="" name="F" ordinal="1"></FieldDefinition>
        </FieldDefinitions></DataShape></DataShapes></Entities>"#;
        let fields = extract(src).unwrap();
        assert_eq!(
            fields[0].aspects.get("dataShape"),
            Some(&Aspect::Text("Other_DS".into()))
        );
        assert!(to_sidecar(&fields).contains("\"dataShape\": \"Other_DS\""));
    }
}
