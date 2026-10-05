use super::super::config::Solution;
use super::super::entity_key::ServiceTarget;
use super::super::profile::Profile;
use super::connection::{
    connection, connection_table, read_only_connection, secret_error, without_password, Connection,
};
use super::model::{DbError, Mode, Options, Report};
use super::remote::Remote;
use super::resolve::resolve_thing;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NAME_COUNTER: AtomicU64 = AtomicU64::new(0);

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
                &ServiceTarget::platform("Resources", "EncryptionServices"),
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
                &ServiceTarget::entity("Things", &temporary)
                    .map_err(|error| secret_error(error.to_string(), password))?,
                "SetConfigurationTable",
                &json!({ "tableName": "ConnectionInfo", "configurationTable": table }),
                options.timeout,
            )
            .map_err(|why| secret_error(why, password))?;
        remote
            .call(
                &ServiceTarget::entity("Things", &temporary)
                    .map_err(|error| secret_error(error.to_string(), password))?,
                "RestartThing",
                &json!({}),
                options.timeout,
            )
            .map_err(|why| secret_error(why, password))?;
        remote
            .call(
                &ServiceTarget::entity("Things", &temporary)
                    .map_err(|error| secret_error(error.to_string(), password))?,
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

pub(super) fn thing_xml(
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

pub(super) fn temporary_name() -> String {
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
