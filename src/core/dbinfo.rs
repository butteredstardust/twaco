//! Which DataShapes a solution stores through DBConnection, read from its `GetDBInfo` services.
//!
//! A DBConnection manager describes its tables in an overriding `GetDBInfo` service whose script
//! returns an object literal:
//!
//! ```text
//! { dbInfo: [ { dataShapeName: "A.B.Dashboards",
//!               fields:        [ { name: "UserName", notNull: true } ],
//!               indexedFields: [ { name: "UserName" }, { fieldNames: ["Dashboard_UID", "UserName"] } ],
//!               foreignKeys:   [ { name: "Dashboard_UID", referenceDataShapeName: "A.B.Dashboards",
//!                                  referenceFieldName: "UID" } ] } ] }
//! ```
//!
//! This is a tolerant reader for that literal subset, not a JavaScript parser: it finds the objects
//! that carry a `dataShapeName` string and the string values of the few keys that name a field, with
//! the byte span of each so a rename can edit them in place. A script it cannot read that way is
//! reported as `unsure` rather than ignored, because a rename of a table the reader missed would
//! leave the database behind.

use super::config::Solution;
use super::rename_scan::{js_tokens, JsKind, JsToken};
use super::scan::{self, Span};
use super::{check, workspace};
use std::collections::BTreeMap;

/// A name that appears as a string literal in the script, with the span of its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub value: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub column: Option<Name>,
    pub reference_shape: Option<Name>,
    pub reference_field: Option<Name>,
}

/// One table the script describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbShape {
    pub data_shape: Name,
    /// `fields[].name`.
    pub fields: Vec<Name>,
    /// One entry per index: `indexedFields[].name` is a one-column index, `fieldNames` several.
    pub indexes: Vec<Vec<Name>>,
    pub foreign_keys: Vec<ForeignKey>,
}

/// What one script says.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    pub shapes: Vec<DbShape>,
    /// The script mentions `dbInfo` but its tables could not all be read as literals.
    pub unsure: bool,
}

/// Read one `GetDBInfo` script.
pub fn scan_script(src: &[u8]) -> Scan {
    let all = js_tokens(src);
    let tokens: Vec<JsToken> = all.into_iter().filter(|token| token.kind != JsKind::Comment).collect();
    let mut scan = Scan::default();
    let mut opens = Vec::new();
    let mut matching = vec![None; tokens.len()];
    for (at, token) in tokens.iter().enumerate() {
        match raw(src, token) {
            b"{" | b"[" | b"(" => opens.push(at),
            b"}" | b"]" | b")" => {
                if let Some(open) = opens.pop() {
                    matching[open] = Some(at);
                }
            }
            _ => {}
        }
    }
    let mut seen_objects = Vec::new();
    for at in 0..tokens.len() {
        if key_text(src, &tokens[at]).as_deref() != Some("dataShapeName") || raw_at(src, &tokens, at + 1) != Some(b":") {
            continue;
        }
        let Some(open) = enclosing_open(src, &tokens, at, &matching) else { continue };
        if raw(src, &tokens[open]) != b"{" || seen_objects.contains(&open) {
            continue;
        }
        seen_objects.push(open);
        let Some(close) = matching[open] else { scan.unsure = true; continue };
        match read_shape(src, &tokens, open, close, &matching) {
            Some(shape) => scan.shapes.push(shape),
            None => scan.unsure = true,
        }
    }
    let mentions = src.windows(6).any(|window| window == b"dbInfo");
    if mentions && scan.shapes.is_empty() {
        scan.unsure = true;
    }
    scan
}

/// Every DB-backed DataShape across scripts, by full name, with the scripts that describe it.
#[derive(Debug, Default)]
pub struct DbInfo {
    /// Data shape name -> the shape as the first script describing it reads.
    pub shapes: BTreeMap<String, DbShape>,
    /// Scripts that look like a `GetDBInfo` but could not be read completely.
    pub unsure: Vec<String>,
}

impl DbInfo {
    pub fn add(&mut self, origin: &str, scan: Scan) {
        for shape in scan.shapes {
            self.shapes.entry(shape.data_shape.value.clone()).or_insert(shape);
        }
        if scan.unsure {
            self.unsure.push(origin.to_string());
        }
    }

    pub fn is_backed(&self, data_shape: &str) -> bool {
        self.shapes.contains_key(data_shape)
    }

    pub fn any(&self) -> bool {
        !self.shapes.is_empty() || !self.unsure.is_empty()
    }
}

/// Read every `GetDBInfo` of the solution: the sidecar scripts, and the scripts inside entity XML
/// (a `GetDBInfo` that overrides an inherited definition has no sidecar, so its only copy is there).
pub fn load(solution: &Solution) -> DbInfo {
    let mut info = DbInfo::default();
    for entity in workspace::entities(solution) {
        if !matches!(entity.info.collection.as_str(), "Things" | "ThingTemplates" | "ThingShapes") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&entity.path) else { continue };
        let Ok(scripts) = scan_entity(&bytes) else { continue };
        for (_, scan) in scripts {
            info.add(&format!("{}/{}", entity.info.collection, entity.info.name), scan);
        }
    }
    for path in check::walk_files(solution) {
        let is_script = path.file_name().is_some_and(|name| name == "script.js")
            && path.parent().and_then(|parent| parent.file_name()).is_some_and(|name| name == "GetDBInfo");
        if !is_script {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            info.add(&path.display().to_string(), scan_script(&bytes));
        }
    }
    info
}

/// The `GetDBInfo` scripts inside an entity document, each as its offset in the document (where its
/// CDATA content starts) and what it says. The offset turns a span in the script into a span in the XML.
pub fn scan_entity(src: &[u8]) -> Result<Vec<(usize, Scan)>, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut out = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let is_service = token.kind == scan::Kind::Start
            && token.name.of(src) == b"ServiceImplementation"
            && scan::attribute(src, token, "name")?.is_some_and(|span| span.of(src) == b"GetDBInfo");
        if !is_service {
            continue;
        }
        let Some(end) = scan::element_end_in(&tokens, src, index) else { continue };
        for cdata in &tokens[index + 1..end] {
            if cdata.kind == scan::Kind::Cdata {
                out.push((cdata.inner.start, scan_script(cdata.inner.of(src))));
            }
        }
    }
    Ok(out)
}

/// The table DBConnection makes for a DataShape: its short name, lowercased.
pub fn table_of(data_shape: &str) -> String {
    data_shape.rsplit('.').next().unwrap_or(data_shape).to_lowercase()
}

/// The column DBConnection makes for a field: its name, lowercased.
pub fn column_of(field: &str) -> String {
    field.to_lowercase()
}

/// The spans of `field` in `scan` that belong to `data_shape`: its own `fields`, `indexedFields`
/// and foreign keys, and the `referenceFieldName` of a foreign key in another table that points at it.
pub fn field_spans(scan: &Scan, data_shape: &str, field: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    for shape in &scan.shapes {
        if shape.data_shape.value == data_shape {
            spans.extend(shape.fields.iter().filter(|name| name.value == field).map(|name| name.span));
            spans.extend(shape.indexes.iter().flatten().filter(|name| name.value == field).map(|name| name.span));
            spans.extend(
                shape
                    .foreign_keys
                    .iter()
                    .filter_map(|key| key.column.as_ref())
                    .filter(|name| name.value == field)
                    .map(|name| name.span),
            );
        }
        for key in &shape.foreign_keys {
            let points_here = key.reference_shape.as_ref().is_some_and(|name| name.value == data_shape);
            if let (true, Some(name)) = (points_here, key.reference_field.as_ref()) {
                if name.value == field {
                    spans.push(name.span);
                }
            }
        }
    }
    spans.sort_by_key(|span| span.start);
    spans.dedup();
    spans
}

fn raw<'a>(src: &'a [u8], token: &JsToken) -> &'a [u8] {
    token.span.of(src)
}

fn raw_at<'a>(src: &'a [u8], tokens: &[JsToken], at: usize) -> Option<&'a [u8]> {
    tokens.get(at).map(|token| raw(src, token))
}

/// The text of an identifier or a plain string literal used as an object key.
fn key_text(src: &[u8], token: &JsToken) -> Option<String> {
    match token.kind {
        JsKind::Ident => Some(String::from_utf8_lossy(raw(src, token)).into_owned()),
        JsKind::String => string_name(src, token).map(|name| name.value),
        _ => None,
    }
}

/// A string literal without escapes: its content and the span of that content.
fn string_name(src: &[u8], token: &JsToken) -> Option<Name> {
    if token.kind != JsKind::String {
        return None;
    }
    let text = raw(src, token);
    if text.len() < 2 || text.contains(&b'\\') {
        return None;
    }
    let span = Span::new(token.span.start + 1, token.span.end - 1);
    Some(Name { value: String::from_utf8_lossy(span.of(src)).into_owned(), span })
}

/// The `{` or `[` that most closely encloses token `at`.
fn enclosing_open(src: &[u8], tokens: &[JsToken], at: usize, matching: &[Option<usize>]) -> Option<usize> {
    (0..at).rev().find(|&index| {
        matches!(raw(src, &tokens[index]), b"{" | b"[" | b"(") && matching[index].is_some_and(|close| close > at)
    })
}

/// The direct `key: value` properties of the object between `open` and `close`: each as the key text
/// and the index range of the value tokens.
fn properties(src: &[u8], tokens: &[JsToken], open: usize, close: usize, matching: &[Option<usize>]) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut at = open + 1;
    while at < close {
        let Some(key) = key_text(src, &tokens[at]) else {
            at += 1;
            continue;
        };
        if raw_at(src, tokens, at + 1) != Some(b":") {
            at += 1;
            continue;
        }
        let start = at + 2;
        let mut end = start;
        while end < close && raw(src, &tokens[end]) != b"," {
            end = matching[end].unwrap_or(end) + 1;
        }
        out.push((key, start, end));
        at = end + 1;
    }
    out
}

fn read_shape(src: &[u8], tokens: &[JsToken], open: usize, close: usize, matching: &[Option<usize>]) -> Option<DbShape> {
    let mut shape = DbShape {
        data_shape: Name { value: String::new(), span: Span::new(0, 0) },
        fields: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
    };
    let mut named = false;
    for (key, start, end) in properties(src, tokens, open, close, matching) {
        match key.as_str() {
            "dataShapeName" => {
                shape.data_shape = string_name(src, tokens.get(start)?)?;
                named = true;
            }
            "fields" | "indexedFields" | "foreignKeys" => {
                if raw_at(src, tokens, start) != Some(b"[") {
                    return None;
                }
                let array_close = matching[start]?;
                let mut at = start + 1;
                while at < array_close {
                    if raw(src, &tokens[at]) == b"{" {
                        let element_close = matching[at]?;
                        read_element(src, tokens, &key, at, element_close, matching, &mut shape)?;
                        at = element_close + 1;
                    } else {
                        at += 1;
                    }
                }
                let _ = end;
            }
            _ => {}
        }
    }
    named.then_some(shape)
}

fn read_element(
    src: &[u8],
    tokens: &[JsToken],
    list: &str,
    open: usize,
    close: usize,
    matching: &[Option<usize>],
    shape: &mut DbShape,
) -> Option<()> {
    let mut foreign = ForeignKey { column: None, reference_shape: None, reference_field: None };
    for (key, start, _) in properties(src, tokens, open, close, matching) {
        let value = tokens.get(start)?;
        match (list, key.as_str()) {
            ("fields", "name") => shape.fields.push(string_name(src, value)?),
            ("indexedFields", "name") => shape.indexes.push(vec![string_name(src, value)?]),
            ("indexedFields", "fieldNames") => {
                if raw(src, value) != b"[" {
                    return None;
                }
                let array_close = matching[start]?;
                let mut names = Vec::new();
                for token in &tokens[start + 1..array_close] {
                    if let Some(name) = string_name(src, token) {
                        names.push(name);
                    }
                }
                shape.indexes.push(names);
            }
            ("foreignKeys", "name") => foreign.column = Some(string_name(src, value)?),
            ("foreignKeys", "referenceDataShapeName") => foreign.reference_shape = Some(string_name(src, value)?),
            ("foreignKeys", "referenceFieldName") => foreign.reference_field = Some(string_name(src, value)?),
            _ => {}
        }
    }
    if list == "foreignKeys" {
        shape.foreign_keys.push(foreign);
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"
// Overrides GetDBInfo.
var result = {
    dbInfo: [
        {
            dataShapeName: "Acme.App.Dashboards",
            // a comment mentioning dataShapeName: "Not.A.Table"
            fields: [
                { name: "UserName", notNull: true },
                { name: 'DashboardName', notNull: true },
            ],
            indexedFields: [ { name: "UserName", unique: false } ],
        },
        {
            "dataShapeName": "Acme.App.SharedDashboards",
            fields: [ { name: "Dashboard_UID", notNull: true } ],
            indexedFields: [ { fieldNames: ["Dashboard_UID", "UserName"], unique: true } ],
            foreignKeys: [ { name: "Dashboard_UID", referenceDataShapeName: "Acme.App.Dashboards", referenceFieldName: "UID" } ],
        },
    ]
};
"#;

    fn names(list: &[Name]) -> Vec<&str> {
        list.iter().map(|name| name.value.as_str()).collect()
    }

    #[test]
    fn reads_tables_fields_indexes_and_foreign_keys_with_quoted_keys_comments_and_trailing_commas() {
        let scan = scan_script(SCRIPT.as_bytes());
        assert!(!scan.unsure);
        assert_eq!(scan.shapes.len(), 2, "the commented-out table is not one");
        let dashboards = &scan.shapes[0];
        assert_eq!(dashboards.data_shape.value, "Acme.App.Dashboards");
        assert_eq!(names(&dashboards.fields), ["UserName", "DashboardName"]);
        assert_eq!(dashboards.indexes.len(), 1);
        let shared = &scan.shapes[1];
        assert_eq!(names(&shared.indexes[0]), ["Dashboard_UID", "UserName"]);
        let key = &shared.foreign_keys[0];
        assert_eq!(key.column.as_ref().unwrap().value, "Dashboard_UID");
        assert_eq!(key.reference_shape.as_ref().unwrap().value, "Acme.App.Dashboards");
        assert_eq!(key.reference_field.as_ref().unwrap().value, "UID");
    }

    #[test]
    fn a_name_span_points_at_the_text_inside_its_quotes() {
        let scan = scan_script(SCRIPT.as_bytes());
        let name = &scan.shapes[0].fields[1];
        assert_eq!(&SCRIPT[name.span.start..name.span.end], "DashboardName");
        let table = &scan.shapes[1].data_shape;
        assert_eq!(&SCRIPT[table.span.start..table.span.end], "Acme.App.SharedDashboards");
    }

    #[test]
    fn a_computed_table_list_is_unsure_not_empty() {
        let scan = scan_script(b"var result = { dbInfo: [] }; tables.forEach(function (t) { result.dbInfo.push(t); });");
        assert!(scan.shapes.is_empty());
        assert!(scan.unsure, "a script that builds dbInfo at run time cannot be trusted to have no tables");
        assert!(!scan_script(b"return 1;").unsure);
    }

    #[test]
    fn field_spans_are_scoped_to_one_shape_and_its_references() {
        let scan = scan_script(SCRIPT.as_bytes());
        let in_text = |spans: Vec<Span>| spans.iter().map(|span| &SCRIPT[span.start..span.end]).collect::<Vec<_>>();
        // `UserName` is a column of both tables' scripts only where the shape matches.
        assert_eq!(in_text(field_spans(&scan, "Acme.App.Dashboards", "UserName")), ["UserName", "UserName"]);
        assert_eq!(in_text(field_spans(&scan, "Acme.App.SharedDashboards", "UserName")), ["UserName"]);
        // The foreign key's own column and the referenced column of another table.
        assert_eq!(in_text(field_spans(&scan, "Acme.App.Dashboards", "UID")), ["UID"]);
        assert_eq!(in_text(field_spans(&scan, "Acme.App.SharedDashboards", "Dashboard_UID")).len(), 3);
        assert!(field_spans(&scan, "Acme.App.Dashboards", "Nope").is_empty());
    }

    #[test]
    fn dbconnection_names_are_lowercased_short_names() {
        assert_eq!(table_of("Acme.App.SharedDashboards"), "shareddashboards");
        assert_eq!(column_of("Dashboard_UID"), "dashboard_uid");
        assert_eq!(table_of("Plain"), "plain");
    }

    #[test]
    fn the_info_collects_shapes_across_scripts_and_remembers_unsure_ones() {
        let mut info = DbInfo::default();
        info.add("a", scan_script(SCRIPT.as_bytes()));
        info.add("b", scan_script(b"{ dbInfo: tables }"));
        assert!(info.is_backed("Acme.App.Dashboards") && !info.is_backed("Acme.Other"));
        assert_eq!(info.unsure, ["b"]);
    }
}
