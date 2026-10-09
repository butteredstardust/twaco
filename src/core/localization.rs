//! Localization-table files kept beside a solution.
//!
//! This module reads the small, shared slice of a ThingWorx localization export that a solution
//! owns.  Edits are deliberately spans over scanner tokens: it refuses a cell whose contents it
//! cannot replace without dropping markup, and never serializes a document it was given.

use super::codes::{Coded, ErrorCode};
use super::config::{Project, Solution};
use super::{scan, splice};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub mod ops;
pub mod remote;
pub use ops::{
    new, pull, push, remove, set, status, Edited, FileChange, Pulled, Pushed, Status, TablePush,
};
pub use remote::Remote;

pub const DEFAULT_ROOT: &str = "localization";
pub const DEFAULT_TABLE: &str = "Default";

/// One token as the server and the files hold it. Values are trimmed when read from a file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Token {
    pub name: String,
    pub value: String,
    pub usage: String,
    pub context: String,
}

/// The attributes a table file's `LocalizationTable` element carries that twaco cares about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub description: Option<String>,
    pub language_common: Option<String>,
    pub language_native: Option<String>,
}

/// One table file read from the solution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableFile {
    pub path: PathBuf,
    pub table: String,
    pub header: Header,
    pub tokens: Vec<Token>,
}

/// Table files found below a localization root and files that looked like tables but were bad.
#[derive(Debug, Default)]
pub struct Discovered {
    pub files: Vec<TableFile>,
    pub unreadable: Vec<(PathBuf, String)>,
}

/// A requested change to one token in a table file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Set(Token),
    Remove(String),
}

/// A localization document could not be read or changed without guessing.
#[derive(Debug, thiserror::Error)]
pub enum LocalizationError {
    #[error("cannot read localization file {}: {why}", path.display())]
    Io { path: PathBuf, why: String },
    #[error("localization file {}: {why}", path.display())]
    Invalid { path: PathBuf, why: String },
    #[error("invalid localization edit: {0}")]
    Arguments(String),
    #[error("{0}")]
    Remote(#[from] super::server::ServerError),
    #[error("{0}")]
    NotVerified(String),
    #[error("{0}")]
    AlreadyExists(String),
    #[error("{0}")]
    Unknown(String),
}

impl Coded for LocalizationError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Io { .. } => ErrorCode::IoError,
            Self::Invalid { .. } => ErrorCode::InvalidData,
            Self::Arguments(_) => ErrorCode::InvalidArguments,
            Self::Remote(error) => error.code(),
            Self::NotVerified(_) => ErrorCode::NotVerified,
            Self::AlreadyExists(_) => ErrorCode::AlreadyExists,
            Self::Unknown(_) => ErrorCode::UnknownEntity,
        }
    }
}

/// The localization root, relative to the solution root unless configuration says otherwise.
pub fn root(solution: &Solution) -> PathBuf {
    solution.root.join(
        solution
            .localization
            .root
            .as_deref()
            .unwrap_or(DEFAULT_ROOT),
    )
}

/// Prefixes this project owns, defaulting to its conventional dotted namespace.
pub fn prefixes(project: &Project) -> Vec<String> {
    if project.localization.prefixes.is_empty() {
        vec![format!("{}.", project.name)]
    } else {
        project.localization.prefixes.clone()
    }
}

#[derive(Clone, Debug)]
struct Cell {
    name: String,
    empty: bool,
    full: scan::Span,
    inner: scan::Span,
    value: String,
}

#[derive(Clone, Debug)]
struct Row {
    full: scan::Span,
    cells: Vec<Cell>,
}

struct Parsed {
    tokens: Vec<scan::Token>,
    table: String,
    header: Header,
    rows_open: Option<usize>,
    rows_close: Option<usize>,
    rows_empty: bool,
    rows: Vec<Row>,
}

fn invalid(path: &Path, why: impl Into<String>) -> LocalizationError {
    LocalizationError::Invalid {
        path: path.to_path_buf(),
        why: why.into(),
    }
}

fn element_end(
    tokens: &[scan::Token],
    src: &[u8],
    at: usize,
    path: &Path,
) -> Result<usize, LocalizationError> {
    scan::element_end_in(tokens, src, at).ok_or_else(|| {
        invalid(
            path,
            "has malformed or unclosed XML; repair the export before using localization",
        )
    })
}

fn direct_children(
    tokens: &[scan::Token],
    src: &[u8],
    parent: usize,
    path: &Path,
) -> Result<Vec<usize>, LocalizationError> {
    let end = element_end(tokens, src, parent, path)?;
    let mut out = Vec::new();
    let mut at = parent + 1;
    while at < end {
        match tokens[at].kind {
            scan::Kind::Start => {
                out.push(at);
                at = element_end(tokens, src, at, path)? + 1;
            }
            scan::Kind::Empty => {
                out.push(at);
                at += 1;
            }
            _ => at += 1,
        }
    }
    Ok(out)
}

fn named_children(
    tokens: &[scan::Token],
    src: &[u8],
    parent: usize,
    name: &str,
    path: &Path,
) -> Result<Vec<usize>, LocalizationError> {
    Ok(direct_children(tokens, src, parent, path)?
        .into_iter()
        .filter(|&i| tokens[i].name.of(src) == name.as_bytes())
        .collect())
}

fn attribute(
    tokens: &[scan::Token],
    src: &[u8],
    at: usize,
    name: &str,
    path: &Path,
) -> Result<Option<String>, LocalizationError> {
    scan::attribute(src, &tokens[at], name)
        .map_err(|e| {
            invalid(
                path,
                format!("cannot scan attributes: {e}; repair the export"),
            )
        })
        .map(|value| {
            value.map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
        })
}

fn cell_text(tokens: &[scan::Token], src: &[u8], open: usize, close: usize) -> String {
    (open + 1..close)
        .filter_map(|i| match tokens[i].kind {
            scan::Kind::Cdata => {
                Some(String::from_utf8_lossy(tokens[i].inner.of(src)).into_owned())
            }
            scan::Kind::Text => Some(scan::decode_entities(&String::from_utf8_lossy(
                tokens[i].span.of(src),
            ))),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn parse_row(
    tokens: &[scan::Token],
    src: &[u8],
    open: usize,
    path: &Path,
) -> Result<Row, LocalizationError> {
    let close = element_end(tokens, src, open, path)?;
    let mut cells = Vec::new();
    for cell_open in direct_children(tokens, src, open, path)? {
        let token = tokens[cell_open];
        let name = String::from_utf8_lossy(token.name.of(src)).into_owned();
        let cell_close = element_end(tokens, src, cell_open, path)?;
        let empty = token.kind == scan::Kind::Empty;
        let inner = if empty {
            scan::Span::new(token.span.end, token.span.end)
        } else {
            scan::Span::new(token.span.end, tokens[cell_close].span.start)
        };
        cells.push(Cell {
            name,
            empty,
            full: scan::Span::new(token.span.start, tokens[cell_close].span.end),
            inner,
            value: if empty {
                String::new()
            } else {
                cell_text(tokens, src, cell_open, cell_close)
            },
        });
    }
    Ok(Row {
        full: scan::Span::new(tokens[open].span.start, tokens[close].span.end),
        cells,
    })
}

fn parse(path: &Path, src: &[u8]) -> Result<Option<Parsed>, LocalizationError> {
    let tokens = match scan::tokenize(src) {
        Ok(tokens) => tokens,
        Err(error) => {
            if src
                .windows(b"LocalizationTables".len())
                .any(|part| part == b"LocalizationTables")
            {
                return Err(invalid(
                    path,
                    format!("cannot scan LocalizationTables XML: {error}; repair the export"),
                ));
            }
            return Ok(None);
        }
    };
    let Some(entities) = tokens.iter().position(|t| {
        matches!(t.kind, scan::Kind::Start | scan::Kind::Empty) && t.name.of(src) == b"Entities"
    }) else {
        return Ok(None);
    };
    // Decide whether this is our kind of export before requiring the rest of an unrelated
    // entity document to be structurally complete. `read` is deliberately an ignorer for every
    // non-localization XML file below the localization root.
    let Some(collection) = tokens
        .iter()
        .enumerate()
        .skip(entities + 1)
        .find_map(|(index, token)| match token.kind {
            scan::Kind::Start | scan::Kind::Empty => Some(Some(index)),
            scan::Kind::End => Some(None),
            _ => None,
        })
        .flatten()
    else {
        return Ok(None);
    };
    if tokens[collection].name.of(src) != b"LocalizationTables" {
        return Ok(None);
    }
    let tables = named_children(&tokens, src, collection, "LocalizationTable", path)?;
    if tables.len() != 1 {
        return Err(invalid(
            path,
            "must contain exactly one LocalizationTable; split or repair the export",
        ));
    }
    let table_open = tables[0];
    let table = attribute(&tokens, src, table_open, "name", path)?.ok_or_else(|| {
        invalid(
            path,
            "LocalizationTable has no name attribute; repair the export",
        )
    })?;
    let header = Header {
        description: attribute(&tokens, src, table_open, "description", path)?,
        language_common: attribute(&tokens, src, table_open, "languageCommon", path)?,
        language_native: attribute(&tokens, src, table_open, "languageNative", path)?,
    };
    let configs = named_children(&tokens, src, table_open, "ConfigurationTables", path)?;
    let mut localizations = Vec::new();
    for configs_open in configs {
        for config in named_children(&tokens, src, configs_open, "ConfigurationTable", path)? {
            if attribute(&tokens, src, config, "name", path)?.as_deref()
                == Some("LocalizationTokens")
            {
                localizations.push(config);
            }
        }
    }
    if localizations.len() != 1 {
        return Err(invalid(
            path,
            "has no unambiguous ConfigurationTable named LocalizationTokens; repair the export",
        ));
    }
    let config = localizations[0];
    let row_containers = named_children(&tokens, src, config, "Rows", path)?;
    let (rows_open, rows_close, rows_empty, rows) = if let Some(&rows_open) = row_containers.first()
    {
        let rows_close = element_end(&tokens, src, rows_open, path)?;
        let mut rows = Vec::new();
        if tokens[rows_open].kind != scan::Kind::Empty {
            for row in named_children(&tokens, src, rows_open, "Row", path)? {
                let parsed = parse_row(&tokens, src, row, path)?;
                if !parsed.cells.iter().any(|cell| cell.name == "name") {
                    return Err(invalid(
                        path,
                        "has a LocalizationTokens Row without a name cell; repair the export",
                    ));
                }
                rows.push(parsed);
            }
        }
        (
            Some(rows_open),
            Some(rows_close),
            tokens[rows_open].kind == scan::Kind::Empty,
            rows,
        )
    } else {
        (None, None, false, Vec::new())
    };
    Ok(Some(Parsed {
        tokens,
        table,
        header,
        rows_open,
        rows_close,
        rows_empty,
        rows,
    }))
}

/// Read one localization export. Non-localization XML is ignored; a localization export whose
/// table shape is incomplete is refused with its path so it can be repaired by hand.
pub fn read(path: &Path, src: &[u8]) -> Result<Option<TableFile>, LocalizationError> {
    let Some(parsed) = parse(path, src)? else {
        return Ok(None);
    };
    let tokens = parsed
        .rows
        .iter()
        .map(|row| Token {
            name: row
                .cells
                .iter()
                .find(|c| c.name == "name")
                .map(|c| c.value.clone())
                .unwrap_or_default(),
            value: row
                .cells
                .iter()
                .find(|c| c.name == "value")
                .map(|c| c.value.clone())
                .unwrap_or_default(),
            usage: row
                .cells
                .iter()
                .find(|c| c.name == "usage")
                .map(|c| c.value.clone())
                .unwrap_or_default(),
            context: row
                .cells
                .iter()
                .find(|c| c.name == "context")
                .map(|c| c.value.clone())
                .unwrap_or_default(),
        })
        .collect();
    Ok(Some(TableFile {
        path: path.to_path_buf(),
        table: parsed.table,
        header: parsed.header,
        tokens,
    }))
}

/// Find localization exports recursively. A missing root is an empty solution, while malformed
/// table exports are returned in `unreadable` for a caller to report without losing good files.
pub fn discover(root: &Path) -> Result<Discovered, LocalizationError> {
    if !root.exists() {
        return Ok(Discovered::default());
    }
    let mut paths = Vec::new();
    fn visit(dir: &Path, paths: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                visit(&path, paths)?;
            } else if path
                .extension()
                .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("xml"))
            {
                paths.push(path);
            }
        }
        Ok(())
    }
    visit(root, &mut paths).map_err(|e| LocalizationError::Io {
        path: root.to_path_buf(),
        why: e.to_string(),
    })?;
    paths.sort();
    let mut found = Discovered::default();
    for path in paths {
        match std::fs::read(&path) {
            Ok(src) => match read(&path, &src) {
                Ok(Some(file)) => found.files.push(file),
                Ok(None) => {}
                Err(error) => found.unreadable.push((path, error.to_string())),
            },
            Err(error) => found.unreadable.push((path, error.to_string())),
        }
    }
    Ok(found)
}

fn desired<'a>(token: &'a Token, field: &str) -> &'a str {
    match field {
        "name" => &token.name,
        "value" => &token.value,
        "usage" => &token.usage,
        "context" => &token.context,
        _ => "",
    }
}

fn whitespace_before(src: &[u8], at: usize) -> &[u8] {
    let mut start = at;
    while start > 0 && src[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    &src[start..at]
}

fn plan_cell(
    path: &Path,
    src: &[u8],
    tokens: &[scan::Token],
    cell: &Cell,
    token_name: &str,
    value: &str,
) -> Result<Option<splice::Edit>, LocalizationError> {
    if cell.value.trim() == value.trim() {
        return Ok(None);
    }
    if cell.empty {
        let mut replacement = Vec::new();
        replacement.extend_from_slice(b"<");
        replacement.extend_from_slice(cell.name.as_bytes());
        replacement.extend_from_slice(b">");
        replacement.extend(scan::render_cdata(value.as_bytes()));
        replacement.extend_from_slice(b"</");
        replacement.extend_from_slice(cell.name.as_bytes());
        replacement.extend_from_slice(b">");
        return Ok(Some(splice::Edit::new(cell.full, replacement)));
    }
    let inside: Vec<&scan::Token> = tokens
        .iter()
        .filter(|t| t.span.start >= cell.inner.start && t.span.end <= cell.inner.end)
        .collect();
    let cdata: Vec<&scan::Token> = inside
        .iter()
        .copied()
        .filter(|t| t.kind == scan::Kind::Cdata)
        .collect();
    let text_only = inside.iter().all(|t| t.kind == scan::Kind::Text);
    let one_cdata_with_space = cdata.len() == 1
        && inside.iter().all(|t| {
            t.kind == scan::Kind::Cdata
                || (t.kind == scan::Kind::Text
                    && t.span.of(src).iter().all(u8::is_ascii_whitespace))
        });
    if one_cdata_with_space {
        return Ok(Some(splice::Edit::new(
            cdata[0].span,
            scan::render_cdata(value.as_bytes()),
        )));
    }
    if cdata.is_empty() && text_only {
        return Ok(Some(splice::Edit::new(
            cell.inner,
            scan::render_cdata(value.as_bytes()),
        )));
    }
    Err(invalid(
        path,
        format!(
            "token {token_name} has a <{}> cell holding markup twaco will not replace; edit it by hand",
            cell.name
        ),
    ))
}

fn plan_set_row(
    path: &Path,
    src: &[u8],
    tokens: &[scan::Token],
    row: &Row,
    token: &Token,
) -> Result<Vec<splice::Edit>, LocalizationError> {
    let mut changes = Vec::new();
    let mut missing = Vec::new();
    for field in ["context", "name", "usage", "value"] {
        if let Some(cell) = row.cells.iter().find(|c| c.name == field) {
            if let Some(change) =
                plan_cell(path, src, tokens, cell, &token.name, desired(token, field))?
            {
                changes.push(change);
            }
        } else if !desired(token, field).is_empty() {
            // A missing cell reads as empty, so an empty value needs no cell.
            missing.push(field);
        }
    }
    if !missing.is_empty() {
        let Some(last) = row.cells.last() else {
            return Err(invalid(
                path,
                format!("token {} has no cells to copy indentation from", token.name),
            ));
        };
        let lead = whitespace_before(src, last.full.start);
        let mut bytes = Vec::new();
        for field in missing {
            bytes.extend_from_slice(lead);
            bytes.extend_from_slice(b"<");
            bytes.extend_from_slice(field.as_bytes());
            bytes.extend_from_slice(b">");
            bytes.extend(scan::render_cdata(desired(token, field).as_bytes()));
            bytes.extend_from_slice(b"</");
            bytes.extend_from_slice(field.as_bytes());
            bytes.extend_from_slice(b">");
        }
        changes.push(splice::Edit::new(
            scan::Span::new(last.full.end, last.full.end),
            bytes,
        ));
    }
    Ok(changes)
}

fn templated_row(
    path: &Path,
    src: &[u8],
    row: &Row,
    token: &Token,
) -> Result<Vec<u8>, LocalizationError> {
    let bytes = row.full.of(src).to_vec();
    let tokens = scan::tokenize(&bytes)
        .map_err(|e| invalid(path, format!("cannot scan row template: {e}")))?;
    let row_open = tokens
        .iter()
        .position(|t| t.kind == scan::Kind::Start && t.name.of(&bytes) == b"Row")
        .ok_or_else(|| invalid(path, "cannot find the row template"))?;
    let copy = parse_row(&tokens, &bytes, row_open, path)?;
    let edits = plan_set_row(path, &bytes, &tokens, &copy, token)?;
    splice::splice(&bytes, &edits)
        .map_err(|e| invalid(path, format!("cannot edit row template: {e}")))
}

fn line_indent(src: &[u8], at: usize) -> Vec<u8> {
    let start = src[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    src[start..at]
        .iter()
        .copied()
        .filter(|b| *b == b' ' || *b == b'\t')
        .collect()
}

fn new_row(token: &Token, indent: &[u8], newline: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(indent);
    out.extend_from_slice(b"<Row>");
    out.extend_from_slice(newline);
    for field in ["context", "name", "usage", "value"] {
        out.extend_from_slice(indent);
        out.extend_from_slice(b"    <");
        out.extend_from_slice(field.as_bytes());
        out.extend_from_slice(b">");
        out.extend_from_slice(newline);
        out.extend_from_slice(indent);
        out.extend_from_slice(b"        ");
        out.extend(scan::render_cdata(desired(token, field).as_bytes()));
        out.extend_from_slice(newline);
        out.extend_from_slice(indent);
        out.extend_from_slice(b"    </");
        out.extend_from_slice(field.as_bytes());
        out.extend_from_slice(b">");
        out.extend_from_slice(newline);
    }
    out.extend_from_slice(indent);
    out.extend_from_slice(b"</Row>");
    out
}

/// Apply changes to a localization table without reformatting any existing bytes. Unsupported
/// cell markup, duplicate rows, duplicate edit arguments, and files without `Rows` are refused.
pub fn edit(path: &Path, src: &[u8], edits: &[Edit]) -> Result<Vec<u8>, LocalizationError> {
    let parsed =
        parse(path, src)?.ok_or_else(|| invalid(path, "is not a localization table export"))?;
    let mut requested = BTreeSet::new();
    for edit in edits {
        let name = match edit {
            Edit::Set(token) => &token.name,
            Edit::Remove(name) => name,
        };
        if !requested.insert(name.clone()) {
            return Err(LocalizationError::Arguments(format!(
                "token {name} is edited more than once"
            )));
        }
    }
    let mut by_name: BTreeMap<&str, Vec<&Row>> = BTreeMap::new();
    for row in &parsed.rows {
        let name = row
            .cells
            .iter()
            .find(|c| c.name == "name")
            .map(|c| c.value.as_str())
            .unwrap_or("");
        by_name.entry(name).or_default().push(row);
    }
    for name in &requested {
        if by_name
            .get(name.as_str())
            .is_some_and(|rows| rows.len() > 1)
        {
            return Err(invalid(
                path,
                format!("token {name} occurs more than once; resolve the duplicate by hand"),
            ));
        }
    }
    let mut changes = Vec::new();
    let mut additions = Vec::new();
    for edit in edits {
        match edit {
            Edit::Set(token) => match by_name
                .get(token.name.as_str())
                .and_then(|r| r.first())
                .copied()
            {
                Some(row) => changes.extend(plan_set_row(path, src, &parsed.tokens, row, token)?),
                None => additions.push(token),
            },
            Edit::Remove(name) => {
                if let Some(row) = by_name.get(name.as_str()).and_then(|r| r.first()).copied() {
                    let start = row.full.start - whitespace_before(src, row.full.start).len();
                    changes.push(splice::Edit::new(
                        scan::Span::new(start, row.full.end),
                        Vec::new(),
                    ));
                }
            }
        }
    }
    if !additions.is_empty() {
        let rows_open = parsed
            .rows_open
            .ok_or_else(|| invalid(path, "has no Rows element; add one before adding tokens"))?;
        let rows_close = parsed.rows_close.expect("Rows opening has a closing token");
        let newline = if src.windows(2).any(|w| w == b"\r\n") {
            b"\r\n".as_slice()
        } else {
            b"\n".as_slice()
        };
        if let Some(last) = parsed.rows.last() {
            let mut bytes = Vec::new();
            let lead = whitespace_before(src, last.full.start);
            for token in additions {
                bytes.extend_from_slice(lead);
                bytes.extend(templated_row(path, src, last, token)?);
            }
            changes.push(splice::Edit::new(
                scan::Span::new(last.full.end, last.full.end),
                bytes,
            ));
        } else {
            let rows_indent = line_indent(src, parsed.tokens[rows_open].span.start);
            let mut row_indent = rows_indent.clone();
            row_indent.extend_from_slice(b"    ");
            let mut bytes = Vec::new();
            for token in additions {
                bytes.extend_from_slice(newline);
                bytes.extend(new_row(token, &row_indent, newline));
            }
            bytes.extend_from_slice(newline);
            bytes.extend_from_slice(&rows_indent);
            if parsed.rows_empty {
                let opening = parsed.tokens[rows_open].span.of(src);
                let mut replacement = opening[..opening.len() - 2].to_vec();
                replacement.extend_from_slice(b">");
                replacement.extend_from_slice(&bytes);
                replacement.extend_from_slice(b"</Rows>");
                changes.push(splice::Edit::new(
                    parsed.tokens[rows_open].span,
                    replacement,
                ));
            } else {
                changes.push(splice::Edit::new(
                    scan::Span::new(
                        parsed.tokens[rows_open].span.end,
                        parsed.tokens[rows_close].span.start,
                    ),
                    bytes,
                ));
            }
        }
    }
    if changes.is_empty() {
        return Ok(src.to_vec());
    }
    splice::splice(src, &changes)
        .map_err(|e| invalid(path, format!("cannot apply localization edit: {e}")))
}

/// Render a new portable table export. It intentionally omits project and modified-date metadata.
pub fn render(table: &str, header: &Header, tokens: &[Token]) -> Vec<u8> {
    let description = header.description.clone().unwrap_or_else(|| {
        if table == DEFAULT_TABLE {
            "Default localization table".to_string()
        } else {
            format!("{table} localization table")
        }
    });
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities>\n    <LocalizationTables>\n        <LocalizationTable\n");
    out.push_str(&format!(
        "                description=\"{}\"\n",
        scan::escape_attribute(&description)
    ));
    out.push_str("                aspect.isEditableExtensionObject=\"false\"\n                documentationContent=\"\"\n                homeMashup=\"\"\n");
    if let Some(value) = &header.language_common {
        out.push_str(&format!(
            "                languageCommon=\"{}\"\n",
            scan::escape_attribute(value)
        ));
    }
    if let Some(value) = &header.language_native {
        out.push_str(&format!(
            "                languageNative=\"{}\"\n",
            scan::escape_attribute(value)
        ));
    }
    out.push_str(&format!(
        "                name=\"{}\"\n                tags=\"\">\n",
        scan::escape_attribute(table)
    ));
    out.push_str("            <avatar></avatar>\n            <DesignTimePermissions>\n                <Create></Create>\n                <Read></Read>\n                <Update></Update>\n                <Delete></Delete>\n                <Metadata></Metadata>\n            </DesignTimePermissions>\n            <RunTimePermissions></RunTimePermissions>\n            <VisibilityPermissions>\n                <Visibility></Visibility>\n            </VisibilityPermissions>\n            <ConfigurationTableDefinitions></ConfigurationTableDefinitions>\n            <ConfigurationTables>\n                <ConfigurationTable\n                        dataShapeName=\"\"\n                        description=\"Localization tokens and usage\"\n                        isMultiRow=\"true\"\n                        name=\"LocalizationTokens\"\n                        ordinal=\"0\">\n                    <DataShape>\n                        <FieldDefinitions>\n");
    for (friendly, description, name, ordinal) in [
        ("Translation context", "Translation context", "context", "3"),
        ("Token name", "Token name", "name", "0"),
        ("Token usage", "Token usage", "usage", "2"),
        ("Localized value", "Localized value", "value", "1"),
    ] {
        out.push_str(&format!("                            <FieldDefinition\n                                    aspect.friendlyName=\"{friendly}\"\n                                    baseType=\"STRING\"\n                                    description=\"{description}\"\n                                    name=\"{name}\"\n                                    ordinal=\"{ordinal}\"></FieldDefinition>\n"));
    }
    out.push_str("                        </FieldDefinitions>\n                    </DataShape>\n                    <Rows>");
    let mut ordered = tokens.to_vec();
    ordered.sort_by(|a, b| a.name.cmp(&b.name));
    for token in &ordered {
        out.push_str("\n                        <Row>\n");
        for field in ["context", "name", "usage", "value"] {
            out.push_str(&format!("                            <{field}>\n                                {}\n                            </{field}>\n", String::from_utf8_lossy(&scan::render_cdata(desired(token, field).as_bytes()))));
        }
        out.push_str("                        </Row>");
    }
    out.push_str("\n                    </Rows>\n                </ConfigurationTable>\n            </ConfigurationTables>\n        </LocalizationTable>\n    </LocalizationTables>\n</Entities>\n");
    out.into_bytes()
}

/// The conventional filename for a table within one project block.
pub fn file_name(table: &str) -> String {
    if table == DEFAULT_TABLE {
        "LocalizationTable.xml".to_string()
    } else {
        format!("LocalizationTable_{table}.xml")
    }
}

/// Select where a new token belongs without guessing between projects that share a language.
pub fn target(
    files: &[TableFile],
    root: &Path,
    project: &Project,
    prefixes: &[String],
    table: &str,
    single_project: bool,
) -> PathBuf {
    let mut same: Vec<&TableFile> = files.iter().filter(|f| f.table == table).collect();
    same.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(file) = same.iter().find(|file| {
        file.tokens
            .iter()
            .any(|token| prefixes.iter().any(|prefix| token.name.starts_with(prefix)))
    }) {
        return file.path.clone();
    }
    let project_dir = root.join(&project.name);
    if let Some(file) = same
        .iter()
        .find(|file| file.path.parent() == Some(project_dir.as_path()))
    {
        return file.path.clone();
    }
    if single_project && same.len() == 1 {
        return same[0].path.clone();
    }
    project_dir.join(file_name(table))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Same,
    Differs,
    LocalOnly,
    ServerOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compared {
    pub table: String,
    pub name: String,
    pub state: State,
    pub local: Option<Token>,
    pub server: Option<Token>,
    pub file: Option<PathBuf>,
}

fn comparable(token: &Token) -> Token {
    Token {
        name: token.name.trim().to_string(),
        value: token.value.trim().to_string(),
        usage: token.usage.trim().to_string(),
        context: token.context.trim().to_string(),
    }
}

/// Compare the local tokens this solution owns with the server's full tables.
pub fn compare(
    files: &[TableFile],
    server: &BTreeMap<String, Vec<Token>>,
    prefixes: &[String],
) -> Vec<Compared> {
    let mut ordered: Vec<&TableFile> = files.iter().collect();
    ordered.sort_by(|a, b| a.path.cmp(&b.path));
    let mut local: BTreeMap<(String, String), (Token, PathBuf)> = BTreeMap::new();
    let mut tables = BTreeSet::new();
    for file in ordered {
        tables.insert(file.table.clone());
        for token in &file.tokens {
            local
                .entry((file.table.clone(), token.name.clone()))
                .or_insert((token.clone(), file.path.clone()));
        }
    }
    let mut out = Vec::new();
    for table in tables {
        let remote = server.get(&table).cloned().unwrap_or_default();
        let remote_by_name: BTreeMap<String, Token> =
            remote.into_iter().map(|t| (t.name.clone(), t)).collect();
        for ((_, name), (token, path)) in local.iter().filter(|((t, _), _)| t == &table) {
            let server_token = remote_by_name.get(name).cloned();
            let state = match &server_token {
                None => State::LocalOnly,
                Some(other) if comparable(token) == comparable(other) => State::Same,
                Some(_) => State::Differs,
            };
            out.push(Compared {
                table: table.clone(),
                name: name.clone(),
                state,
                local: Some(token.clone()),
                server: server_token,
                file: Some(path.clone()),
            });
        }
        for (name, token) in remote_by_name {
            if !local.contains_key(&(table.clone(), name.clone()))
                && prefixes.iter().any(|prefix| name.starts_with(prefix))
            {
                out.push(Compared {
                    table: table.clone(),
                    name,
                    state: State::ServerOnly,
                    local: None,
                    server: Some(token),
                    file: None,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        (a.table != DEFAULT_TABLE, &a.table, &a.name).cmp(&(
            b.table != DEFAULT_TABLE,
            &b.table,
            &b.name,
        ))
    });
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The same token name in two rows of one table, in one file or across files.
    Duplicate {
        table: String,
        name: String,
        files: Vec<PathBuf>,
    },
    /// A language table's token that no Default file has; the server refuses it.
    NotInDefault {
        table: String,
        name: String,
        file: PathBuf,
    },
    /// A Default token, under a prefix the language table already uses, that the table lacks.
    Untranslated { table: String, name: String },
}

/// Report cross-file table constraints in deterministic table and token order.
pub fn problems(files: &[TableFile], prefixes: &[String]) -> Vec<Problem> {
    let mut grouped: BTreeMap<String, Vec<&TableFile>> = BTreeMap::new();
    for file in files {
        grouped.entry(file.table.clone()).or_default().push(file);
    }
    let mut out = Vec::new();
    for (table, group) in &grouped {
        let mut owners: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for file in group {
            for token in &file.tokens {
                owners
                    .entry(token.name.clone())
                    .or_default()
                    .push(file.path.clone());
            }
        }
        for (name, mut paths) in owners.clone() {
            if paths.len() > 1 {
                paths.sort();
                paths.dedup();
                out.push(Problem::Duplicate {
                    table: table.clone(),
                    name,
                    files: paths,
                });
            }
        }
    }
    let defaults: BTreeSet<String> = grouped
        .get(DEFAULT_TABLE)
        .into_iter()
        .flatten()
        .flat_map(|file| file.tokens.iter().map(|token| token.name.clone()))
        .collect();
    for (table, group) in &grouped {
        if table == DEFAULT_TABLE {
            continue;
        }
        let names: BTreeSet<String> = group
            .iter()
            .flat_map(|file| file.tokens.iter().map(|token| token.name.clone()))
            .collect();
        for file in group {
            for token in &file.tokens {
                if !defaults.contains(&token.name) {
                    out.push(Problem::NotInDefault {
                        table: table.clone(),
                        name: token.name.clone(),
                        file: file.path.clone(),
                    });
                }
            }
        }
        let owns_prefix = names
            .iter()
            .any(|name| prefixes.iter().any(|prefix| name.starts_with(prefix)));
        if owns_prefix {
            for name in &defaults {
                if prefixes.iter().any(|prefix| name.starts_with(prefix)) && !names.contains(name) {
                    out.push(Problem::Untranslated {
                        table: table.clone(),
                        name: name.clone(),
                    });
                }
            }
        }
    }
    out.sort_by_key(problem_key);
    out
}

fn problem_key(problem: &Problem) -> (bool, String, String, u8, String) {
    match problem {
        Problem::Duplicate { table, name, files } => (
            table != DEFAULT_TABLE,
            table.clone(),
            name.clone(),
            0,
            files
                .first()
                .map_or_else(String::new, |p| p.to_string_lossy().into_owned()),
        ),
        Problem::NotInDefault { table, name, file } => (
            table != DEFAULT_TABLE,
            table.clone(),
            name.clone(),
            1,
            file.to_string_lossy().into_owned(),
        ),
        Problem::Untranslated { table, name } => (
            table != DEFAULT_TABLE,
            table.clone(),
            name.clone(),
            2,
            String::new(),
        ),
    }
}

#[cfg(test)]
mod ops_tests;
#[cfg(test)]
mod tests;
