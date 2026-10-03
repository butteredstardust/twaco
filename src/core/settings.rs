//! The server's subsystem settings, read-only.
//!
//! Each of the platform's subsystems keeps its settings in configuration tables, read alike
//! with `GetConfigurationTables` and `GetConfigurationTable`. Most are one-row tables whose
//! fields are described, so a setting can be found by name or by what it does. A setting is
//! the whole server's, so twaco only reads them; a PASSWORD field's value is never shown.

use super::server::{Client, ServerError};
use serde_json::{json, Value};
use std::fmt;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(60);
pub const HIDDEN: &str = "(hidden)";

/// What this module asks of a server, as a trait so it is tested offline.
pub trait Remote: Sync {
    fn subsystems(&self) -> Result<Vec<String>, ServerError>;
    fn service(&self, subsystem: &str, service: &str, body: &Value) -> Result<Option<Value>, ServerError>;
}

impl Remote for Client {
    fn subsystems(&self) -> Result<Vec<String>, ServerError> {
        let reply = self.call_service(
            "Resources/EntityServices",
            "GetEntityList",
            &json!({ "type": "Subsystem", "maxItems": 500 }),
            TIMEOUT,
        )?;
        let shape = |why: &str| ServerError::InvalidResponse { url: "Resources/EntityServices/Services/GetEntityList".to_string(), why: format!("subsystems: {why}") };
        let rows = reply
            .as_ref()
            .and_then(|v| v.get("rows"))
            .and_then(Value::as_array)
            .ok_or_else(|| shape("no rows"))?;
        let mut names = rows
            .iter()
            .map(|row| row.get("name").and_then(Value::as_str).map(str::to_string).ok_or_else(|| shape("a row without a name")))
            .collect::<Result<Vec<String>, ServerError>>()?;
        names.sort();
        Ok(names)
    }

    fn service(&self, subsystem: &str, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
        self.call_service(&format!("Subsystems/{subsystem}"), service, body, TIMEOUT)
    }
}

#[derive(Debug)]
pub enum SettingsError {
    Remote(ServerError),
    Shape(String),
    Invalid(String),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Remote(error) => write!(f, "{error}"),
            SettingsError::Shape(why) => write!(f, "unexpected settings response: {why}"),
            SettingsError::Invalid(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for SettingsError {}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub base_type: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub name: String,
    pub fields: Vec<Field>,
    /// Values by field name; a PASSWORD field's value is already replaced by `HIDDEN`.
    pub rows: Vec<serde_json::Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Subsystem {
    pub name: String,
    pub running: bool,
    pub tables: Vec<Table>,
}

fn rows_of(reply: Option<Value>, what: &str) -> Result<Vec<Value>, SettingsError> {
    reply
        .and_then(|v| v.get("rows").and_then(Value::as_array).cloned())
        .ok_or_else(|| SettingsError::Shape(format!("{what} returned no rows")))
}

/// Whether a subsystem runs. A failed or odd answer is an error, never an unknown.
fn running(remote: &dyn Remote, name: &str) -> Result<bool, SettingsError> {
    remote
        .service(name, "IsRunning", &json!({}))
        .map_err(SettingsError::Remote)?
        .and_then(|v| v.pointer("/rows/0/result").and_then(Value::as_bool))
        .ok_or_else(|| SettingsError::Shape(format!("{name}.IsRunning returned no boolean result")))
}

/// The table names a subsystem lists; a row without a name is an error.
fn table_names(remote: &dyn Remote, name: &str) -> Result<Vec<String>, SettingsError> {
    let what = format!("{name}.GetConfigurationTables");
    let mut names = rows_of(remote.service(name, "GetConfigurationTables", &json!({})).map_err(SettingsError::Remote)?, &what)?
        .iter()
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| SettingsError::Shape(format!("{what}: a row without a name")))
        })
        .collect::<Result<Vec<String>, SettingsError>>()?;
    names.sort_by_key(|t| t.to_lowercase());
    Ok(names)
}

/// One table's fields and rows. Fails closed: every field must say its type, so that a
/// PASSWORD is always known to be one, and every row must be an object.
fn table(name: &str, table: &str, reply: &Value) -> Result<Table, SettingsError> {
    let shape = |why: String| SettingsError::Shape(format!("{name}.{table}: {why}"));
    let defs = reply
        .pointer("/dataShape/fieldDefinitions")
        .and_then(Value::as_object)
        .ok_or_else(|| shape("no field definitions".to_string()))?;
    let mut fields = Vec::new();
    for (field, def) in defs {
        let base_type = def
            .get("baseType")
            .and_then(Value::as_str)
            .ok_or_else(|| shape(format!("field {field} has no baseType")))?;
        let description = match def.get("description") {
            None | Some(Value::Null) => "",
            Some(Value::String(text)) => text,
            Some(_) => return Err(shape(format!("field {field} has a description that is not text"))),
        };
        fields.push(Field { name: field.clone(), base_type: base_type.to_string(), description: description.to_string() });
    }
    fields.sort_by_key(|f| f.name.to_lowercase());
    let secret: Vec<&str> = fields.iter().filter(|f| f.base_type == "PASSWORD").map(|f| f.name.as_str()).collect();
    let rows = reply.get("rows").and_then(Value::as_array).ok_or_else(|| shape("no rows".to_string()))?;
    let rows = rows
        .iter()
        .map(|row| {
            let mut row = row.as_object().ok_or_else(|| shape("a row that is not an object".to_string()))?.clone();
            for field in &secret {
                if row.get(*field).is_some_and(|v| !v.is_null() && v.as_str() != Some("")) {
                    row.insert((*field).to_string(), json!(HIDDEN));
                }
            }
            Ok(row)
        })
        .collect::<Result<Vec<_>, SettingsError>>()?;
    Ok(Table { name: table.to_string(), fields, rows })
}

/// One subsystem: whether it runs, and every configuration table with its fields and values.
pub fn read(remote: &dyn Remote, name: &str) -> Result<Subsystem, SettingsError> {
    let running = running(remote, name)?;
    let mut tables = Vec::new();
    for wanted in table_names(remote, name)? {
        let reply = remote
            .service(name, "GetConfigurationTable", &json!({ "tableName": wanted }))
            .map_err(SettingsError::Remote)?
            .ok_or_else(|| SettingsError::Shape(format!("{name}.{wanted} returned nothing")))?;
        tables.push(table(name, &wanted, &reply)?);
    }
    Ok(Subsystem { name: name.to_string(), running, tables })
}

/// A subsystem as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub name: String,
    pub running: bool,
    pub tables: Vec<String>,
}

/// Every subsystem's running state and table names, without the tables' values: the list a
/// person picks from, which reading every table made take 9 s.
pub fn summaries(remote: &dyn Remote) -> Result<Vec<Summary>, SettingsError> {
    let names = remote.subsystems().map_err(SettingsError::Remote)?;
    super::parallel::map(&names, |name| -> Result<Summary, SettingsError> {
        Ok(Summary { name: name.clone(), running: running(remote, name)?, tables: table_names(remote, name)? })
    })
    .into_iter()
    .collect()
}

/// Every subsystem, read in parallel.
pub fn read_all(remote: &dyn Remote) -> Result<Vec<Subsystem>, SettingsError> {
    let names = remote.subsystems().map_err(SettingsError::Remote)?;
    super::parallel::map(&names, |name| read(remote, name)).into_iter().collect()
}

/// The subsystem a name means: exactly, or without its `Subsystem` suffix, any case.
pub fn resolve<'a>(names: &'a [String], wanted: &str) -> Result<&'a str, SettingsError> {
    let wanted = wanted.to_lowercase();
    names
        .iter()
        .find(|n| n.to_lowercase() == wanted || n.to_lowercase() == format!("{wanted}subsystem"))
        .map(String::as_str)
        .ok_or_else(|| SettingsError::Invalid(format!("no subsystem {wanted:?}; there are: {}", names.join(", "))))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub subsystem: String,
    pub table: String,
    pub field: Field,
    /// The value in each row (most tables have one).
    pub values: Vec<Value>,
}

/// Settings whose subsystem, table, field name or description contains `text`, any case.
pub fn search(all: &[Subsystem], text: &str) -> Vec<Found> {
    let text = text.to_lowercase();
    let mut found = Vec::new();
    for subsystem in all {
        for table in &subsystem.tables {
            for field in &table.fields {
                let haystack = format!("{}.{}.{} {}", subsystem.name, table.name, field.name, field.description).to_lowercase();
                if haystack.contains(&text) {
                    found.push(Found {
                        subsystem: subsystem.name.clone(),
                        table: table.name.clone(),
                        field: field.clone(),
                        values: table.rows.iter().map(|row| row.get(&field.name).cloned().unwrap_or(Value::Null)).collect(),
                    });
                }
            }
        }
    }
    found
}

/// A table as JSON: each setting with its type, description and value in every row.
pub fn table_json(subsystem: &str, t: &Table) -> Value {
    json!({
        "subsystem": subsystem,
        "table": t.name,
        "settings": t.fields.iter().map(|f| json!({
            "name": f.name,
            "type": f.base_type,
            "description": f.description,
            "values": t.rows.iter().map(|row| row.get(&f.name).cloned().unwrap_or(Value::Null)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// A value for a person: text as it is, everything else as JSON.
pub fn shown(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    impl Remote for Fake {
        fn subsystems(&self) -> Result<Vec<String>, ServerError> {
            Ok(vec!["LoggingSubsystem".into(), "FederationSubsystem".into()])
        }

        fn service(&self, subsystem: &str, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
            Ok(Some(match (subsystem, service) {
                (_, "IsRunning") => json!({ "rows": [{ "result": true }] }),
                ("LoggingSubsystem", "GetConfigurationTables") => json!({ "rows": [{ "name": "LogRetentionSettings" }] }),
                ("FederationSubsystem", "GetConfigurationTables") => json!({ "rows": [{ "name": "Subscribers" }] }),
                ("LoggingSubsystem", "GetConfigurationTable") => {
                    assert_eq!(body["tableName"], "LogRetentionSettings");
                    json!({
                        "dataShape": { "fieldDefinitions": {
                            "maxDays": { "baseType": "INTEGER", "description": "Days a log entry is kept" },
                            "enabled": { "baseType": "BOOLEAN", "description": "Whether retention runs" }
                        } },
                        "rows": [{ "maxDays": 7, "enabled": true }]
                    })
                }
                ("FederationSubsystem", "GetConfigurationTable") => json!({
                    "dataShape": { "fieldDefinitions": {
                        "applicationKey": { "baseType": "PASSWORD", "description": "Key used to subscribe" },
                        "server": { "baseType": "STRING", "description": "Subscriber address" }
                    } },
                    "rows": [{ "applicationKey": "s3cret", "server": "https://other" }, { "applicationKey": "", "server": "x" }]
                }),
                other => panic!("unexpected {other:?}"),
            }))
        }
    }

    #[test]
    fn a_subsystem_reads_with_its_fields_described_and_secrets_hidden() {
        let all = read_all(&Fake).unwrap();
        let logging = all.iter().find(|s| s.name == "LoggingSubsystem").unwrap();
        assert!(logging.running);
        assert_eq!(logging.tables[0].fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["enabled", "maxDays"]);
        assert_eq!(logging.tables[0].rows[0]["maxDays"], 7);
        let federation = all.iter().find(|s| s.name == "FederationSubsystem").unwrap();
        assert_eq!(federation.tables[0].rows[0]["applicationKey"], HIDDEN, "a secret is never shown");
        assert_eq!(federation.tables[0].rows[1]["applicationKey"], "", "an empty secret says it is empty");
    }

    #[test]
    fn a_setting_is_found_by_name_or_by_what_it_does() {
        let all = read_all(&Fake).unwrap();
        let by_name = search(&all, "maxdays");
        assert_eq!(by_name.len(), 1);
        assert_eq!((by_name[0].subsystem.as_str(), by_name[0].table.as_str()), ("LoggingSubsystem", "LogRetentionSettings"));
        assert_eq!(by_name[0].values, [json!(7)]);
        assert_eq!(search(&all, "kept").len(), 1, "by description");
        assert!(search(&all, "s3cret").is_empty(), "a hidden value is not searchable");
    }

    #[test]
    fn an_odd_answer_is_an_error_never_a_shown_secret_or_an_empty_table() {
        let defs = |def: Value| json!({ "dataShape": { "fieldDefinitions": { "applicationKey": def } }, "rows": [{ "applicationKey": "s3cret" }] });
        // A field that does not say its type might be a PASSWORD: refuse rather than show it.
        for def in [json!({}), json!({ "baseType": 7 }), json!("PASSWORD")] {
            let error = table("S", "T", &defs(def.clone())).unwrap_err();
            assert!(error.to_string().contains("applicationKey has no baseType"), "{def}: {error}");
        }
        assert!(table("S", "T", &json!({ "dataShape": {}, "rows": [] })).is_err(), "no field definitions");
        let fields = json!({ "x": { "baseType": "STRING" } });
        for rows in [json!("invalid"), json!(["not a row"]), Value::Null] {
            let reply = json!({ "dataShape": { "fieldDefinitions": fields }, "rows": rows });
            assert!(table("S", "T", &reply).is_err(), "rows {rows}");
        }
        assert!(table("S", "T", &json!({ "dataShape": { "fieldDefinitions": fields }, "rows": [] })).unwrap().rows.is_empty(), "an empty table is fine");
    }

    struct Broken(&'static str);

    impl Remote for Broken {
        fn subsystems(&self) -> Result<Vec<String>, ServerError> {
            Ok(vec!["LoggingSubsystem".into()])
        }

        fn service(&self, _: &str, service: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            match (self.0, service) {
                ("running fails", "IsRunning") => Err(ServerError::InvalidUrl("down".into())),
                ("running odd", "IsRunning") => Ok(Some(json!({ "rows": [{ "result": "yes" }] }))),
                ("unnamed table", "GetConfigurationTables") => Ok(Some(json!({ "rows": [{ "title": "x" }] }))),
                (_, "IsRunning") => Ok(Some(json!({ "rows": [{ "result": false }] }))),
                (_, "GetConfigurationTables") => Ok(Some(json!({ "rows": [] }))),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn a_failed_or_odd_running_state_or_table_list_is_an_error() {
        for case in ["running fails", "running odd", "unnamed table"] {
            assert!(summaries(&Broken(case)).is_err(), "{case}: summaries");
            assert!(read(&Broken(case), "LoggingSubsystem").is_err(), "{case}: read");
        }
        assert!(!summaries(&Broken("fine")).unwrap()[0].running);
    }

    #[test]
    fn a_subsystem_is_named_with_or_without_its_suffix() {
        let names = vec!["LoggingSubsystem".to_string()];
        assert_eq!(resolve(&names, "logging").unwrap(), "LoggingSubsystem");
        assert_eq!(resolve(&names, "LoggingSubsystem").unwrap(), "LoggingSubsystem");
        assert!(resolve(&names, "Nope").is_err());
    }
}
