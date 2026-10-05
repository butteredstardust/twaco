//! A Thing's configuration table on a live server: read it, back it up, put it back, and compare
//! it with the entity XML in the repository.
//!
//! Configuration tables hold what a service reads every time it runs, so testing a service that
//! writes one means writing to real rows. The safe routine is back up, test, restore, diff.
//!
//! Restore follows verified platform behavior with throwaway entities:
//! `SetConfigurationTableRows` **upserts by primary key** (it updates matching rows and adds new
//! ones, leaving the rest) and `DeleteConfigurationTableRows` removes rows by key. So restore
//! writes the saved rows first and deletes the extras after. The table is never briefly empty,
//! which matters because open mashups read it. The key comes from the table's own DataShape,
//! never assumed, and a multi-row table without one is refused rather than guessed at.

use super::scan::{self, Kind, Token};
use super::entity_key::ServiceTarget;
use super::server::{Client, ServerError};
use super::sidecar;
use serde_json::{json, Map, Value};
use std::fmt;
use std::path::Path;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(120);

/// A table as the server reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub data_shape: Value,
    pub rows: Vec<Map<String, Value>>,
}

/// The server operations this module needs, as a trait so restore can be tested without one.
pub trait Remote {
    fn call(&self, target: &ServiceTarget, service: &str, parameters: &Value) -> Result<Option<Value>, ServerError>;
}

impl Remote for Client {
    fn call(&self, target: &ServiceTarget, service: &str, parameters: &Value) -> Result<Option<Value>, ServerError> {
        self.call_service(target, service, parameters, TIMEOUT)
    }
}

#[derive(Debug)]
pub enum TableError {
    Remote(ServerError),
    /// The server answered, but not with an InfoTable.
    Shape(String),
    /// A backup made from another Thing or table.
    WrongBackup { expected: String, found: String },
    /// A table without a primary key cannot have rows matched or removed one by one.
    NoPrimaryKey,
    /// The backup's DataShape declares another primary key than the table on the server.
    KeyMismatch { backup: Vec<String>, server: Vec<String> },
    /// A saved row without a value for a key field, or two saved rows with one key.
    BadKey(String),
    Backup { path: String, why: String },
    Repository(String),
    /// Restore wrote, but the table read back is not the backup.
    NotRestored(Vec<String>),
    /// A write failed after an earlier one succeeded: the table is between the two states.
    PartlyRestored { why: ServerError, left: Vec<String> },
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableError::Remote(error) => write!(f, "{error}"),
            TableError::Shape(why) => write!(f, "unexpected configuration table response: {why}"),
            TableError::WrongBackup { expected, found } => write!(
                f,
                "the backup is of {found}, not {expected}; restoring it here would overwrite the \
                 wrong table"
            ),
            TableError::NoPrimaryKey => write!(
                f,
                "the table's DataShape declares no primary key, and this restore would have to \
                 tell rows apart or remove one, which needs a key; nothing was written"
            ),
            TableError::KeyMismatch { backup, server } => write!(
                f,
                "the backup's primary key is [{}] but the table on the server has [{}]; the \
                 backup is of another version of the table, so nothing was written",
                backup.join(", "),
                server.join(", ")
            ),
            TableError::BadKey(why) => write!(f, "the backup cannot be restored: {why}; nothing was written"),
            TableError::Backup { path, why } => write!(f, "{path}: {why}"),
            TableError::Repository(why) => write!(f, "{why}"),
            TableError::NotRestored(differences) => write!(
                f,
                "the restore was sent, but the table read back differs from the backup in {} \
                 place(s): {}",
                differences.len(),
                differences.join("; ")
            ),
            TableError::PartlyRestored { why, left } => {
                write!(f, "the restore failed part way, after writing: {why}. ")?;
                if left.is_empty() {
                    write!(f, "Read back, the table nevertheless matches the backup")
                } else {
                    write!(
                        f,
                        "Read back, the table differs from the backup in {} place(s): {}",
                        left.len(),
                        left.join("; ")
                    )
                }
            }
        }
    }
}

impl std::error::Error for TableError {}

fn thing_target(thing: &str) -> Result<ServiceTarget, ServerError> {
    ServiceTarget::entity("Things", thing).map_err(ServerError::from)
}

/// Read a table from the server.
pub fn fetch(remote: &dyn Remote, thing: &str, table: &str) -> Result<Table, TableError> {
    let reply = remote
        .call(&thing_target(thing).map_err(TableError::Remote)?, "GetConfigurationTable", &json!({ "tableName": table }))
        .map_err(TableError::Remote)?
        .ok_or_else(|| TableError::Shape("an empty body".to_string()))?;
    let data_shape = reply
        .get("dataShape")
        .cloned()
        .ok_or_else(|| TableError::Shape("no dataShape".to_string()))?;
    let rows = reply
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| TableError::Shape("no rows".to_string()))?
        .iter()
        .map(|row| {
            row.as_object()
                .cloned()
                .ok_or_else(|| TableError::Shape("a row that is not an object".to_string()))
        })
        .collect::<Result<_, _>>()?;
    Ok(Table { data_shape, rows })
}

/// The primary-key fields a DataShape declares, sorted so the key text is stable.
pub fn primary_key(data_shape: &Value) -> Vec<String> {
    let mut key: Vec<String> = data_shape
        .get("fieldDefinitions")
        .and_then(Value::as_object)
        .map(|fields| {
            fields
                .iter()
                .filter(|(_, field)| {
                    field.pointer("/aspects/isPrimaryKey").and_then(Value::as_bool) == Some(true)
                })
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();
    key.sort();
    key
}

/// Reduce a cell to what it means, from either side.
///
/// The server gives typed values (24.0 for a NUMBER, an object for JSON, true for a BOOLEAN)
/// and the XML gives the text that was written ("24.0", the JSON as text, "true"). Text that
/// parses as JSON is compared as JSON, so both spellings agree; other text is compared as itself.
/// A number with no fractional part compares as an integer for backup-format compatibility.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Number(number) => match number.as_f64() {
            Some(float) if float.fract() == 0.0 && float.abs() < 9.0e15 => json!(float as i64),
            _ => value.clone(),
        },
        Value::Array(items) => Value::Array(items.iter().map(normalized).collect()),
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), normalized(v))).collect())
        }
        Value::String(text) => {
            let text = text.trim();
            match serde_json::from_str::<Value>(text) {
                Ok(parsed) if !parsed.is_string() => normalized(&parsed),
                _ => Value::String(text.to_string()),
            }
        }
        other => other.clone(),
    }
}

/// One text form of a cell, for comparing and for printing. Object keys are sorted.
fn comparable(value: Option<&Value>) -> String {
    fn sorted(value: Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut entries: Vec<(String, Value)> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
            }
            Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
            other => other,
        }
    }
    match value.map(normalized).map(sorted) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text,
        Some(other) => other.to_string(),
    }
}

/// One text form of a cell for comparing two copies of the *server's* rows, as a restore does.
///
/// Both sides are the server's own typed JSON, so nothing is coerced: text is compared as it is,
/// untrimmed, and never parsed. Only a number's spelling is set aside (24 and 24.0 agree), and a
/// missing cell, null and empty text agree, because the reply leaves empty cells out.
fn exact(value: Option<&Value>) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Number(number) => match number.as_f64() {
                Some(float) if float.fract() == 0.0 && float.abs() < 9.0e15 => json!(float as i64),
                _ => value.clone(),
            },
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            Value::Object(map) => {
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                Value::Object(entries.into_iter().map(|(k, v)| (k.clone(), canonical(v))).collect())
            }
            other => other.clone(),
        }
    }
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => canonical(other).to_string(),
    }
}

type Cell = fn(Option<&Value>) -> String;

fn key_of(row: &Map<String, Value>, key: &[String], cell: Cell) -> String {
    key.iter().map(|field| cell(row.get(field))).collect::<Vec<_>>().join("\u{1f}")
}

/// Differences between two sets of rows, keyed by primary key, or by position when there is none.
///
/// Cells are compared by meaning, so the entity XML's text agrees with the server's typed values.
pub fn differences(
    left_name: &str,
    left: &[Map<String, Value>],
    right_name: &str,
    right: &[Map<String, Value>],
    key: &[String],
) -> Vec<String> {
    differences_by(left_name, left, right_name, right, key, comparable)
}

fn differences_by(
    left_name: &str,
    left: &[Map<String, Value>],
    right_name: &str,
    right: &[Map<String, Value>],
    key: &[String],
    cell: Cell,
) -> Vec<String> {
    let label = |row_key: &str| row_key.replace('\u{1f}', ", ");
    let mut out = Vec::new();
    type Row<'a> = Option<&'a Map<String, Value>>;
    let pairs: Vec<(String, Row, Row)> = if key.is_empty() {
        (0..left.len().max(right.len()))
            .map(|i| (format!("#{}", i + 1), left.get(i), right.get(i)))
            .collect()
    } else {
        let mut keys: Vec<String> = left.iter().chain(right).map(|row| key_of(row, key, cell)).collect();
        keys.sort();
        keys.dedup();
        keys.into_iter()
            .map(|k| {
                let l = left.iter().find(|row| key_of(row, key, cell) == k);
                let r = right.iter().find(|row| key_of(row, key, cell) == k);
                (label(&k), l, r)
            })
            .collect()
    };
    for (row, l, r) in pairs {
        match (l, r) {
            (Some(_), None) => out.push(format!("row {row}: on {left_name}, missing from {right_name}")),
            (None, Some(_)) => out.push(format!("row {row}: in {right_name}, missing on {left_name}")),
            (Some(l), Some(r)) => {
                let mut columns: Vec<&String> = l.keys().chain(r.keys()).collect();
                columns.sort();
                columns.dedup();
                for column in columns {
                    let (here, there) = (cell(l.get(column)), cell(r.get(column)));
                    if here != there {
                        let cut = |s: &str| s.chars().take(60).collect::<String>();
                        out.push(format!(
                            "row {row}, {column}: {left_name} [{}] {right_name} [{}]",
                            cut(&here),
                            cut(&there)
                        ));
                    }
                }
            }
            (None, None) => {}
        }
    }
    out
}

/// Write a backup in the established JSON format, retained for compatibility. It never
/// overwrites an existing file:
/// a backup written over the one taken before a test is the backup that was needed.
pub fn write_backup(path: &Path, thing: &str, table_name: &str, table: &Table) -> Result<(), TableError> {
    let document = json!({
        "thing": thing,
        "table": table_name,
        "dataShape": table.data_shape,
        "rows": table.rows,
    });
    let mut bytes = serde_json::to_vec_pretty(&document).expect("JSON values serialise");
    bytes.push(b'\n');
    let io = |why: std::io::Error| TableError::Backup { path: path.display().to_string(), why: why.to_string() };
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                TableError::Backup {
                    path: path.display().to_string(),
                    why: "already exists; a backup is never overwritten".to_string(),
                }
            } else {
                io(error)
            }
        })?;
    // A half-written backup must not stay behind: it would be refused as "already exists" by
    // the next attempt, and taken for a good one by a restore.
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(io(error));
    }
    Ok(())
}

/// Read a backup and check it is of this Thing's table.
pub fn read_backup(path: &Path, thing: &str, table_name: &str) -> Result<Table, TableError> {
    let bad = |why: String| TableError::Backup { path: path.display().to_string(), why };
    let text = std::fs::read_to_string(path).map_err(|e| bad(e.to_string()))?;
    let document: Value = serde_json::from_str(&text).map_err(|e| bad(e.to_string()))?;
    let found_thing = document.get("thing").and_then(Value::as_str).unwrap_or("");
    let found_table = document.get("table").and_then(Value::as_str).unwrap_or("");
    if found_thing != thing || found_table != table_name {
        return Err(TableError::WrongBackup {
            expected: format!("{thing}.{table_name}"),
            found: format!("{found_thing}.{found_table}"),
        });
    }
    let data_shape = document.get("dataShape").cloned().ok_or_else(|| bad("no dataShape".to_string()))?;
    let rows = document
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("no rows".to_string()))?
        .iter()
        .map(|row| row.as_object().cloned().ok_or_else(|| bad("a row that is not an object".to_string())))
        .collect::<Result<_, _>>()?;
    Ok(Table { data_shape, rows })
}

/// What a restore changes, computed before anything is written.
#[derive(Debug, PartialEq, Eq)]
pub struct RestorePlan {
    /// Saved rows that differ from, or are missing on, the server.
    pub writes: usize,
    /// Keys of rows on the server that the backup does not hold.
    pub deletes: Vec<String>,
}

/// The key a restore matches rows by: the server's table's own, which the backup must agree with.
fn restore_key(current: &Table, saved: &Table) -> Result<Vec<String>, TableError> {
    let key = primary_key(&current.data_shape);
    let declared = primary_key(&saved.data_shape);
    if declared != key {
        return Err(TableError::KeyMismatch { backup: declared, server: key });
    }
    if !key.is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        for (index, row) in saved.rows.iter().enumerate() {
            if let Some(field) = key.iter().find(|field| exact(row.get(*field)).is_empty()) {
                return Err(TableError::BadKey(format!("saved row #{} has no value for key field {field}", index + 1)));
            }
            let k = key_of(row, &key, exact);
            if !seen.insert(k.clone()) {
                return Err(TableError::BadKey(format!("two saved rows have the key {}", k.replace('\u{1f}', ", "))));
            }
        }
    }
    Ok(key)
}

pub fn plan_restore(current: &Table, saved: &Table) -> Result<RestorePlan, TableError> {
    let key = restore_key(current, saved)?;
    if key.is_empty() {
        let same = differences_by("server", &current.rows, "backup", &saved.rows, &key, exact).is_empty();
        if same {
            return Ok(RestorePlan { writes: 0, deletes: Vec::new() });
        }
        // Without a key, only a single row written over at most one row is unambiguous. A row to
        // remove cannot be named, so an empty backup over a filled table is refused too.
        if saved.rows.len() != 1 || current.rows.len() > 1 {
            return Err(TableError::NoPrimaryKey);
        }
        return Ok(RestorePlan { writes: 1, deletes: Vec::new() });
    }
    let writes = saved
        .rows
        .iter()
        .filter(|row| {
            let k = key_of(row, &key, exact);
            match current.rows.iter().find(|c| key_of(c, &key, exact) == k) {
                Some(existing) => !differences_by(
                    "server",
                    std::slice::from_ref(existing),
                    "backup",
                    std::slice::from_ref(row),
                    &key,
                    exact,
                )
                .is_empty(),
                None => true,
            }
        })
        .count();
    let saved_keys: Vec<String> = saved.rows.iter().map(|row| key_of(row, &key, exact)).collect();
    let deletes = current
        .rows
        .iter()
        .map(|row| key_of(row, &key, exact))
        .filter(|k| !saved_keys.contains(k))
        .collect();
    Ok(RestorePlan { writes, deletes })
}

/// How the table read back differs from the backup, by cell and by row count.
fn left_over(after: &Table, saved: &Table, key: &[String]) -> Vec<String> {
    let mut left = differences_by("server", &after.rows, "backup", &saved.rows, key, exact);
    if after.rows.len() != saved.rows.len() {
        left.push(format!("the server holds {} row(s), the backup {}", after.rows.len(), saved.rows.len()));
    }
    left
}

/// Put a table back as the backup holds it, then read it back and prove it.
///
/// `apply` false stops after the plan. Writing the saved rows before deleting the extras keeps
/// the table populated throughout.
pub fn restore(
    remote: &dyn Remote,
    thing: &str,
    table_name: &str,
    saved: &Table,
    apply: bool,
) -> Result<RestorePlan, TableError> {
    let current = fetch(remote, thing, table_name)?;
    let plan = plan_restore(&current, saved)?;
    if !apply || (plan.writes == 0 && plan.deletes.is_empty()) {
        return Ok(plan);
    }
    let target = thing_target(thing).map_err(TableError::Remote)?;
    let key = primary_key(&current.data_shape);
    let mut calls: Vec<Value> = Vec::new();
    if plan.writes > 0 {
        calls.push(json!({
            "service": "SetConfigurationTableRows",
            "tableName": table_name,
            "persistent": true,
            "values": { "dataShape": saved.data_shape, "rows": saved.rows },
        }));
    }
    for row in current.rows.iter().filter(|row| plan.deletes.contains(&key_of(row, &key, exact))) {
        let key_values: Map<String, Value> =
            key.iter().map(|field| (field.clone(), row.get(field).cloned().unwrap_or(Value::Null))).collect();
        calls.push(json!({
            "service": "DeleteConfigurationTableRows",
            "tableName": table_name,
            "persistent": true,
            "removeAllMatchingRows": true,
            "values": { "dataShape": current.data_shape, "rows": [key_values] },
        }));
    }
    for (index, mut parameters) in calls.into_iter().enumerate() {
        let service = parameters.as_object_mut().and_then(|p| p.remove("service"));
        let service = service.as_ref().and_then(Value::as_str).expect("every call names its service");
        if let Err(why) = remote.call(&target, service, &parameters) {
            if index == 0 {
                return Err(TableError::Remote(why));
            }
            // Something was written already. Say where the table was left, if it can be read.
            let left = fetch(remote, thing, table_name)
                .map(|after| left_over(&after, saved, &key))
                .unwrap_or_else(|error| vec![format!("it could not be read back: {error}")]);
            return Err(TableError::PartlyRestored { why, left });
        }
    }
    let after = fetch(remote, thing, table_name)?;
    let left = left_over(&after, saved, &key);
    if left.is_empty() {
        Ok(plan)
    } else {
        Err(TableError::NotRestored(left))
    }
}

/// The rows of a Thing's own configuration table, as the entity XML holds them.
///
/// Only the entity's own `ConfigurationTables` count: a service implementation carries a
/// `ConfigurationTables` of its own (its `Script` table), which is not the Thing's.
pub fn repository_rows(src: &[u8], table_name: &str) -> Result<Vec<Map<String, Value>>, TableError> {
    let bad = |why: String| TableError::Repository(why);
    let tokens = scan::tokenize(src).map_err(|e| bad(e.to_string()))?;
    let entity = sidecar::entity_element(&tokens, src).ok_or_else(|| bad("not an entity document".to_string()))?;
    let table = scan::child_tags(&tokens, src, "ConfigurationTables", entity)
        .into_iter()
        .flat_map(|tables| scan::child_tags(&tokens, src, "ConfigurationTable", tables))
        .find(|&index| {
            matches!(scan::attribute(src, &tokens[index], "name"), Ok(Some(v)) if v.of(src) == table_name.as_bytes())
        })
        .ok_or_else(|| bad(format!("the entity XML has no configuration table called {table_name}")))?;
    let mut rows = Vec::new();
    for container in scan::child_tags(&tokens, src, "Rows", table) {
        for row in scan::child_tags(&tokens, src, "Row", container) {
            rows.push(row_cells(&tokens, src, row));
        }
    }
    Ok(rows)
}

/// Each child of a row, with all the text under it: a JSON-typed value sits one element deeper.
fn row_cells(tokens: &[Token], src: &[u8], row: usize) -> Map<String, Value> {
    let mut cells = Map::new();
    let Some(end) = scan::element_end(tokens, row) else { return cells };
    let mut index = row + 1;
    while index < end {
        let token = &tokens[index];
        match token.kind {
            Kind::Start => {
                let name = String::from_utf8_lossy(token.name.of(src)).into_owned();
                let close = scan::element_end(tokens, index).unwrap_or(end);
                let text: String = (index + 1..close)
                    .filter_map(|i| match tokens[i].kind {
                        Kind::Cdata => Some(String::from_utf8_lossy(tokens[i].inner.of(src)).into_owned()),
                        Kind::Text => {
                            Some(scan::decode_entities(&String::from_utf8_lossy(tokens[i].span.of(src))))
                        }
                        _ => None,
                    })
                    .collect();
                cells.insert(name, Value::String(text.trim().to_string()));
                index = close + 1;
            }
            Kind::Empty => {
                cells.insert(String::from_utf8_lossy(token.name.of(src)).into_owned(), Value::String(String::new()));
                index += 1;
            }
            _ => index += 1,
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn shape(key: &[&str]) -> Value {
        let mut fields = Map::new();
        for name in ["Key", "Value", "Size"] {
            fields.insert(
                name.to_string(),
                json!({ "name": name, "aspects": { "isPrimaryKey": key.contains(&name) } }),
            );
        }
        json!({ "fieldDefinitions": fields })
    }

    fn row(key: &str, value: Value) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("Key".to_string(), json!(key));
        map.insert("Value".to_string(), value);
        map
    }

    /// A table that behaves as the platform was observed to: Set upserts by key, Delete removes
    /// by key. It records every call so tests can assert order and absence.
    struct Fake {
        table: RefCell<Table>,
        calls: RefCell<Vec<String>>,
        ignore_set: bool,
        append_set: bool,
        fail_delete: bool,
    }

    impl Fake {
        fn with(rows: Vec<Map<String, Value>>) -> Self {
            Fake {
                table: RefCell::new(Table { data_shape: shape(&["Key"]), rows }),
                calls: RefCell::new(Vec::new()),
                ignore_set: false,
                append_set: false,
                fail_delete: false,
            }
        }
    }

    impl Remote for Fake {
        fn call(&self, _: &ServiceTarget, service: &str, parameters: &Value) -> Result<Option<Value>, ServerError> {
            self.calls.borrow_mut().push(service.to_string());
            let mut table = self.table.borrow_mut();
            match service {
                "GetConfigurationTable" => Ok(Some(json!({ "dataShape": table.data_shape, "rows": table.rows }))),
                "SetConfigurationTableRows" if self.append_set => {
                    for new in parameters["values"]["rows"].as_array().unwrap() {
                        table.rows.push(new.as_object().unwrap().clone());
                    }
                    Ok(None)
                }
                "SetConfigurationTableRows" if !self.ignore_set => {
                    for new in parameters["values"]["rows"].as_array().unwrap() {
                        let new = new.as_object().unwrap().clone();
                        match table.rows.iter_mut().find(|r| r["Key"] == new["Key"]) {
                            Some(existing) => *existing = new,
                            None => table.rows.push(new),
                        }
                    }
                    Ok(None)
                }
                "SetConfigurationTableRows" => Ok(None),
                "DeleteConfigurationTableRows" if self.fail_delete => {
                    Err(ServerError::InvalidUrl("connection reset".to_string()))
                }
                "DeleteConfigurationTableRows" => {
                    let gone = parameters["values"]["rows"][0]["Key"].clone();
                    table.rows.retain(|r| r["Key"] != gone);
                    Ok(None)
                }
                other => panic!("unexpected service {other}"),
            }
        }
    }

    #[test]
    fn the_primary_key_comes_from_the_data_shape() {
        assert_eq!(primary_key(&shape(&["Key"])), ["Key"]);
        assert_eq!(primary_key(&shape(&["Value", "Key"])), ["Key", "Value"]);
        assert!(primary_key(&shape(&[])).is_empty());
    }

    #[test]
    fn typed_server_values_equal_their_xml_text() {
        assert_eq!(comparable(Some(&json!(24.0))), comparable(Some(&json!("24.0"))));
        assert_eq!(comparable(Some(&json!(true))), comparable(Some(&json!("true"))));
        assert_eq!(
            comparable(Some(&json!({"b": 1, "a": [2.0]}))),
            comparable(Some(&json!(" {\"a\":[2],\"b\":1} ")))
        );
        assert_ne!(comparable(Some(&json!("small"))), comparable(Some(&json!("large"))));
        assert_eq!(comparable(None), comparable(Some(&json!(""))));
    }

    #[test]
    fn restore_upserts_then_deletes_extras_and_proves_it() {
        let saved = Table { data_shape: shape(&["Key"]), rows: vec![row("a", json!("1")), row("b", json!("2"))] };
        let fake = Fake::with(vec![row("a", json!("1")), row("b", json!("20")), row("c", json!("3"))]);
        let plan = restore(&fake, "T", "CT", &saved, true).unwrap();
        assert_eq!(plan, RestorePlan { writes: 1, deletes: vec!["c".to_string()] });
        assert_eq!(
            *fake.calls.borrow(),
            ["GetConfigurationTable", "SetConfigurationTableRows", "DeleteConfigurationTableRows", "GetConfigurationTable"]
        );
        assert_eq!(fake.table.borrow().rows, saved.rows);
    }

    #[test]
    fn a_dry_run_restore_only_reads() {
        let saved = Table { data_shape: shape(&["Key"]), rows: vec![row("a", json!("1"))] };
        let fake = Fake::with(vec![row("a", json!("9")), row("z", json!("0"))]);
        let plan = restore(&fake, "T", "CT", &saved, false).unwrap();
        assert_eq!(plan, RestorePlan { writes: 1, deletes: vec!["z".to_string()] });
        assert_eq!(*fake.calls.borrow(), ["GetConfigurationTable"]);
    }

    #[test]
    fn an_already_matching_table_is_not_written() {
        let saved = Table { data_shape: shape(&["Key"]), rows: vec![row("a", json!("1"))] };
        let fake = Fake::with(vec![row("a", json!(1.0))]);
        restore(&fake, "T", "CT", &saved, true).unwrap();
        assert_eq!(*fake.calls.borrow(), ["GetConfigurationTable"]);
    }

    #[test]
    fn a_restore_the_server_did_not_take_is_reported() {
        let saved = Table { data_shape: shape(&["Key"]), rows: vec![row("a", json!("1"))] };
        let mut fake = Fake::with(vec![row("a", json!("9"))]);
        fake.ignore_set = true;
        let error = restore(&fake, "T", "CT", &saved, true).unwrap_err();
        assert!(matches!(error, TableError::NotRestored(_)), "{error}");
    }

    #[test]
    fn a_multi_row_table_without_a_key_is_refused_before_any_write() {
        let saved = Table { data_shape: shape(&[]), rows: vec![row("a", json!("1")), row("b", json!("2"))] };
        let fake = Fake::with(vec![row("a", json!("1"))]);
        fake.table.borrow_mut().data_shape = shape(&[]);
        let error = restore(&fake, "T", "CT", &saved, true).unwrap_err();
        assert!(matches!(error, TableError::NoPrimaryKey));
        assert_eq!(*fake.calls.borrow(), ["GetConfigurationTable"]);
    }

    #[test]
    fn an_empty_backup_over_a_filled_keyless_table_is_refused() {
        let saved = Table { data_shape: shape(&[]), rows: vec![] };
        let fake = Fake::with(vec![row("a", json!("1"))]);
        fake.table.borrow_mut().data_shape = shape(&[]);
        let error = restore(&fake, "T", "CT", &saved, true).unwrap_err();
        assert!(matches!(error, TableError::NoPrimaryKey), "{error}");
        assert_eq!(*fake.calls.borrow(), ["GetConfigurationTable"]);
    }

    #[test]
    fn the_key_is_the_servers_and_a_backup_declaring_another_is_refused() {
        let saved = Table { data_shape: shape(&["Value"]), rows: vec![row("a", json!("1"))] };
        let fake = Fake::with(vec![row("a", json!("1")), row("b", json!("2"))]);
        let error = restore(&fake, "T", "CT", &saved, true).unwrap_err();
        assert!(matches!(error, TableError::KeyMismatch { .. }), "{error}");
        assert_eq!(*fake.calls.borrow(), ["GetConfigurationTable"]);
    }

    #[test]
    fn saved_rows_with_a_missing_or_repeated_key_are_refused() {
        let fake = Fake::with(vec![row("a", json!("1"))]);
        let repeated = Table { data_shape: shape(&["Key"]), rows: vec![row("a", json!("1")), row("a", json!("2"))] };
        assert!(matches!(restore(&fake, "T", "CT", &repeated, true), Err(TableError::BadKey(_))));
        let mut keyless_row = Map::new();
        keyless_row.insert("Value".to_string(), json!("1"));
        let missing = Table { data_shape: shape(&["Key"]), rows: vec![keyless_row] };
        assert!(matches!(restore(&fake, "T", "CT", &missing, true), Err(TableError::BadKey(_))));
        assert!(fake.calls.borrow().iter().all(|call| call == "GetConfigurationTable"));
    }

    #[test]
    fn a_restore_compares_text_exactly() {
        // "1" and "1.0" are two keys, and " x" is not "x": the loose comparison --diff needs
        // would merge them, and a restore would skip a write it owed.
        let saved = Table {
            data_shape: shape(&["Key"]),
            rows: vec![row("1", json!(" x")), row("1.0", json!("y"))],
        };
        let fake = Fake::with(vec![row("1", json!("x"))]);
        let plan = restore(&fake, "T", "CT", &saved, true).unwrap();
        assert_eq!(plan, RestorePlan { writes: 2, deletes: vec![] });
        assert_eq!(fake.table.borrow().rows, saved.rows);
    }

    #[test]
    fn extra_rows_read_back_are_reported() {
        let saved = Table { data_shape: shape(&["Key"]), rows: vec![row("a", json!("1"))] };
        let mut fake = Fake::with(vec![row("a", json!("9"))]);
        fake.append_set = true;
        let error = restore(&fake, "T", "CT", &saved, true).unwrap_err();
        match error {
            TableError::NotRestored(left) => assert!(left.iter().any(|l| l.contains("2 row(s)")), "{left:?}"),
            other => panic!("{other}"),
        }
    }

    #[test]
    fn a_failure_after_a_write_says_where_the_table_was_left() {
        let saved = Table { data_shape: shape(&["Key"]), rows: vec![row("a", json!("1"))] };
        let mut fake = Fake::with(vec![row("a", json!("9")), row("z", json!("0"))]);
        fake.fail_delete = true;
        let error = restore(&fake, "T", "CT", &saved, true).unwrap_err();
        match &error {
            TableError::PartlyRestored { left, .. } => assert!(left.iter().any(|l| l.contains("row z")), "{left:?}"),
            other => panic!("{other}"),
        }
        assert_eq!(
            *fake.calls.borrow(),
            ["GetConfigurationTable", "SetConfigurationTableRows", "DeleteConfigurationTableRows", "GetConfigurationTable"]
        );
    }

    #[test]
    fn a_backup_of_another_table_is_refused_and_one_is_never_overwritten() {
        let dir = std::env::temp_dir().join(format!(
            "twaco-ct-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("backup.json");
        let table = Table { data_shape: shape(&["Key"]), rows: vec![row("a", json!("1"))] };
        write_backup(&path, "T", "CT", &table).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(write_backup(&path, "T", "CT", &table), Err(TableError::Backup { .. })));
        assert_eq!(std::fs::read(&path).unwrap(), before, "the first backup survives");

        assert_eq!(read_backup(&path, "T", "CT").unwrap(), table);
        assert!(matches!(read_backup(&path, "T", "Other"), Err(TableError::WrongBackup { .. })));
        assert!(matches!(read_backup(&path, "U", "CT"), Err(TableError::WrongBackup { .. })));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn repository_rows_come_from_the_things_own_table_only() {
        let src = br#"<Entities><Things><Thing name="T">
            <ConfigurationTables>
              <ConfigurationTable name="CT"><Rows>
                <Row><Key><![CDATA[
                    a
                ]]></Key><Value><json><![CDATA[{"x":1}]]></json></Value></Row>
                <Row><Key>b &amp; c</Key><Value/></Row>
              </Rows></ConfigurationTable>
            </ConfigurationTables>
            <ThingShape><ServiceImplementations><ServiceImplementation name="S">
              <ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code>x</code></Row></Rows>
              </ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape>
            </Thing></Things></Entities>"#;
        let rows = repository_rows(src, "CT").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["Key"], json!("a"));
        assert_eq!(rows[0]["Value"], json!("{\"x\":1}"));
        assert_eq!(rows[1]["Key"], json!("b & c"));
        assert_eq!(rows[1]["Value"], json!(""));
        assert!(matches!(repository_rows(src, "Script"), Err(TableError::Repository(_))));
    }

    #[test]
    fn differences_are_keyed_and_named() {
        let key = vec!["Key".to_string()];
        let server = vec![row("a", json!(1.0)), row("b", json!("x"))];
        let repo = vec![row("a", json!("1")), row("c", json!("y"))];
        let found = differences("server", &server, "source control", &repo, &key);
        assert_eq!(
            found,
            [
                "row b: on server, missing from source control",
                "row c: in source control, missing on server",
            ]
        );
    }
}
