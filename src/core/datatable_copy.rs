//! Copy the rows of one DataTable into another, typically after a rename created a new table (and
//! perhaps a new DataShape with renamed fields). A rename moves the definition; the rows live on the
//! server and are not part of the XML, so this carries them.
//!
//! Rows are read with `GetDataTableEntries` and written with `AddDataTableEntries`, both verified on
//! a live server: the read returns the key and system columns (`key`, `location`, `source`,
//! `sourceType`, `tags`, `timestamp`) beside the shape's fields, and the write takes an InfoTable of
//! the shape's fields. Only the shape's fields are copied; the per-row system columns are not (the
//! write stamps the caller and the time). Nothing is written without `apply`, a non-empty target is
//! refused unless `append`, and the result is read back and compared.

use super::config::Solution;
use super::entity_key::ServiceTarget;
use super::server::{Client, ServerError};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

/// Columns the platform adds to every DataTable row; they are not part of the shape.
pub const SYSTEM_COLUMNS: [&str; 6] = [
    "key",
    "location",
    "source",
    "sourceType",
    "tags",
    "timestamp",
];

const BATCH: usize = 200;

pub trait Remote {
    fn exists(&self, table: &str) -> Result<bool, ServerError>;
    fn count(&self, table: &str) -> Result<u64, ServerError>;
    /// An InfoTable `{dataShape, rows}` of up to `max_items` entries.
    fn entries(&self, table: &str, max_items: u64) -> Result<Value, ServerError>;
    /// Add the rows of an InfoTable `{dataShape, rows}`.
    fn add(&self, table: &str, rows: &Value) -> Result<(), ServerError>;
}

impl Remote for Client {
    fn exists(&self, table: &str) -> Result<bool, ServerError> {
        self.entity_exists("Things", table)
    }

    fn count(&self, table: &str) -> Result<u64, ServerError> {
        let target = ServiceTarget::entity("Things", table)?;
        let value = self.call_service(
            &target,
            "GetDataTableEntryCount",
            &json!({}),
            Duration::from_secs(120),
        )?;
        value
            .as_ref()
            .and_then(|value| value.get("rows"))
            .and_then(|rows| rows.get(0))
            .and_then(|row| row.get("result"))
            .and_then(Value::as_f64)
            .map(|count| count as u64)
            .ok_or_else(|| ServerError::InvalidResponse {
                url: table.to_string(),
                why: "GetDataTableEntryCount returned no count".to_string(),
            })
    }

    fn entries(&self, table: &str, max_items: u64) -> Result<Value, ServerError> {
        let target = ServiceTarget::entity("Things", table)?;
        self.call_service(
            &target,
            "GetDataTableEntries",
            &json!({ "maxItems": max_items }),
            Duration::from_secs(300),
        )?
        .ok_or_else(|| ServerError::InvalidResponse {
            url: table.to_string(),
            why: "GetDataTableEntries returned nothing".to_string(),
        })
    }

    fn add(&self, table: &str, rows: &Value) -> Result<(), ServerError> {
        let target = ServiceTarget::entity("Things", table)?;
        self.call_service(
            &target,
            "AddDataTableEntries",
            &json!({ "values": rows }),
            Duration::from_secs(300),
        )?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum CopyError {
    Arguments(String),
    Ledger(String),
    Refused(String),
    Server(ServerError),
}

impl fmt::Display for CopyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CopyError::Arguments(why) | CopyError::Refused(why) => f.write_str(why),
            CopyError::Ledger(why) => write!(f, "{why}; fix or remove .twaco/renames.json"),
            CopyError::Server(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CopyError {}

impl From<ServerError> for CopyError {
    fn from(error: ServerError) -> Self {
        CopyError::Server(error)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub old: String,
    pub new: String,
    /// Explicit `old -> new` field names, taking precedence over everything else.
    pub map: BTreeMap<String, String>,
    /// Allow source fields that have no target to be left behind.
    pub drop_unmapped: bool,
    /// Allow a target that already has rows.
    pub append: bool,
    pub max_rows: u64,
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FieldMove {
    pub from: String,
    pub to: String,
    /// `same name`, `ledger` or `--map`.
    pub by: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub old: String,
    pub new: String,
    pub applied: bool,
    pub source_rows: u64,
    pub target_rows_before: u64,
    pub fields: Vec<FieldMove>,
    /// Source fields left behind (only with `drop_unmapped`).
    pub dropped: Vec<String>,
    /// Target fields nothing fills: they keep their default.
    pub unfilled: Vec<String>,
    pub written: u64,
    pub verified: bool,
}

/// `old=new,old=new` into a map.
pub fn parse_map(text: &str) -> Result<BTreeMap<String, String>, CopyError> {
    let mut map = BTreeMap::new();
    for pair in text.split(',').filter(|pair| !pair.trim().is_empty()) {
        match pair.split_once('=') {
            Some((from, to)) if !from.trim().is_empty() && !to.trim().is_empty() => {
                if map
                    .insert(from.trim().to_string(), to.trim().to_string())
                    .is_some()
                {
                    return Err(CopyError::Arguments(format!(
                        "--map names {} twice",
                        from.trim()
                    )));
                }
            }
            _ => {
                return Err(CopyError::Arguments(format!(
                    "--map wants old=new pairs, got {pair:?}"
                )))
            }
        }
    }
    Ok(map)
}

/// The `(old, new)` field renames the ledger records.
fn ledger_fields(solution: &Solution) -> Result<Vec<(String, String)>, CopyError> {
    let ledger = super::ledger::Ledger::read(&solution.root.join(super::ledger::RELATIVE_PATH))
        .map_err(|error| CopyError::Ledger(error.to_string()))?;
    Ok(ledger
        .0
        .iter()
        .filter(|record| record.kind == super::ledger::Kind::Field)
        .map(|record| (record.old.clone(), record.new.clone()))
        .collect())
}

/// Field name to base type, without the platform's system columns.
fn shape_fields(entries: &Value) -> BTreeMap<String, String> {
    entries["dataShape"]["fieldDefinitions"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| !SYSTEM_COLUMNS.contains(&name.as_str()))
        .map(|(name, definition)| {
            (
                name.clone(),
                definition["baseType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Compare numbers as numbers: the platform writes `2` and reads `2.0`.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number
            .as_f64()
            .map_or_else(|| number.to_string(), |float| format!("{float}"))),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), canonical(item)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

fn row_key(row: &Map<String, Value>) -> String {
    canonical(&Value::Object(row.clone())).to_string()
}

fn sorted_keys(rows: &[Map<String, Value>]) -> Vec<String> {
    let mut keys: Vec<String> = rows.iter().map(row_key).collect();
    keys.sort();
    keys
}

fn rows_of(entries: &Value, fields: &BTreeSet<&str>) -> Vec<Map<String, Value>> {
    entries["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|row| {
            row.iter()
                .filter(|(name, _)| fields.contains(name.as_str()))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .collect()
}

pub fn run(
    remote: &dyn Remote,
    solution: &Solution,
    request: &Request,
) -> Result<Report, CopyError> {
    if request.old == request.new {
        return Err(CopyError::Arguments(
            "the source and the target are the same DataTable".to_string(),
        ));
    }
    for name in [&request.old, &request.new] {
        if !remote.exists(name)? {
            return Err(CopyError::Refused(format!(
                "{name} is not on the server; deploy it first"
            )));
        }
    }
    let source_rows = remote.count(&request.old)?;
    let target_before = remote.count(&request.new)?;
    if source_rows > request.max_rows {
        return Err(CopyError::Refused(format!(
            "{} has {source_rows} rows, over --max-rows {}",
            request.old, request.max_rows
        )));
    }
    if target_before > 0 && !request.append {
        return Err(CopyError::Refused(format!(
            "{} already has {target_before} row(s); copy with --append to add to them",
            request.new
        )));
    }

    let old_entries = remote.entries(&request.old, source_rows.max(1))?;
    let new_shape_probe = remote.entries(&request.new, 1)?;
    let old_fields = shape_fields(&old_entries);
    let new_fields = shape_fields(&new_shape_probe);
    let ledger = ledger_fields(solution)?;

    for (from, to) in &request.map {
        if !old_fields.contains_key(from) {
            return Err(CopyError::Arguments(format!(
                "--map: {} has no field {from}",
                request.old
            )));
        }
        if !new_fields.contains_key(to) {
            return Err(CopyError::Arguments(format!(
                "--map: {} has no field {to}",
                request.new
            )));
        }
    }
    let mut fields = Vec::new();
    let mut dropped = Vec::new();
    for from in old_fields.keys() {
        let chosen = if let Some(to) = request.map.get(from) {
            Some((to.clone(), "--map"))
        } else if new_fields.contains_key(from) && !request.map.values().any(|to| to == from) {
            Some((from.clone(), "same name"))
        } else {
            ledger
                .iter()
                .find(|(old, new)| old == from && new_fields.contains_key(new))
                .map(|(_, new)| (new.clone(), "ledger"))
        };
        match chosen {
            Some((to, by)) => fields.push(FieldMove {
                from: from.clone(),
                to,
                by,
            }),
            None => dropped.push(from.clone()),
        }
    }
    let mut targets = BTreeSet::new();
    for field in &fields {
        if !targets.insert(field.to.clone()) {
            return Err(CopyError::Refused(format!(
                "two fields of {} would fill {} in {}",
                request.old, field.to, request.new
            )));
        }
        let (from_type, to_type) = (&old_fields[&field.from], &new_fields[&field.to]);
        if from_type != to_type {
            return Err(CopyError::Refused(format!(
                "field {} is {from_type} in {} but {} is {to_type} in {}",
                field.from, request.old, field.to, request.new
            )));
        }
    }
    if !dropped.is_empty() && !request.drop_unmapped {
        return Err(CopyError::Refused(format!(
            "{} has field(s) with no match in {}: {}; name them with --map old=new, or leave them behind with --drop-unmapped",
            request.old,
            request.new,
            dropped.join(", ")
        )));
    }
    let unfilled: Vec<String> = new_fields
        .keys()
        .filter(|name| !targets.contains(name.as_str()))
        .cloned()
        .collect();

    let mut report = Report {
        old: request.old.clone(),
        new: request.new.clone(),
        applied: false,
        source_rows,
        target_rows_before: target_before,
        fields,
        dropped,
        unfilled,
        written: 0,
        verified: false,
    };
    if !request.apply {
        return Ok(report);
    }

    // The old rows, renamed field by field, as the target's own InfoTable.
    let source_names: BTreeSet<&str> = old_fields.keys().map(String::as_str).collect();
    let moved: Vec<Map<String, Value>> = rows_of(&old_entries, &source_names)
        .into_iter()
        .map(|row| {
            report
                .fields
                .iter()
                .filter_map(|field| {
                    row.get(&field.from)
                        .map(|value| (field.to.clone(), value.clone()))
                })
                .collect()
        })
        .collect();
    let definitions: Map<String, Value> = new_shape_probe["dataShape"]["fieldDefinitions"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| targets.contains(name.as_str()))
        .map(|(name, definition)| (name.clone(), definition.clone()))
        .collect();
    for chunk in moved.chunks(BATCH) {
        let table = json!({ "dataShape": { "fieldDefinitions": definitions }, "rows": chunk });
        remote.add(&request.new, &table)?;
        report.written += chunk.len() as u64;
    }
    report.applied = true;

    // Read the target back: the count grew by what was written, and its rows include every one
    // that was sent (a preexisting row of an append is allowed to be there as well).
    let after = remote.count(&request.new)?;
    if after != target_before + report.written {
        return Err(CopyError::Refused(format!(
            "{} has {after} row(s) after copying {}, expected {}; compare the tables before using either",
            request.new,
            report.written,
            target_before + report.written
        )));
    }
    let back = remote.entries(&request.new, after.max(1))?;
    let target_names: BTreeSet<&str> = targets.iter().map(String::as_str).collect();
    let have = sorted_keys(&rows_of(&back, &target_names));
    let mut missing = 0u64;
    for key in sorted_keys(&moved) {
        match have.binary_search(&key) {
            Ok(_) => {}
            Err(_) => missing += 1,
        }
    }
    if missing > 0 {
        return Err(CopyError::Refused(format!(
            "{missing} of the {} copied row(s) did not read back equal from {}; compare the tables before using either",
            report.written, request.new
        )));
    }
    report.verified = true;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Table {
        fields: Vec<(&'static str, &'static str)>,
        rows: Vec<Value>,
    }

    #[derive(Default)]
    struct Fake {
        tables: RefCell<BTreeMap<String, Table>>,
        adds: RefCell<usize>,
        /// Silently drop everything written.
        lose_writes: bool,
    }

    impl Fake {
        fn table(
            self,
            name: &str,
            fields: &[(&'static str, &'static str)],
            rows: Vec<Value>,
        ) -> Self {
            self.tables.borrow_mut().insert(
                name.to_string(),
                Table {
                    fields: fields.to_vec(),
                    rows,
                },
            );
            self
        }
    }

    impl Remote for Fake {
        fn exists(&self, table: &str) -> Result<bool, ServerError> {
            Ok(self.tables.borrow().contains_key(table))
        }
        fn count(&self, table: &str) -> Result<u64, ServerError> {
            Ok(self.tables.borrow()[table].rows.len() as u64)
        }
        fn entries(&self, table: &str, max_items: u64) -> Result<Value, ServerError> {
            let tables = self.tables.borrow();
            let table = &tables[table];
            let mut definitions = Map::new();
            for system in SYSTEM_COLUMNS {
                definitions.insert(
                    system.to_string(),
                    json!({ "name": system, "baseType": "STRING" }),
                );
            }
            for (name, base) in &table.fields {
                definitions.insert(name.to_string(), json!({ "name": name, "baseType": base }));
            }
            let rows: Vec<Value> = table
                .rows
                .iter()
                .take(max_items as usize)
                .map(|row| {
                    let mut row = row.as_object().unwrap().clone();
                    row.insert("key".into(), json!("1"));
                    row.insert("source".into(), json!("Administrator"));
                    Value::Object(row)
                })
                .collect();
            Ok(json!({ "dataShape": { "fieldDefinitions": definitions }, "rows": rows }))
        }
        fn add(&self, table: &str, rows: &Value) -> Result<(), ServerError> {
            *self.adds.borrow_mut() += 1;
            if !self.lose_writes {
                let mut tables = self.tables.borrow_mut();
                // The platform reads numbers back as floats.
                let added = rows["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(canonical_numbers);
                tables.get_mut(table).unwrap().rows.extend(added);
            }
            Ok(())
        }
    }

    fn canonical_numbers(row: &Value) -> Value {
        Value::Object(
            row.as_object()
                .unwrap()
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        value
                            .as_i64()
                            .map_or_else(|| value.clone(), |int| json!(int as f64)),
                    )
                })
                .collect(),
        )
    }

    fn solution(ledger: Option<&str>) -> (std::path::PathBuf, Solution) {
        let nonce = crate::test_nonce();
        let root =
            std::env::temp_dir().join(format!("twaco-dtcopy-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        if let Some(ledger) = ledger {
            std::fs::write(root.join(".twaco/renames.json"), ledger).unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    fn request(apply: bool) -> Request {
        Request {
            old: "Old_DT".into(),
            new: "New_DT".into(),
            max_rows: 1000,
            apply,
            ..Default::default()
        }
    }

    fn rows() -> Vec<Value> {
        vec![
            json!({ "id": "1", "label": "first", "amount": 1.5 }),
            json!({ "id": "2", "label": "second", "amount": 2.0 }),
        ]
    }

    const OLD: [(&str, &str); 3] = [("id", "STRING"), ("label", "STRING"), ("amount", "NUMBER")];
    const LEDGER: &str = r#"[{"date":"2026-10-01","kind":"field","old":"label","new":"title","scope":"Old_DS","entities":[]}]"#;

    #[test]
    fn a_plan_reads_the_shapes_maps_fields_by_name_and_the_ledger_and_writes_nothing() {
        let (root, solution) = solution(Some(LEDGER));
        let fake = Fake::default().table("Old_DT", &OLD, rows()).table(
            "New_DT",
            &[
                ("id", "STRING"),
                ("title", "STRING"),
                ("amount", "NUMBER"),
                ("extra", "STRING"),
            ],
            vec![],
        );
        let report = run(&fake, &solution, &request(false)).unwrap();
        let moves: Vec<(&str, &str, &str)> = report
            .fields
            .iter()
            .map(|f| (f.from.as_str(), f.to.as_str(), f.by))
            .collect();
        assert_eq!(
            moves,
            [
                ("amount", "amount", "same name"),
                ("id", "id", "same name"),
                ("label", "title", "ledger")
            ]
        );
        assert_eq!(report.unfilled, ["extra"]);
        assert_eq!((report.source_rows, report.applied), (2, false));
        assert_eq!(*fake.adds.borrow(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_apply_copies_renamed_rows_and_confirms_them_read_back() {
        let (root, solution) = solution(Some(LEDGER));
        let fake = Fake::default().table("Old_DT", &OLD, rows()).table(
            "New_DT",
            &[("id", "STRING"), ("title", "STRING"), ("amount", "NUMBER")],
            vec![],
        );
        let report = run(&fake, &solution, &request(true)).unwrap();
        assert!(report.applied && report.verified);
        assert_eq!(report.written, 2);
        let tables = fake.tables.borrow();
        assert_eq!(
            tables["New_DT"].rows[1],
            json!({ "id": "2", "title": "second", "amount": 2.0 })
        );
        assert_eq!(tables["Old_DT"].rows.len(), 2, "the source keeps its rows");
        drop(tables);
        // A second copy is refused: the target is not empty.
        let again = run(&fake, &solution, &request(true)).unwrap_err();
        assert!(
            again.to_string().contains("already has 2 row(s)"),
            "{again}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unmapped_fields_type_mismatches_and_collisions_are_refused_before_any_write() {
        let (root, solution) = solution(None);
        let renamed = Fake::default().table("Old_DT", &OLD, rows()).table(
            "New_DT",
            &[("id", "STRING"), ("title", "STRING"), ("amount", "NUMBER")],
            vec![],
        );
        let refused = run(&renamed, &solution, &request(true)).unwrap_err();
        assert!(
            refused.to_string().contains("label") && refused.to_string().contains("--map"),
            "{refused}"
        );
        // --map names it; --drop-unmapped leaves it behind.
        let mut mapped = request(false);
        mapped.map = parse_map("label=title").unwrap();
        assert_eq!(run(&renamed, &solution, &mapped).unwrap().fields.len(), 3);
        let mut dropping = request(false);
        dropping.drop_unmapped = true;
        assert_eq!(
            run(&renamed, &solution, &dropping).unwrap().dropped,
            ["label"]
        );
        // A type change is refused.
        let retyped = Fake::default().table("Old_DT", &OLD, rows()).table(
            "New_DT",
            &[("id", "STRING"), ("label", "STRING"), ("amount", "STRING")],
            vec![],
        );
        assert!(run(&retyped, &solution, &request(true))
            .unwrap_err()
            .to_string()
            .contains("NUMBER"));
        // Two source fields cannot fill one target field.
        let mut collide = request(false);
        collide.map = parse_map("label=id").unwrap();
        assert!(run(&renamed, &solution, &collide).is_err());
        assert_eq!(*renamed.adds.borrow() + *retyped.adds.borrow(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_write_the_server_loses_is_a_failure_not_a_success() {
        let (root, solution) = solution(None);
        let fake = Fake {
            lose_writes: true,
            ..Default::default()
        }
        .table("Old_DT", &OLD, rows())
        .table("New_DT", &OLD, vec![]);
        let error = run(&fake, &solution, &request(true)).unwrap_err();
        assert!(
            error.to_string().contains("has 0 row(s) after copying 2"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_tables_sizes_and_arguments_are_refused() {
        let (root, solution) = solution(None);
        let fake = Fake::default().table("Old_DT", &OLD, rows());
        assert!(run(&fake, &solution, &request(false))
            .unwrap_err()
            .to_string()
            .contains("New_DT is not on the server"));
        let both = Fake::default()
            .table("Old_DT", &OLD, rows())
            .table("New_DT", &OLD, vec![]);
        let mut small = request(false);
        small.max_rows = 1;
        assert!(run(&both, &solution, &small)
            .unwrap_err()
            .to_string()
            .contains("over --max-rows 1"));
        let mut same = request(false);
        same.new = "Old_DT".into();
        assert!(run(&both, &solution, &same).is_err());
        assert!(
            parse_map("a=b,c").is_err()
                && parse_map("a=b,a=c").is_err()
                && parse_map("").unwrap().is_empty()
        );
        let mut unknown = request(false);
        unknown.map = parse_map("nope=id").unwrap();
        assert!(run(&both, &solution, &unknown)
            .unwrap_err()
            .to_string()
            .contains("no field nope"));
        let _ = std::fs::remove_dir_all(root);
    }
}
