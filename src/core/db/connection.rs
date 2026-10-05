use super::model::DbError;
use serde_json::{json, Map, Value};

#[derive(Clone, Debug)]
pub(super) struct Connection {
    pub(super) url: String,
    pub(super) driver: String,
    pub(super) user: String,
    pub(super) validation: String,
    pub(super) max_connections: Value,
}

pub(super) fn connection(live: &Value) -> Result<Connection, DbError> {
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
pub(super) fn read_only_connection(mut connection: Connection) -> Result<Connection, DbError> {
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

pub(super) fn connection_table(connection: &Connection, encrypted: &str) -> Value {
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

pub(super) fn secret_error(mut why: String, password: &str) -> DbError {
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
