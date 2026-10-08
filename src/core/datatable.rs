//! A DataTable Thing's configuration as an editable file.
//!
//! A DataTable carries its shape and its indexes in configuration tables rather than in fields,
//! buried under two levels of `Rows`/`Row` and, for the accumulated shape, inside a `<json>`
//! wrapper inside a CDATA section. It extracts to `datatable.json`.
//!
//! **No committed sidecar was available when support for this kind was added.** The property
//! tested here is therefore round-trip identity against representative DataTable documents,
//! not a byte comparison against a previously generated sidecar.

use super::scan::{self, Kind, ScanError, Token};

/// What a DataTable declares about its storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    /// The DataShape its rows take.
    pub data_shape: String,
    /// The shape the platform accumulated, as raw JSON text.
    pub accumulated: String,
    pub indexes: Vec<Index>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    /// Comma-separated, as the platform writes it.
    pub field_names: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DataTableError {
    #[error("{0}")]
    Scan(ScanError),
    #[error("not a DataTable Thing")]
    NotADataTable,
    #[error("no <ConfigurationTable name=\"{0}\">")]
    MissingTable(&'static str),
    #[error("{0}")]
    Malformed(String),
}

/// The `Thing` element that is a DataTable, as the range of tokens inside it.
///
/// Everything else here searches inside that range rather than the whole document. A search
/// across the file finds the first `ConfigurationTable name="Settings"` anywhere, which in a
/// document holding more than one entity is a span belonging to something else entirely.
fn data_table_thing(tokens: &[Token], src: &[u8]) -> Option<(usize, usize)> {
    let start = tokens.iter().position(|t| {
        t.kind == Kind::Start
            && t.name.of(src) == b"Thing"
            && matches!(scan::attribute(src, t, "thingTemplate"), Ok(Some(v)) if v.of(src) == b"DataTable")
    })?;
    let end = scan::element_end_in(tokens, src, start)?;
    Some((start, end))
}

/// Whether a document is a DataTable Thing.
pub fn is_data_table(src: &[u8]) -> bool {
    let Ok(tokens) = scan::tokenize(src) else {
        return false;
    };
    data_table_thing(&tokens, src).is_some()
}

/// Read a DataTable's configuration.
pub fn extract(src: &[u8]) -> Result<Configuration, DataTableError> {
    let tokens = scan::tokenize(src).map_err(DataTableError::Scan)?;
    let thing = data_table_thing(&tokens, src).ok_or(DataTableError::NotADataTable)?;

    let settings = table_named(&tokens, src, thing, "Settings")
        .ok_or(DataTableError::MissingTable("Settings"))?;
    let row = first_row(&tokens, src, settings)
        .ok_or_else(|| DataTableError::Malformed("the Settings table has no Row".into()))?;

    let data_shape = child_text(&tokens, src, row, "dataShape").unwrap_or_default();
    let accumulated = child_text(&tokens, src, row, "accumulatedDataShape").unwrap_or_default();

    // The Indexes table is optional: a DataTable may have none.
    let indexes = match table_named(&tokens, src, thing, "Indexes") {
        Some(table) => rows_of(&tokens, src, table)
            .into_iter()
            .map(|row| Index {
                name: child_text(&tokens, src, row, "name").unwrap_or_default(),
                field_names: child_text(&tokens, src, row, "fieldNames").unwrap_or_default(),
            })
            .collect(),
        None => Vec::new(),
    };

    Ok(Configuration {
        data_shape,
        accumulated,
        indexes,
    })
}

/// The `datatable.json` text.
///
/// An accumulated shape that will not parse is refused rather than replaced with an empty
/// object. Writing `{}` here and then syncing it back would erase the real shape, and the
/// toolchain would have corrupted the document while reporting a routine extract.
pub fn to_sidecar(configuration: &Configuration) -> Result<String, DataTableError> {
    // Empty is legitimate: a DataTable that has never accumulated a shape. It stays null, so
    // the sidecar says "nothing here" rather than "an empty object here".
    let accumulated = if configuration.accumulated.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(configuration.accumulated.trim()).map_err(|e| {
            DataTableError::Malformed(format!("the accumulated shape will not parse: {e}"))
        })?
    };

    let mut document = serde_json::Map::new();
    document.insert(
        "dataShape".into(),
        serde_json::Value::String(configuration.data_shape.clone()),
    );
    document.insert("accumulatedDataShape".into(), accumulated);
    document.insert(
        "indexes".into(),
        serde_json::Value::Array(
            configuration
                .indexes
                .iter()
                .map(|index| {
                    let mut entry = serde_json::Map::new();
                    entry.insert("name".into(), serde_json::Value::String(index.name.clone()));
                    entry.insert(
                        "fieldNames".into(),
                        serde_json::Value::String(index.field_names.clone()),
                    );
                    serde_json::Value::Object(entry)
                })
                .collect(),
        ),
    );
    Ok(super::mashup::to_sidecar(&serde_json::Value::Object(
        document,
    )))
}

/// Parse a `datatable.json` sidecar.
///
/// Every key the sidecar holds is required, and a value of the wrong type is refused. A
/// missing key would otherwise read as "make this empty", so a truncated or hand-typed file
/// would silently erase a DataTable's shape and all of its indexes on the next sync.
pub fn from_sidecar(text: &str) -> Result<Configuration, DataTableError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| DataTableError::Malformed(format!("datatable.json will not parse: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| DataTableError::Malformed("datatable.json is not an object".into()))?;

    let data_shape = match object.get("dataShape") {
        Some(serde_json::Value::String(s)) => s.clone(),
        None => return Err(missing("dataShape")),
        Some(other) => {
            return Err(DataTableError::Malformed(format!(
                "dataShape must be a string, found {other}"
            )))
        }
    };
    let accumulated = match object.get("accumulatedDataShape") {
        // Null means a DataTable that has never accumulated a shape, which is a real state
        // and not the same as the key being absent.
        Some(serde_json::Value::Null) => String::new(),
        Some(value) => value.to_string(),
        None => return Err(missing("accumulatedDataShape")),
    };

    let mut indexes = Vec::new();
    match object.get("indexes") {
        Some(serde_json::Value::Array(items)) => {
            for (position, item) in items.iter().enumerate() {
                let entry = item.as_object().ok_or_else(|| {
                    DataTableError::Malformed(format!("index {position} is not an object"))
                })?;
                indexes.push(Index {
                    name: index_string(entry, "name", position)?,
                    field_names: index_string(entry, "fieldNames", position)?,
                });
            }
        }
        None => return Err(missing("indexes")),
        Some(other) => {
            return Err(DataTableError::Malformed(format!(
                "indexes must be a list, found {other}"
            )))
        }
    }
    Ok(Configuration {
        data_shape,
        accumulated,
        indexes,
    })
}

fn missing(key: &str) -> DataTableError {
    DataTableError::Malformed(format!(
        "datatable.json has no {key}; the key is required, because an absent one would read as \
         an instruction to empty it"
    ))
}

/// One string member of an index, refusing an absent or wrongly typed one.
///
/// A typo in the key -- `fieldnames` for `fieldNames` -- would otherwise blank the real value
/// rather than say the file is wrong.
fn index_string(
    entry: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    position: usize,
) -> Result<String, DataTableError> {
    match entry.get(key) {
        Some(serde_json::Value::String(s)) => Ok(s.clone()),
        None => Err(DataTableError::Malformed(format!(
            "index {position} has no {key}"
        ))),
        Some(other) => Err(DataTableError::Malformed(format!(
            "index {position}: {key} must be a string, found {other}"
        ))),
    }
}

/// Write a `datatable.json` back into a DataTable document.
///
/// Only the three things the sidecar holds are touched, each inside its own element, so every
/// other configuration table and every attribute keeps the bytes the exporter wrote.
pub fn sync(src: &[u8], wanted: &Configuration) -> Result<(Vec<u8>, Vec<String>), DataTableError> {
    let current = extract(src)?;
    let tokens = scan::tokenize(src).map_err(DataTableError::Scan)?;
    let thing = data_table_thing(&tokens, src).ok_or(DataTableError::NotADataTable)?;
    let settings = table_named(&tokens, src, thing, "Settings")
        .ok_or(DataTableError::MissingTable("Settings"))?;
    let row = first_row(&tokens, src, settings)
        .ok_or_else(|| DataTableError::Malformed("the Settings table has no Row".into()))?;

    let mut changes = Vec::new();
    let mut edits = Vec::new();
    let document_newline = super::sync::newline_of(&String::from_utf8_lossy(src));

    if current.data_shape != wanted.data_shape {
        let span = value_region(&tokens, src, row, "dataShape").ok_or_else(|| {
            DataTableError::Malformed("no <dataShape> to write the shape name into".into())
        })?;
        writable(&tokens, src, span, "dataShape")?;
        let existing = String::from_utf8_lossy(span.of(src));
        let newline = super::sync::newline_of(&existing);
        edits.push(super::splice::Edit::new(
            span,
            render_value(&existing, &wanted.data_shape, newline),
        ));
        changes.push("dataShape".to_string());
    }

    if !same_json(&current.accumulated, &wanted.accumulated) {
        let span = value_region(&tokens, src, row, "accumulatedDataShape").ok_or_else(|| {
            DataTableError::Malformed("no <accumulatedDataShape> to write the shape into".into())
        })?;
        writable(&tokens, src, span, "accumulatedDataShape")?;
        let existing = String::from_utf8_lossy(span.of(src));
        let newline = super::sync::newline_of(&existing);
        edits.push(super::splice::Edit::new(
            span,
            render_value(&existing, wanted.accumulated.trim(), newline),
        ));
        changes.push("accumulatedDataShape".to_string());
    }

    if current.indexes != wanted.indexes {
        edits.extend(index_edits(
            &tokens,
            src,
            thing,
            &current,
            wanted,
            document_newline,
        )?);
        changes.push("indexes".to_string());
    }

    if edits.is_empty() {
        return Ok((src.to_vec(), changes));
    }
    let out =
        super::splice::splice(src, &edits).map_err(|e| DataTableError::Malformed(e.to_string()))?;
    Ok((out, changes))
}

/// The edits that turn the document's index rows into the ones the sidecar asks for.
///
/// Row by row, not table at a time. Rewriting the whole `<Rows>` body would discard anything
/// this module does not model -- a row attribute, a comment, a column a later platform version
/// adds -- from every row in the table because one of them changed a name.
fn index_edits(
    tokens: &[Token],
    src: &[u8],
    thing: (usize, usize),
    current: &Configuration,
    wanted: &Configuration,
    newline: &str,
) -> Result<Vec<super::splice::Edit>, DataTableError> {
    let table = table_named(tokens, src, thing, "Indexes")
        .ok_or(DataTableError::MissingTable("Indexes"))?;
    let rows_tag = scan::child_tags(tokens, src, "Rows", table)
        .first()
        .copied()
        .ok_or_else(|| {
            DataTableError::Malformed("the Indexes table has no <Rows> to write into".into())
        })?;
    // Checked here rather than relied on: a child search over an unclosed element quietly
    // returns nothing, which would read as "this table has no rows".
    let _closed = scan::element_end_in(tokens, src, rows_tag)
        .ok_or_else(|| DataTableError::Malformed("<Rows> is not closed".into()))?;
    let rows = scan::child_tags(tokens, src, "Row", rows_tag);
    let indent = own_line_indent(src, tokens[rows_tag].span.start).unwrap_or_default();
    let mut edits = Vec::new();

    // Rows the document already has: only the values that differ are written.
    for (position, row) in rows.iter().enumerate().take(wanted.indexes.len()) {
        for (name, before, after) in [
            (
                "name",
                &current.indexes[position].name,
                &wanted.indexes[position].name,
            ),
            (
                "fieldNames",
                &current.indexes[position].field_names,
                &wanted.indexes[position].field_names,
            ),
        ] {
            if before == after {
                continue;
            }
            let span = value_region(tokens, src, *row, name).ok_or_else(|| {
                DataTableError::Malformed(format!("index {position} has no <{name}> to write into"))
            })?;
            writable(tokens, src, span, name)?;
            let existing = String::from_utf8_lossy(span.of(src));
            let local = super::sync::newline_of(&existing);
            edits.push(super::splice::Edit::new(
                span,
                render_value(&existing, after, local),
            ));
        }
    }

    // Rows the sidecar drops, removed with the whitespace that indented them so the table does
    // not end up with a blank line where each one was.
    for row in rows.iter().skip(wanted.indexes.len()) {
        let end = scan::element_end_in(tokens, src, *row)
            .ok_or_else(|| DataTableError::Malformed("a <Row> is not closed".into()))?;
        let start = line_start(src, tokens[*row].span.start);
        edits.push(super::splice::Edit::new(
            scan::Span::new(start, tokens[end].span.end),
            Vec::new(),
        ));
    }

    // Rows the sidecar adds, written after the last one the document already has so the
    // whitespace before the closing tag stays where it is.
    if wanted.indexes.len() > rows.len() {
        if tokens[rows_tag].kind == Kind::Empty {
            return Err(DataTableError::Malformed(
                "the Indexes table's <Rows/> is empty and self-closed; there is nowhere to write \
                 a row without rewriting the element"
                    .into(),
            ));
        }
        let at = match rows.last() {
            Some(&last) => {
                let end = scan::element_end_in(tokens, src, last)
                    .ok_or_else(|| DataTableError::Malformed("a <Row> is not closed".into()))?;
                tokens[end].span.end
            }
            None => tokens[rows_tag].span.end,
        };
        let added = render_rows(&wanted.indexes[rows.len()..], &indent, newline);
        edits.push(super::splice::Edit::new(
            scan::Span::new(at, at),
            added.into_bytes(),
        ));
    }
    Ok(edits)
}

/// The offset of the newline that begins a tag's line, when nothing else shares it.
///
/// Deleting from here rather than from the tag takes the line's indentation with it, so a
/// removed row does not leave a blank line behind.
fn line_start(src: &[u8], at: usize) -> usize {
    match own_line_indent(src, at) {
        Some(indent) => {
            let line = at - indent.len();
            // Take the line ending too, CRLF included.
            if line >= 2 && &src[line - 2..line] == b"\r\n" {
                line - 2
            } else if line >= 1 && src[line - 1] == b'\n' {
                line - 1
            } else {
                line
            }
        }
        None => at,
    }
}

/// Whether two accumulated shapes say the same thing, whatever their spacing.
fn same_json(a: &str, b: &str) -> bool {
    match (
        serde_json::from_str::<serde_json::Value>(a.trim()),
        serde_json::from_str::<serde_json::Value>(b.trim()),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.trim() == b.trim(),
    }
}

/// Refuse to rewrite a value whose element holds markup between its CDATA sections: the write
/// covers them all and would drop it.
fn writable(
    tokens: &[Token],
    src: &[u8],
    span: scan::Span,
    what: &str,
) -> Result<(), DataTableError> {
    let in_cdata = span.of(src).starts_with(b"<![CDATA[");
    if scan::only_cdata_and_text(tokens, src, span, in_cdata) {
        Ok(())
    } else {
        Err(DataTableError::Malformed(format!(
            "<{what}> holds markup between its CDATA sections, which a sync would drop; take it out of the entity file"
        )))
    }
}

/// The bytes to replace when a row value changes.
///
/// A JSON-typed value lives inside a `<json>` element, and that wrapper is what the platform
/// reads, so the replacement goes inside it rather than over it. The span covers the CDATA
/// sections and nothing else, so the whitespace the exporter put around them stays put.
fn value_region(tokens: &[Token], src: &[u8], row: usize, name: &str) -> Option<scan::Span> {
    let child = scan::child_tags(tokens, src, name, row).first().copied()?;
    let holder = scan::child_tags(tokens, src, "json", child)
        .first()
        .copied()
        .unwrap_or(child);
    if tokens[holder].kind == Kind::Empty {
        return None;
    }
    let end = scan::element_end_in(tokens, src, holder)?;
    let cdata: Vec<usize> = (holder + 1..end)
        .filter(|&i| tokens[i].kind == Kind::Cdata)
        .collect();
    match (cdata.first(), cdata.last()) {
        (Some(&first), Some(&last)) => Some(scan::Span::new(
            tokens[first].span.start,
            tokens[last].span.end,
        )),
        // No CDATA at all: the element holds plain text, or nothing yet.
        _ => Some(scan::Span::new(
            tokens[holder].span.end,
            tokens[end].span.start,
        )),
    }
}

/// A value re-indented the way the document already indents that element.
///
/// A one-line value stays on its line. One written across lines keeps that shape, so a change
/// to a single value does not reflow the element around it into a diff nobody asked for.
fn render_value(existing: &str, value: &str, newline: &str) -> Vec<u8> {
    let inner = existing
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(existing);
    let rendered = if inner.contains('\n') {
        super::sync::render_payload(inner, value, newline, true)
    } else {
        value.to_string()
    };
    scan::render_cdata(rendered.as_bytes())
}

/// New rows, each on a line of its own, indented one step inside `<Rows>`.
fn render_rows(indexes: &[Index], indent: &str, newline: &str) -> String {
    let mut out = String::new();
    for index in indexes {
        out.push_str(newline);
        out.push_str(indent);
        out.push_str("  <Row><name>");
        out.push_str(&String::from_utf8_lossy(&scan::render_cdata(
            index.name.as_bytes(),
        )));
        out.push_str("</name><fieldNames>");
        out.push_str(&String::from_utf8_lossy(&scan::render_cdata(
            index.field_names.as_bytes(),
        )));
        out.push_str("</fieldNames></Row>");
    }
    out
}

/// The whitespace before a tag, when nothing else shares its line.
fn own_line_indent(src: &[u8], at: usize) -> Option<String> {
    let start = src[..at]
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let prefix = &src[start..at];
    prefix
        .iter()
        .all(|b| *b == b' ' || *b == b'\t')
        .then(|| String::from_utf8_lossy(prefix).into_owned())
}

/// The index of a `ConfigurationTable` with the given name, inside the DataTable Thing.
fn table_named(tokens: &[Token], src: &[u8], thing: (usize, usize), name: &str) -> Option<usize> {
    (thing.0 + 1..thing.1).find(|&i| {
        tokens[i].kind == Kind::Start
            && tokens[i].name.of(src) == b"ConfigurationTable"
            && matches!(scan::attribute(src, &tokens[i], "name"), Ok(Some(v)) if v.of(src) == name.as_bytes())
    })
}

fn rows_of(tokens: &[Token], src: &[u8], table: usize) -> Vec<usize> {
    let Some(rows) = scan::child_tags(tokens, src, "Rows", table)
        .first()
        .copied()
    else {
        return Vec::new();
    };
    scan::child_tags(tokens, src, "Row", rows)
}

fn first_row(tokens: &[Token], src: &[u8], table: usize) -> Option<usize> {
    rows_of(tokens, src, table).first().copied()
}

/// The text of one child element of a row, CDATA and `<json>` wrapper alike.
///
/// A JSON-typed value sits inside a `<json>` element inside the CDATA, and everything is
/// surrounded by the indentation the exporter used, so the result is trimmed.
fn child_text(tokens: &[Token], src: &[u8], row: usize, name: &str) -> Option<String> {
    let child = scan::child_tags(tokens, src, name, row).first().copied()?;
    let end = scan::element_end_in(tokens, src, child)?;
    let text: String = (child + 1..end)
        .filter_map(|i| match tokens[i].kind {
            Kind::Cdata => Some(String::from_utf8_lossy(tokens[i].inner.of(src)).into_owned()),
            Kind::Text => Some(scan::decode_entities(&String::from_utf8_lossy(
                tokens[i].span.of(src),
            ))),
            _ => None,
        })
        .collect();
    Some(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &[u8] = br#"<Entities><Things>
        <Thing name="My_DT" projectName="P" thingTemplate="DataTable">
            <ConfigurationTables>
                <ConfigurationTable name="Settings">
                    <Rows><Row>
                        <accumulatedDataShape><json><![CDATA[{"fieldDefinitions":{"id":{"name":"id"}}}]]></json></accumulatedDataShape>
                        <dataShape><![CDATA[My_DS]]></dataShape>
                    </Row></Rows>
                </ConfigurationTable>
                <ConfigurationTable name="Indexes">
                    <Rows>
                        <Row><name><![CDATA[byName]]></name><fieldNames><![CDATA[dashboardName]]></fieldNames></Row>
                    </Rows>
                </ConfigurationTable>
            </ConfigurationTables>
        </Thing>
    </Things></Entities>"#;

    #[test]
    fn a_data_table_is_recognised_by_its_template() {
        assert!(is_data_table(TABLE));
        let other = br#"<Entities><Things><Thing name="T" thingTemplate="GenericThing"></Thing></Things></Entities>"#;
        assert!(!is_data_table(other));
        assert!(matches!(extract(other), Err(DataTableError::NotADataTable)));
    }

    #[test]
    fn the_shape_and_indexes_come_out() {
        let configuration = extract(TABLE).unwrap();
        assert_eq!(configuration.data_shape, "My_DS");
        assert_eq!(configuration.indexes.len(), 1);
        assert_eq!(configuration.indexes[0].name, "byName");
        assert_eq!(configuration.indexes[0].field_names, "dashboardName");
    }

    #[test]
    fn the_accumulated_shape_comes_out_of_its_json_wrapper() {
        // A JSON-typed value is wrapped twice: a <json> element inside the CDATA.
        let configuration = extract(TABLE).unwrap();
        assert!(configuration
            .accumulated
            .starts_with("{\"fieldDefinitions\""));
    }

    #[test]
    fn the_sidecar_round_trips() {
        let configuration = extract(TABLE).unwrap();
        let text = to_sidecar(&configuration).unwrap();
        let back = from_sidecar(&text).unwrap();
        assert_eq!(back.data_shape, configuration.data_shape);
        assert_eq!(back.indexes, configuration.indexes);
        // The accumulated shape is re-serialised, so compare it as JSON rather than as text.
        let a: serde_json::Value = serde_json::from_str(&back.accumulated).unwrap();
        let b: serde_json::Value = serde_json::from_str(&configuration.accumulated).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn a_data_table_with_no_indexes_is_fine() {
        let src = br#"<Entities><Things><Thing name="T" thingTemplate="DataTable">
            <ConfigurationTables><ConfigurationTable name="Settings"><Rows><Row>
                <dataShape><![CDATA[S]]></dataShape>
            </Row></Rows></ConfigurationTable></ConfigurationTables>
        </Thing></Things></Entities>"#;
        let configuration = extract(src).unwrap();
        assert!(configuration.indexes.is_empty());
        assert!(to_sidecar(&configuration)
            .unwrap()
            .contains("\"indexes\": []"));
    }

    #[test]
    fn a_missing_settings_table_is_named_in_the_error() {
        let src = br#"<Entities><Things><Thing name="T" thingTemplate="DataTable">
            <ConfigurationTables></ConfigurationTables></Thing></Things></Entities>"#;
        assert!(matches!(
            extract(src),
            Err(DataTableError::MissingTable("Settings"))
        ));
    }

    #[test]
    fn syncing_what_was_extracted_changes_nothing() {
        let configuration = extract(TABLE).unwrap();
        let (out, changes) = sync(TABLE, &configuration).unwrap();
        assert!(changes.is_empty(), "got {changes:?}");
        assert_eq!(out, TABLE);
    }

    #[test]
    fn markup_between_value_sections_is_refused_rather_than_dropped() {
        let text = String::from_utf8(TABLE.to_vec()).unwrap().replace(
            "<dataShape><![CDATA[My_DS]]></dataShape>",
            "<dataShape><![CDATA[My_]]><!-- note --><![CDATA[DS]]></dataShape>",
        );
        let mut configuration = extract(text.as_bytes()).unwrap();
        assert_eq!(configuration.data_shape, "My_DS");
        configuration.data_shape = "Other_DS".to_string();
        let error = sync(text.as_bytes(), &configuration)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("markup between its CDATA sections"),
            "{error}"
        );
    }

    #[test]
    fn a_new_shape_name_is_written_into_its_own_element() {
        let mut configuration = extract(TABLE).unwrap();
        configuration.data_shape = "Other_DS".to_string();
        let (out, changes) = sync(TABLE, &configuration).unwrap();
        assert_eq!(changes, vec!["dataShape"]);
        assert_eq!(extract(&out).unwrap(), configuration);
        // Everything else keeps the bytes the exporter wrote.
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(r#"<Row><name><![CDATA[byName]]>"#), "{text}");
    }

    #[test]
    fn the_accumulated_shape_is_written_inside_its_json_wrapper() {
        let mut configuration = extract(TABLE).unwrap();
        configuration.accumulated =
            r#"{"fieldDefinitions":{"id":{"name":"id"},"b":{"name":"b"}}}"#.to_string();
        let (out, changes) = sync(TABLE, &configuration).unwrap();
        assert_eq!(changes, vec!["accumulatedDataShape"]);
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            text.contains("<accumulatedDataShape><json><![CDATA[{"),
            "{text}"
        );
        assert!(same_json(
            &extract(&out).unwrap().accumulated,
            &configuration.accumulated
        ));
    }

    #[test]
    fn reordered_spacing_in_the_accumulated_shape_is_not_a_change() {
        let mut configuration = extract(TABLE).unwrap();
        configuration.accumulated =
            r#"{ "fieldDefinitions" : { "id" : { "name" : "id" } } }"#.to_string();
        let (out, changes) = sync(TABLE, &configuration).unwrap();
        assert!(changes.is_empty(), "got {changes:?}");
        assert_eq!(out, TABLE);
    }

    #[test]
    fn an_added_index_becomes_a_new_row() {
        let mut configuration = extract(TABLE).unwrap();
        configuration.indexes.push(Index {
            name: "byOwner".to_string(),
            field_names: "owner,createdAt".to_string(),
        });
        let (out, changes) = sync(TABLE, &configuration).unwrap();
        assert_eq!(changes, vec!["indexes"]);
        assert_eq!(extract(&out).unwrap().indexes, configuration.indexes);
        // And the rewrite is stable: syncing the result back changes nothing again.
        assert!(sync(&out, &configuration).unwrap().1.is_empty());
    }

    #[test]
    fn removing_every_index_leaves_an_empty_rows_element() {
        let mut configuration = extract(TABLE).unwrap();
        configuration.indexes.clear();
        let (out, _) = sync(TABLE, &configuration).unwrap();
        assert!(extract(&out).unwrap().indexes.is_empty());
        assert!(sync(&out, &configuration).unwrap().1.is_empty());
    }

    #[test]
    fn a_value_written_across_lines_keeps_that_shape() {
        // The exporter indents a CDATA section onto lines of its own. Collapsing it onto one
        // line writes a diff nobody asked for into an element that only changed its value.
        let src = b"<Entities><Things><Thing name=\"T\" thingTemplate=\"DataTable\">\n<ConfigurationTables><ConfigurationTable name=\"Settings\"><Rows><Row>\n        <dataShape>\n        <![CDATA[\n        Old_DS\n        ]]>\n        </dataShape>\n</Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>";
        let mut configuration = extract(src).unwrap();
        configuration.data_shape = "New_DS".to_string();
        let (out, _) = sync(src, &configuration).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            text.contains("<![CDATA[\n        New_DS\n        ]]>"),
            "{text}"
        );
        assert_eq!(extract(&out).unwrap().data_shape, "New_DS");
    }

    #[test]
    fn a_value_holding_the_cdata_terminator_survives() {
        // Written raw, a "]]>" inside the value closes the section early and truncates the
        // document from there on.
        let mut configuration = extract(TABLE).unwrap();
        configuration.indexes[0].field_names = "a]]>b".to_string();
        let (out, _) = sync(TABLE, &configuration).unwrap();
        assert_eq!(extract(&out).unwrap().indexes[0].field_names, "a]]>b");
    }

    #[test]
    fn a_wrongly_typed_sidecar_value_is_refused() {
        assert!(
            from_sidecar(r#"{"dataShape": 3, "accumulatedDataShape": null, "indexes": []}"#)
                .is_err()
        );
        assert!(from_sidecar(
            r#"{"dataShape": "S", "accumulatedDataShape": null, "indexes": "no"}"#
        )
        .is_err());
    }

    #[test]
    fn a_sidecar_missing_a_key_is_refused_rather_than_read_as_empty() {
        // `{}` would otherwise mean "clear the shape and every index", which is the worst
        // possible reading of a truncated or half-typed file.
        assert!(from_sidecar("{}").is_err());
        assert!(from_sidecar(r#"{"dataShape": "S", "indexes": []}"#).is_err());
        assert!(from_sidecar(r#"{"dataShape": "S", "accumulatedDataShape": null}"#).is_err());
    }

    #[test]
    fn a_mistyped_index_key_is_refused_rather_than_blanking_the_value() {
        let text = r#"{"dataShape":"S","accumulatedDataShape":null,
                       "indexes":[{"name":"i","fieldnames":"a"}]}"#;
        let message = from_sidecar(text).unwrap_err().to_string();
        assert!(message.contains("fieldNames"), "got {message}");
    }

    #[test]
    fn an_accumulated_shape_that_will_not_parse_is_refused_not_emptied() {
        let configuration = Configuration {
            data_shape: "S".to_string(),
            accumulated: "{not json".to_string(),
            indexes: Vec::new(),
        };
        assert!(to_sidecar(&configuration).is_err());
    }

    #[test]
    fn a_shape_that_was_never_accumulated_stays_null_through_the_sidecar() {
        let src = br#"<Entities><Things><Thing name="T" thingTemplate="DataTable">
            <ConfigurationTables><ConfigurationTable name="Settings"><Rows><Row>
                <dataShape><![CDATA[S]]></dataShape>
                <accumulatedDataShape><json><![CDATA[]]></json></accumulatedDataShape>
            </Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>"#;
        let text = to_sidecar(&extract(src).unwrap()).unwrap();
        assert!(text.contains("\"accumulatedDataShape\": null"), "{text}");
        // And it round-trips without inventing an empty object to write back.
        let (out, changes) = sync(src, &from_sidecar(&text).unwrap()).unwrap();
        assert!(changes.is_empty(), "got {changes:?}");
        assert_eq!(out, src);
    }

    #[test]
    fn a_configuration_written_out_by_hand_is_the_document_as_it_stands() {
        // Not `extract`'s output: giving `sync` what `extract` produced lets the two agree on a
        // mistake they share. These values were read off the XML above by eye.
        let stated = Configuration {
            data_shape: "My_DS".to_string(),
            accumulated: r#"{"fieldDefinitions":{"id":{"name":"id"}}}"#.to_string(),
            indexes: vec![Index {
                name: "byName".to_string(),
                field_names: "dashboardName".to_string(),
            }],
        };
        assert_eq!(extract(TABLE).unwrap(), stated);
        let (out, changes) = sync(TABLE, &stated).unwrap();
        assert!(changes.is_empty(), "got {changes:?}");
        assert_eq!(out, TABLE);
    }

    #[test]
    fn changing_one_index_leaves_every_other_row_byte_for_byte() {
        // Rewriting the whole <Rows> body would discard the comment and the attribute, which
        // this module does not model and has no business deleting.
        let src = b"<Entities><Things><Thing name=\"T\" thingTemplate=\"DataTable\">\n<ConfigurationTables>\n<ConfigurationTable name=\"Settings\"><Rows><Row><dataShape><![CDATA[S]]></dataShape></Row></Rows></ConfigurationTable>\n<ConfigurationTable name=\"Indexes\">\n    <Rows>\n        <!-- kept -->\n        <Row order=\"1\"><name><![CDATA[a]]></name><fieldNames><![CDATA[x]]></fieldNames></Row>\n        <Row order=\"2\"><name><![CDATA[b]]></name><fieldNames><![CDATA[y]]></fieldNames></Row>\n    </Rows>\n</ConfigurationTable>\n</ConfigurationTables></Thing></Things></Entities>";
        let mut configuration = extract(src).unwrap();
        configuration.indexes[1].field_names = "z".to_string();
        let (out, _) = sync(src, &configuration).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains("<!-- kept -->"), "{text}");
        assert!(
            text.contains("<Row order=\"1\"><name><![CDATA[a]]></name>"),
            "{text}"
        );
        assert!(
            text.contains("<Row order=\"2\"><name><![CDATA[b]]></name><fieldNames><![CDATA[z]]>"),
            "{text}"
        );
        assert_eq!(extract(&out).unwrap(), configuration);
    }

    #[test]
    fn a_removed_index_takes_its_line_with_it() {
        let src = b"<Entities><Things><Thing name=\"T\" thingTemplate=\"DataTable\">\n<ConfigurationTables>\n<ConfigurationTable name=\"Settings\"><Rows><Row><dataShape><![CDATA[S]]></dataShape></Row></Rows></ConfigurationTable>\n<ConfigurationTable name=\"Indexes\">\n    <Rows>\n        <Row><name><![CDATA[a]]></name><fieldNames><![CDATA[x]]></fieldNames></Row>\n        <Row><name><![CDATA[b]]></name><fieldNames><![CDATA[y]]></fieldNames></Row>\n    </Rows>\n</ConfigurationTable>\n</ConfigurationTables></Thing></Things></Entities>";
        let mut configuration = extract(src).unwrap();
        configuration.indexes.pop();
        let (out, _) = sync(src, &configuration).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(!text.contains("CDATA[b]"), "{text}");
        assert!(
            !text.contains("\n\n"),
            "a blank line was left behind:\n{text}"
        );
        assert_eq!(extract(&out).unwrap().indexes, configuration.indexes);
    }

    #[test]
    fn a_document_holding_another_entity_first_is_not_read_from_the_wrong_one() {
        // A search across the whole file finds the first Settings table anywhere, which here
        // belongs to a Thing that is not a DataTable at all.
        let src = br#"<Entities><Things>
            <Thing name="Other" thingTemplate="GenericThing"><ConfigurationTables>
                <ConfigurationTable name="Settings"><Rows><Row><dataShape><![CDATA[Wrong_DS]]></dataShape></Row></Rows></ConfigurationTable>
            </ConfigurationTables></Thing>
            <Thing name="T" thingTemplate="DataTable"><ConfigurationTables>
                <ConfigurationTable name="Settings"><Rows><Row><dataShape><![CDATA[Right_DS]]></dataShape></Row></Rows></ConfigurationTable>
            </ConfigurationTables></Thing>
        </Things></Entities>"#;
        assert_eq!(extract(src).unwrap().data_shape, "Right_DS");
    }
}
