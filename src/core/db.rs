//! Run one SQL command or query through a short-lived Thing on the built-in Database template.
//! The repository identifies the connection, but credentials and connection values come from
//! the live Thing and the selected profile. The temporary Thing is removed on every exit path.

use super::config::Solution;
use super::profile::Profile;
use super::scan::{self, Kind};
use super::server::Client;
use super::workspace;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static NAME_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Run,
    Query,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub mode: Mode,
    pub thing: Option<String>,
    pub apply: bool,
    pub no_transaction: bool,
    pub max_rows: u64,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub applied: bool,
    pub mode: Mode,
    pub thing: String,
    pub jdbc_url: String,
    pub user: String,
    pub bytes: usize,
    pub sql: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary_thing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbError(String);

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DbError {}

/// The server operations are deliberately small so the entire destructive recipe is testable
/// without a server. Implementations must not retain or log service parameter bodies.
pub trait Remote {
    fn thing(&self, name: &str) -> Result<Value, String>;
    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), String>;
    fn call(
        &self,
        target: &str,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, String>;
    fn delete_thing(&self, name: &str) -> Result<(), String>;
    fn thing_exists(&self, name: &str) -> Result<bool, String>;
}

impl Remote for Client {
    fn thing(&self, name: &str) -> Result<Value, String> {
        self.fetch_entity_json("Things", name)
            .map_err(|error| error.to_string())
    }

    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), String> {
        self.import_entity(file_name, xml)
            .map_err(|error| error.to_string())
    }

    fn call(
        &self,
        target: &str,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, String> {
        self.call_service(target, service, parameters, timeout)
            .map_err(|error| error.to_string())
    }

    fn delete_thing(&self, name: &str) -> Result<(), String> {
        self.call_service(
            "Resources/EntityServices",
            "DeleteThing",
            &json!({ "name": name }),
            Duration::from_secs(120),
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn thing_exists(&self, name: &str) -> Result<bool, String> {
        self.entity_exists("Things", name)
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone, Debug)]
struct Connection {
    url: String,
    driver: String,
    user: String,
    validation: String,
    max_connections: Value,
}

/// Resolve the database Thing from local inheritance unless the caller named one explicitly.
pub fn resolve_thing(solution: &Solution, explicit: Option<&str>) -> Result<String, DbError> {
    if let Some(name) = explicit {
        if name.is_empty() {
            return Err(DbError("--thing needs a non-empty Thing name".to_string()));
        }
        return Ok(name.to_string());
    }
    let discovery = workspace::discover(solution);
    if !discovery.unreadable.is_empty() {
        return Err(DbError(format!(
            "cannot choose a database Thing while entity files are unreadable: {}",
            discovery.unreadable.join("; ")
        )));
    }
    let mut bases = BTreeMap::new();
    let mut things = Vec::new();
    for entity in discovery.entities {
        let bytes = std::fs::read(&entity.path)
            .map_err(|error| DbError(format!("{}: {error}", entity.path.display())))?;
        let tokens = scan::tokenize(&bytes)
            .map_err(|error| DbError(format!("{}: {error}", entity.path.display())))?;
        let tag = tokens
            .iter()
            .filter(|token| matches!(token.kind, Kind::Start | Kind::Empty))
            .nth(2)
            .ok_or_else(|| DbError(format!("{}: entity tag is missing", entity.path.display())))?;
        if entity.info.collection == "ThingTemplates" {
            let base = attribute(&bytes, tag, "baseThingTemplate")?;
            bases.insert(entity.info.name, base);
        } else if entity.info.collection == "Things" {
            things.push((entity.info.name, attribute(&bytes, tag, "thingTemplate")?));
        }
    }
    let inherits_database = |template: &str| {
        let mut current = template;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            if current == "Database" || current.ends_with("Database_TT") {
                return true;
            }
            if !seen.insert(current.to_string()) {
                return false;
            }
            let Some(parent) = bases.get(current) else {
                return false;
            };
            current = parent;
        }
    };
    let candidates: Vec<String> = things
        .into_iter()
        .filter(|(_, template)| inherits_database(template))
        .map(|(name, _)| name)
        .collect();
    match candidates.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(DbError(
            "no database Thing was found in the solution (candidates: none); pass --thing <name>"
                .to_string(),
        )),
        _ => Err(DbError(format!(
            "several database Things were found: {}; pass --thing <name>",
            candidates.join(", ")
        ))),
    }
}

fn attribute(bytes: &[u8], tag: &scan::Token, name: &str) -> Result<String, DbError> {
    scan::attribute(bytes, tag, name)
        .map_err(|error| DbError(error.to_string()))
        .map(|span| {
            span.map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(bytes))))
                .unwrap_or_default()
        })
}

pub fn execute(
    remote: &dyn Remote,
    solution: &Solution,
    profile: &Profile,
    sql: &str,
    options: &Options,
) -> Result<Report, DbError> {
    let password = profile.database_password().ok_or_else(|| {
        DbError(
            "the selected profile has no database_password; add it to the uncommitted profile"
                .to_string(),
        )
    })?;
    let thing = resolve_thing(solution, options.thing.as_deref())?;
    let live = remote
        .thing(&thing)
        .map_err(|why| secret_error(why, password))?;
    let mut connection = connection(&live)?;
    if let Some(user) = profile.database_user() {
        connection.user = user.to_string();
    }
    let sql = if options.mode == Mode::Run && options.no_transaction {
        format!("COMMIT; {sql}")
    } else {
        sql.to_string()
    };
    let mut report = Report {
        applied: options.apply,
        mode: options.mode,
        thing,
        jdbc_url: without_password(&connection.url),
        user: connection.user.clone(),
        bytes: sql.len(),
        sql: sql.clone(),
        temporary_thing: None,
        result: None,
    };
    if !options.apply {
        return Ok(report);
    }

    // `db query` is the one database command that does not plan first, so the database itself must
    // refuse a write: the label "read-only" is not enough for SQL that can contain `DELETE ... RETURNING`.
    let connection = if options.mode == Mode::Query {
        read_only_connection(connection)?
    } else {
        connection
    };
    let temporary = temporary_name();
    report.temporary_thing = Some(temporary.clone());
    let handler = if options.mode == Mode::Run {
        "SQLCommand"
    } else {
        "SQLQuery"
    };
    let xml = thing_xml(
        &temporary,
        handler,
        &sql,
        &connection,
        options.max_rows,
        options.timeout.as_secs(),
    );
    let mut cleanup = Cleanup {
        remote,
        name: temporary.clone(),
        password,
        active: true,
    };
    let attempt = (|| -> Result<Option<Value>, DbError> {
        remote
            .import(&format!("{temporary}.xml"), xml.as_bytes())
            .map_err(|why| secret_error(why, password))?;
        let encrypted = remote
            .call(
                "Resources/EncryptionServices",
                "EncryptPropertyValue",
                &json!({ "data": password }),
                options.timeout,
            )
            .map_err(|why| secret_error(why, password))?
            .and_then(|value| {
                value
                    .pointer("/rows/0/result")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .ok_or_else(|| {
                DbError("EncryptPropertyValue returned no rows[0].result".to_string())
            })?;
        let table = connection_table(&connection, &encrypted);
        remote
            .call(
                &format!("Things/{temporary}"),
                "SetConfigurationTable",
                &json!({ "tableName": "ConnectionInfo", "configurationTable": table }),
                options.timeout,
            )
            .map_err(|why| secret_error(why, password))?;
        remote
            .call(
                &format!("Things/{temporary}"),
                "RestartThing",
                &json!({}),
                options.timeout,
            )
            .map_err(|why| secret_error(why, password))?;
        remote
            .call(
                &format!("Things/{temporary}"),
                "Run",
                &json!({}),
                options.timeout,
            )
            .map_err(|why| secret_error(why, password))
    })();
    let removed = cleanup.finish();
    match (attempt, removed) {
        (Ok(result), Ok(())) => {
            report.result = result;
            Ok(report)
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(delete)) => Err(delete),
        (Err(error), Err(delete)) => {
            Err(DbError(format!("{error}; cleanup also failed: {delete}")))
        }
    }
}

/// What sweeping stale temporary Things needs from a server.
pub trait Sweeper {
    fn thing_names(&self) -> Result<Vec<String>, String>;
    fn template_of(&self, name: &str) -> Result<String, String>;
    fn delete_thing(&self, name: &str) -> Result<(), String>;
    fn thing_exists(&self, name: &str) -> Result<bool, String>;
}

impl Sweeper for Client {
    fn thing_names(&self) -> Result<Vec<String>, String> {
        self.list_entity_names("Things").map_err(|error| error.to_string())
    }

    fn template_of(&self, name: &str) -> Result<String, String> {
        let thing = Remote::thing(self, name)?;
        Ok(thing.get("thingTemplate").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    fn delete_thing(&self, name: &str) -> Result<(), String> {
        Remote::delete_thing(self, name)
    }

    fn thing_exists(&self, name: &str) -> Result<bool, String> {
        Remote::thing_exists(self, name)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SweepStatus {
    /// Found, and a plan leaves it.
    Stale,
    Deleted,
    /// Matches the name but is not a `Database` Thing: left alone.
    Skipped,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct Swept {
    pub name: String,
    pub status: SweepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// Whether a name is one `temporary_name` makes: `ZZ.Twaco.Sql.` and eight lowercase hex digits.
pub fn is_temporary_name(name: &str) -> bool {
    name.strip_prefix("ZZ.Twaco.Sql.")
        .is_some_and(|rest| rest.len() == 8 && rest.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
}

/// Find the temporary Things an interrupted run left, and with `apply` delete them, confirming
/// each is gone. Only a name twaco generates, on a `Database` Thing, is ever touched.
pub fn sweep(remote: &dyn Sweeper, apply: bool) -> Result<Vec<Swept>, DbError> {
    let mut names: Vec<String> = remote
        .thing_names()
        .map_err(|why| DbError(format!("cannot list the server's Things: {why}")))?
        .into_iter()
        .filter(|name| is_temporary_name(name))
        .collect();
    names.sort();
    let mut swept = Vec::new();
    for name in names {
        let template = remote.template_of(&name).map_err(|why| DbError(format!("cannot read {name}: {why}")))?;
        if template != "Database" {
            swept.push(Swept { name, status: SweepStatus::Skipped, why: Some(format!("its template is {template:?}, not Database")) });
            continue;
        }
        if !apply {
            swept.push(Swept { name, status: SweepStatus::Stale, why: None });
            continue;
        }
        let outcome = remote.delete_thing(&name).and_then(|()| match remote.thing_exists(&name) {
            Ok(false) => Ok(()),
            Ok(true) => Err("it still exists after the delete".to_string()),
            Err(why) => Err(format!("cannot confirm the delete: {why}")),
        });
        swept.push(match outcome {
            Ok(()) => Swept { name, status: SweepStatus::Deleted, why: None },
            Err(why) => Swept { name, status: SweepStatus::Failed, why: Some(why) },
        });
    }
    Ok(swept)
}

struct Cleanup<'a> {
    remote: &'a dyn Remote,
    name: String,
    password: &'a str,
    active: bool,
}

impl Cleanup<'_> {
    fn remove(&self) -> Result<(), DbError> {
        // The import may have failed before the Thing existed: there is nothing to delete, and
        // saying "FAILED TO DELETE" would send the user looking for a Thing that was never made.
        if let Ok(false) = self.remote.thing_exists(&self.name) {
            return Ok(());
        }
        self.remote.delete_thing(&self.name).map_err(|why| {
            secret_error(
                format!(
                    "FAILED TO DELETE temporary Thing {}: {why}; remove it manually",
                    self.name
                ),
                self.password,
            )
        })?;
        match self.remote.thing_exists(&self.name) {
            Ok(false) => Ok(()),
            Ok(true) => Err(DbError(format!("FAILED TO DELETE temporary Thing {}: it still exists; remove it manually", self.name))),
            Err(why) => Err(secret_error(format!("FAILED TO CONFIRM deletion of temporary Thing {}: {why}; check and remove it manually", self.name), self.password)),
        }
    }

    fn finish(&mut self) -> Result<(), DbError> {
        let result = self.remove();
        self.active = false;
        result
    }
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.remove();
        }
    }
}

fn connection(live: &Value) -> Result<Connection, DbError> {
    let row = live
        .pointer("/configurationTables/ConnectionInfo/rows/0")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            DbError("the live Thing has no configurationTables.ConnectionInfo.rows[0]".to_string())
        })?;
    let string = |name: &str| {
        row.get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                DbError(format!(
                    "the live ConnectionInfo row has no non-empty {name}"
                ))
            })
    };
    Ok(Connection {
        url: string("jDBCConnectionURL")?,
        driver: string("jDBCDriverClass")?,
        user: string("userName")?,
        validation: string("connectionValidationString")?,
        max_connections: row.get("maxConnections").cloned().ok_or_else(|| {
            DbError("the live ConnectionInfo row has no maxConnections".to_string())
        })?,
    })
}

/// The connection with PostgreSQL's `default_transaction_read_only` switched on for every session
/// the throwaway Thing opens. Another driver has no equivalent twaco can rely on, so it is refused.
fn read_only_connection(mut connection: Connection) -> Result<Connection, DbError> {
    if !connection
        .driver
        .to_ascii_lowercase()
        .contains("postgresql")
    {
        return Err(DbError(format!(
            "db query cannot enforce read-only for the driver {}; run the statement with `db run` instead",
            connection.driver
        )));
    }
    if connection.url.to_ascii_lowercase().contains("options=") {
        return Err(DbError(
            "the JDBC URL already sets `options`; db query will not override it".to_string(),
        ));
    }
    let separator = if connection.url.contains('?') {
        '&'
    } else {
        '?'
    };
    connection.url = format!(
        "{}{separator}options=-c%20default_transaction_read_only%3Don",
        connection.url
    );
    Ok(connection)
}

fn connection_table(connection: &Connection, encrypted: &str) -> Value {
    let fields = [
        ("connectionValidationString", "STRING"),
        ("jDBCConnectionURL", "STRING"),
        ("jDBCDriverClass", "STRING"),
        ("maxConnections", "NUMBER"),
        ("password", "PASSWORD"),
        ("userName", "STRING"),
    ];
    let definitions: Map<String, Value> = fields
        .iter()
        .enumerate()
        .map(|(ordinal, (name, base))| {
            (
                (*name).to_string(),
                json!({ "name": name, "baseType": base, "ordinal": ordinal }),
            )
        })
        .collect();
    json!({
        "dataShape": { "fieldDefinitions": definitions },
        "rows": [{
            "connectionValidationString": connection.validation,
            "jDBCConnectionURL": connection.url,
            "jDBCDriverClass": connection.driver,
            "maxConnections": connection.max_connections,
            "password": encrypted,
            "userName": connection.user,
        }]
    })
}

fn thing_xml(
    name: &str,
    handler: &str,
    sql: &str,
    connection: &Connection,
    max_rows: u64,
    timeout: u64,
) -> String {
    let field = |name: &str, base: &str, ordinal: usize| {
        format!(
        "<FieldDefinition baseType=\"{base}\" description=\"\" name=\"{name}\" ordinal=\"{ordinal}\"/>"
    )
    };
    let conn_fields = [
        ("connectionValidationString", "STRING"),
        ("jDBCConnectionURL", "STRING"),
        ("jDBCDriverClass", "STRING"),
        ("maxConnections", "NUMBER"),
        ("password", "PASSWORD"),
        ("userName", "STRING"),
    ]
    .iter()
    .enumerate()
    .map(|(at, (name, base))| field(name, base, at))
    .collect::<String>();
    let query_fields = [
        ("maxItems", "NUMBER"),
        ("timeout", "NUMBER"),
        ("sql", "STRING"),
    ]
    .iter()
    .enumerate()
    .map(|(at, (name, base))| field(name, base, at))
    .collect::<String>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<Entities build="b75" majorVersion="10" minorVersion="2" universal="">
<Things>
<Thing description="TEMPORARY: runs one SQL statement through the Database template." enabled="true" name="{}" projectName="" published="false" thingTemplate="Database">
<avatar/>
<DesignTimePermissions><Create/><Read/><Update/><Delete/><Metadata/></DesignTimePermissions>
<RunTimePermissions/>
<VisibilityPermissions><Visibility/></VisibilityPermissions>
<ConfigurationTableDefinitions/>
<ConfigurationTables>
<ConfigurationTable dataShapeName="" description="JDBC Settings" isMultiRow="false" name="ConnectionInfo" ordinal="0">
<DataShape><FieldDefinitions>{}</FieldDefinitions></DataShape>
<Rows><Row>
<connectionValidationString>{}</connectionValidationString>
<jDBCConnectionURL>{}</jDBCConnectionURL>
<jDBCDriverClass>{}</jDBCDriverClass>
<maxConnections>{}</maxConnections>
<password></password>
<userName>{}</userName>
</Row></Rows>
</ConfigurationTable>
</ConfigurationTables>
<ThingShape>
<PropertyDefinitions/>
<ServiceDefinitions>
<ServiceDefinition aspect.isAsync="false" category="" description="" isAllowOverride="false" isLocalOnly="false" isOpen="false" isPrivate="false" name="Run">
<ResultType baseType="INFOTABLE" description="" name="result"/>
<ParameterDefinitions/>
</ServiceDefinition>
</ServiceDefinitions>
<EventDefinitions/>
<ServiceMappings/>
<ServiceImplementations>
<ServiceImplementation description="" handlerName="{}" name="Run">
<ConfigurationTables>
<ConfigurationTable description="{}" isMultiRow="false" name="Query" ordinal="0">
<DataShape><FieldDefinitions>{}</FieldDefinitions></DataShape>
<Rows><Row><maxItems>{}.0</maxItems><timeout>{}.0</timeout><sql><![CDATA[{}]]></sql></Row></Rows>
</ConfigurationTable>
</ConfigurationTables>
</ServiceImplementation>
</ServiceImplementations>
<Subscriptions/>
</ThingShape>
</Thing>
</Things>
</Entities>"#,
        xml(name),
        conn_fields,
        xml(&connection.validation),
        xml(&without_password(&connection.url)),
        xml(&connection.driver),
        xml(connection.max_connections.to_string().trim_matches('"')),
        xml(&connection.user),
        xml(handler),
        xml(handler),
        query_fields,
        max_rows,
        timeout,
        cdata(sql)
    )
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn cdata(value: &str) -> String {
    value.replace("]]>", "]]]]><![CDATA[>")
}

fn temporary_name() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let counter = NAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "ZZ.Twaco.Sql.{:08x}",
        (now ^ counter.rotate_left(17) ^ u64::from(std::process::id())) as u32
    )
}

fn secret_error(mut why: String, password: &str) -> DbError {
    if !password.is_empty() {
        why = why.replace(password, "<redacted>");
    }
    DbError(why)
}

/// Remove a JDBC `password=` parameter without otherwise hiding the connection target.
pub fn without_password(url: &str) -> String {
    let mut out = url.to_string();
    loop {
        let lower = out.to_ascii_lowercase();
        let mut search_from = 0;
        let key = loop {
            let Some(relative) = lower[search_from..].find("password=") else {
                return out;
            };
            let found = search_from + relative;
            if found == 0 || matches!(lower.as_bytes()[found - 1], b'?' | b'&' | b';') {
                break found;
            }
            search_from = found + "password=".len();
        };
        let previous = out[..key]
            .char_indices()
            .rev()
            .find(|(_, c)| matches!(c, '?' | '&' | ';'));
        let next = out[key..]
            .char_indices()
            .find(|(_, c)| matches!(c, '&' | ';'))
            .map(|(at, _)| key + at);
        let (start, end) = match (previous, next) {
            (Some((_at, '?')), Some(end)) => (key, end + 1),
            (Some((at, _)), _) => (at, next.unwrap_or(out.len())),
            (None, Some(end)) => (key, end + 1),
            (None, None) => (key, out.len()),
        };
        out.replace_range(start..end, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    fn temp(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("twaco-db-{label}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname='P'\nroot='.'\n").unwrap();
        root
    }

    fn solution(root: &std::path::Path) -> Solution {
        Solution::load(&root.join("twaco.toml")).unwrap()
    }

    fn profile(password: Option<&str>) -> Profile {
        let mut extra = BTreeMap::new();
        if let Some(password) = password {
            extra.insert(
                "database_password".into(),
                toml::Value::String(password.into()),
            );
        }
        extra.insert(
            "database_user".into(),
            toml::Value::String("profile-user".into()),
        );
        Profile {
            url: "http://server".into(),
            username: "api".into(),
            password: "api-secret".into(),
            app_key: None,
            extra,
        }
    }

    fn live() -> Value {
        json!({ "configurationTables": { "ConnectionInfo": { "rows": [{
            "jDBCConnectionURL": "jdbc:postgresql://db/name?ssl=true&password=url-secret",
            "jDBCDriverClass": "org.postgresql.Driver", "userName": "thing-user",
            "connectionValidationString": "SELECT NOW()", "maxConnections": 7.0
        }] } } })
    }

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<(String, Value)>>,
        imported: RefCell<Option<String>>,
        fail: RefCell<Option<String>>,
        delete_fails: Cell<bool>,
        exists: Cell<bool>,
    }

    impl Fake {
        fn fail(&self, service: &str) -> bool {
            self.fail.borrow().as_deref() == Some(service)
        }
    }

    struct SweepFake {
        things: std::cell::RefCell<Vec<(String, String)>>,
        keep: bool,
    }

    impl Sweeper for SweepFake {
        fn thing_names(&self) -> Result<Vec<String>, String> {
            Ok(self.things.borrow().iter().map(|(name, _)| name.clone()).collect())
        }
        fn template_of(&self, name: &str) -> Result<String, String> {
            Ok(self.things.borrow().iter().find(|(thing, _)| thing == name).unwrap().1.clone())
        }
        fn delete_thing(&self, name: &str) -> Result<(), String> {
            if !self.keep {
                self.things.borrow_mut().retain(|(thing, _)| thing != name);
            }
            Ok(())
        }
        fn thing_exists(&self, name: &str) -> Result<bool, String> {
            Ok(self.things.borrow().iter().any(|(thing, _)| thing == name))
        }
    }

    fn sweep_fake(keep: bool) -> SweepFake {
        let things = [
            ("ZZ.Twaco.Sql.0badf00d", "Database"),
            ("ZZ.Twaco.Sql.0badf00e", "Database"),
            // Same prefix, not twaco's generated shape, or not a Database: never touched.
            ("ZZ.Twaco.Sql.mine", "Database"),
            ("ZZ.Twaco.Sql.0badf00f", "GenericThing"),
            ("Acme.Real.Database", "Database"),
        ];
        SweepFake { things: std::cell::RefCell::new(things.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()), keep }
    }

    #[test]
    fn a_sweep_plan_lists_only_generated_database_things_and_deletes_nothing() {
        let fake = sweep_fake(false);
        let plan = sweep(&fake, false).unwrap();
        let rows: Vec<(&str, &SweepStatus)> = plan.iter().map(|row| (row.name.as_str(), &row.status)).collect();
        assert_eq!(
            rows,
            [
                ("ZZ.Twaco.Sql.0badf00d", &SweepStatus::Stale),
                ("ZZ.Twaco.Sql.0badf00e", &SweepStatus::Stale),
                ("ZZ.Twaco.Sql.0badf00f", &SweepStatus::Skipped),
            ]
        );
        assert_eq!(fake.things.borrow().len(), 5, "a plan deletes nothing");
    }

    #[test]
    fn a_sweep_apply_deletes_confirms_and_leaves_everything_else() {
        let fake = sweep_fake(false);
        let applied = sweep(&fake, true).unwrap();
        assert_eq!(applied.iter().filter(|row| row.status == SweepStatus::Deleted).count(), 2);
        let left: Vec<String> = fake.things.borrow().iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(left, ["ZZ.Twaco.Sql.mine", "ZZ.Twaco.Sql.0badf00f", "Acme.Real.Database"]);
        // A delete that answers success but leaves the Thing is a failure.
        let stubborn = sweep_fake(true);
        let failed = sweep(&stubborn, true).unwrap();
        assert!(failed.iter().any(|row| row.status == SweepStatus::Failed && row.why.as_deref().is_some_and(|why| why.contains("still exists"))));
        assert!(is_temporary_name(&temporary_name()));
        assert!(!is_temporary_name("ZZ.Twaco.Sql.0BADF00D") && !is_temporary_name("ZZ.Twaco.Sql.0badf00"));
    }

    impl Remote for Fake {
        fn thing(&self, name: &str) -> Result<Value, String> {
            self.calls
                .borrow_mut()
                .push((format!("GET {name}"), Value::Null));
            Ok(live())
        }

        fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push((format!("IMPORT {file_name}"), Value::Null));
            self.exists.set(true);
            self.imported
                .replace(Some(String::from_utf8(xml.to_vec()).unwrap()));
            if self.fail("import") {
                Err("import broke DISTINCT-db-password".into())
            } else {
                Ok(())
            }
        }

        fn call(
            &self,
            target: &str,
            service: &str,
            parameters: &Value,
            _timeout: Duration,
        ) -> Result<Option<Value>, String> {
            self.calls
                .borrow_mut()
                .push((format!("CALL {target} {service}"), parameters.clone()));
            if self.fail(service) {
                return Err(format!("{service} broke DISTINCT-db-password"));
            }
            Ok(match service {
                "EncryptPropertyValue" => {
                    Some(json!({ "rows": [{ "result": "encrypted-value" }] }))
                }
                "Run" => Some(json!({ "rows": [{ "one": 1 }] })),
                _ => None,
            })
        }

        fn delete_thing(&self, name: &str) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push((format!("DELETE {name}"), Value::Null));
            if self.delete_fails.get() {
                Err("delete broke DISTINCT-db-password".into())
            } else {
                self.exists.set(false);
                Ok(())
            }
        }

        fn thing_exists(&self, name: &str) -> Result<bool, String> {
            self.calls
                .borrow_mut()
                .push((format!("EXISTS {name}"), Value::Null));
            Ok(self.exists.get())
        }
    }

    #[test]
    fn xml_escapes_values_splits_cdata_and_never_contains_password() {
        let connection = Connection {
            url: "jdbc:x?x=<&password=not-written".into(),
            driver: "D&\"".into(),
            user: "u<".into(),
            validation: "SELECT 1".into(),
            max_connections: json!(3.0),
        };
        let built = thing_xml("T&\"", "SQLCommand", "a ]]> b", &connection, 5, 6);
        assert!(built.contains("name=\"T&amp;&quot;\""));
        assert!(built.contains("a ]]]]><![CDATA[> b"));
        assert!(built.contains("<password></password>"));
        assert!(
            !built.contains("not-written"),
            "the URL itself is expected here, not a profile password"
        );
    }

    #[test]
    fn a_query_connection_is_read_only_at_the_database_or_refused() {
        let connection = |url: &str, driver: &str| Connection {
            url: url.into(),
            driver: driver.into(),
            user: "u".into(),
            validation: "SELECT 1".into(),
            max_connections: serde_json::json!(2),
        };
        let pg = read_only_connection(connection(
            "jdbc:postgresql://h/db",
            "org.postgresql.Driver",
        ))
        .unwrap();
        assert_eq!(
            pg.url,
            "jdbc:postgresql://h/db?options=-c%20default_transaction_read_only%3Don"
        );
        let with_query = read_only_connection(connection(
            "jdbc:postgresql://h/db?ssl=true",
            "org.postgresql.Driver",
        ))
        .unwrap();
        assert!(with_query
            .url
            .contains("?ssl=true&options=-c%20default_transaction_read_only%3Don"));
        let other = read_only_connection(connection(
            "jdbc:sqlserver://h",
            "com.microsoft.sqlserver.jdbc.SQLServerDriver",
        ))
        .unwrap_err();
        assert!(other.0.contains("cannot enforce read-only"), "{}", other.0);
        let taken = read_only_connection(connection(
            "jdbc:postgresql://h/db?options=-c%20x%3D1",
            "org.postgresql.Driver",
        ))
        .unwrap_err();
        assert!(taken.0.contains("already sets `options`"), "{}", taken.0);
    }

    #[test]
    fn jdbc_password_parameter_is_removed() {
        assert_eq!(
            without_password("jdbc:x?a=1&password=secret&b=2"),
            "jdbc:x?a=1&b=2"
        );
        assert_eq!(without_password("jdbc:x;a=1;PASSWORD=secret"), "jdbc:x;a=1");
        assert_eq!(
            without_password("jdbc:x?notpassword=visible&password=secret"),
            "jdbc:x?notpassword=visible"
        );
    }

    #[test]
    fn plan_reads_only_and_printable_fields_hide_both_passwords() {
        let root = temp("plan");
        let fake = Fake::default();
        let options = Options {
            mode: Mode::Run,
            thing: Some("P.Database".into()),
            apply: false,
            no_transaction: false,
            max_rows: 500,
            timeout: Duration::from_secs(120),
        };
        let report = execute(
            &fake,
            &solution(&root),
            &profile(Some("DISTINCT-db-password")),
            "UPDATE x SET y=1",
            &options,
        )
        .unwrap();
        assert_eq!(
            fake.calls
                .borrow()
                .iter()
                .map(|call| call.0.as_str())
                .collect::<Vec<_>>(),
            ["GET P.Database"]
        );
        assert!(fake.imported.borrow().is_none());
        let output = serde_json::to_string(&report).unwrap();
        assert!(output.contains("UPDATE x SET y=1"));
        assert!(!output.contains("DISTINCT-db-password"));
        assert!(!output.contains("url-secret"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn apply_uses_the_verified_order_and_bodies_then_deletes_and_confirms() {
        let root = temp("apply");
        let fake = Fake::default();
        let options = Options {
            mode: Mode::Run,
            thing: Some("P.Database".into()),
            apply: true,
            no_transaction: true,
            max_rows: 123,
            timeout: Duration::from_secs(9),
        };
        let report = execute(
            &fake,
            &solution(&root),
            &profile(Some("DISTINCT-db-password")),
            "a ]]> b",
            &options,
        )
        .unwrap();
        assert_eq!(report.sql, "COMMIT; a ]]> b");
        let calls = fake.calls.borrow();
        let names: Vec<&str> = calls.iter().map(|call| call.0.as_str()).collect();
        assert_eq!(names[0], "GET P.Database");
        assert!(names[1].starts_with("IMPORT ZZ.Twaco.Sql."));
        assert!(names[2].contains("EncryptionServices EncryptPropertyValue"));
        assert!(names[3].contains("SetConfigurationTable"));
        assert!(names[4].contains("RestartThing"));
        assert!(names[5].ends_with(" Run"));
        // The Thing is looked for first (it may never have been made), then deleted, then confirmed gone.
        assert!(names[6].starts_with("EXISTS ZZ.Twaco.Sql."));
        assert!(names[7].starts_with("DELETE ZZ.Twaco.Sql."));
        assert!(names[8].starts_with("EXISTS ZZ.Twaco.Sql."));
        assert_eq!(calls[2].1, json!({ "data": "DISTINCT-db-password" }));
        let table = &calls[3].1["configurationTable"];
        assert_eq!(table["rows"][0]["password"], "encrypted-value");
        assert_eq!(table["rows"][0]["userName"], "profile-user");
        assert_eq!(
            table["dataShape"]["fieldDefinitions"]["password"]["baseType"],
            "PASSWORD"
        );
        assert_eq!(
            table["dataShape"]["fieldDefinitions"]
                .as_object()
                .unwrap()
                .len(),
            6
        );
        let xml = fake.imported.borrow();
        let xml = xml.as_deref().unwrap();
        assert!(xml.contains("<password></password>"));
        assert!(xml.contains("<![CDATA[COMMIT; a ]]]]><![CDATA[> b]]>"));
        assert!(!xml.contains("DISTINCT-db-password"));
        assert!(!xml.contains("url-secret"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn middle_and_run_failures_still_delete_and_errors_hide_the_password() {
        for service in ["SetConfigurationTable", "Run"] {
            let root = temp(service);
            let fake = Fake::default();
            fake.fail.replace(Some(service.into()));
            let options = Options {
                mode: Mode::Query,
                thing: Some("Db".into()),
                apply: true,
                no_transaction: false,
                max_rows: 10,
                timeout: Duration::from_secs(1),
            };
            let error = execute(
                &fake,
                &solution(&root),
                &profile(Some("DISTINCT-db-password")),
                "SELECT 1",
                &options,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(service));
            assert!(!error.contains("DISTINCT-db-password"));
            assert!(fake
                .calls
                .borrow()
                .iter()
                .any(|call| call.0.starts_with("DELETE ZZ.Twaco.Sql.")));
            assert!(!fake.exists.get());
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn failed_delete_is_loud_names_the_thing_and_hides_the_password() {
        let root = temp("delete-fail");
        let fake = Fake::default();
        fake.delete_fails.set(true);
        let options = Options {
            mode: Mode::Run,
            thing: Some("Db".into()),
            apply: true,
            no_transaction: false,
            max_rows: 10,
            timeout: Duration::from_secs(1),
        };
        let error = execute(
            &fake,
            &solution(&root),
            &profile(Some("DISTINCT-db-password")),
            "UPDATE x",
            &options,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("FAILED TO DELETE temporary Thing ZZ.Twaco.Sql."));
        assert!(error.contains("remove it manually"));
        assert!(!error.contains("DISTINCT-db-password"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_database_password_refuses_before_any_server_read() {
        let root = temp("password");
        let fake = Fake::default();
        let options = Options {
            mode: Mode::Run,
            thing: Some("Db".into()),
            apply: false,
            no_transaction: false,
            max_rows: 10,
            timeout: Duration::from_secs(1),
        };
        let error = execute(
            &fake,
            &solution(&root),
            &profile(None),
            "UPDATE x",
            &options,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("database_password"));
        assert!(fake.calls.borrow().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_resolution_follows_template_inheritance_and_explicit_wins() {
        let root = temp("candidates");
        std::fs::create_dir_all(root.join("ThingTemplates")).unwrap();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(root.join("ThingTemplates/P.DbBase.xml"), r#"<Entities><ThingTemplates><ThingTemplate name="P.DbBase" projectName="P" baseThingTemplate="Database"/></ThingTemplates></Entities>"#).unwrap();
        std::fs::write(root.join("Things/P.Db.xml"), r#"<Entities><Things><Thing name="P.Db" projectName="P" thingTemplate="P.DbBase"/></Things></Entities>"#).unwrap();
        std::fs::write(root.join("Things/P.Other.xml"), r#"<Entities><Things><Thing name="P.Other" projectName="P" thingTemplate="GenericThing"/></Things></Entities>"#).unwrap();
        let solution = solution(&root);
        assert_eq!(resolve_thing(&solution, None).unwrap(), "P.Db");
        assert_eq!(
            resolve_thing(&solution, Some("Outside.Db")).unwrap(),
            "Outside.Db"
        );
        std::fs::write(root.join("Things/P.Second.xml"), r#"<Entities><Things><Thing name="P.Second" projectName="P" thingTemplate="Acme.Database_TT"/></Things></Entities>"#).unwrap();
        let error = resolve_thing(&solution, None).unwrap_err().to_string();
        assert!(error.contains("P.Db") && error.contains("P.Second"));
        std::fs::remove_file(root.join("Things/P.Db.xml")).unwrap();
        std::fs::remove_file(root.join("Things/P.Second.xml")).unwrap();
        assert!(resolve_thing(&solution, None)
            .unwrap_err()
            .to_string()
            .contains("no database Thing"));
        let _ = std::fs::remove_dir_all(root);
    }
}
