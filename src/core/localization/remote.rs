//! The small live-server surface used by localization operations.

use super::{Header, Token};
use crate::core::entity_key::{EntityKey, ServiceTarget};
use crate::core::imports;
use crate::core::server::{Client, ServerError};
use serde_json::{json, Value};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(120);

/// What localization asks of a server, as a trait so it is tested offline.
pub trait Remote: imports::Remote + Sync {
    fn tables(&self) -> Result<Vec<String>, ServerError>;
    fn tokens(&self, table: &str) -> Result<Vec<Token>, ServerError>;
    fn header(&self, table: &str) -> Result<Header, ServerError>;
    fn delete_token(&self, table: &str, name: &str) -> Result<(), ServerError>;
}

impl Remote for Client {
    fn tables(&self) -> Result<Vec<String>, ServerError> {
        self.list_entity_names("LocalizationTables")
    }

    fn tokens(&self, table: &str) -> Result<Vec<Token>, ServerError> {
        let target = ServiceTarget::entity("LocalizationTables", table)?;
        let reply = self.call_service(&target, "GetTokensAnnotated", &json!({}), TIMEOUT)?;
        let rows = reply
            .as_ref()
            .and_then(|value| value.get("rows"))
            .and_then(Value::as_array)
            .ok_or_else(|| ServerError::InvalidResponse {
                url: target.to_string(),
                why: "GetTokensAnnotated has no rows".to_string(),
            })?;
        Ok(rows
            .iter()
            .map(|row| Token {
                name: text(row, "name"),
                value: text(row, "value"),
                usage: text(row, "usage"),
                context: text(row, "context"),
            })
            .collect())
    }

    fn header(&self, table: &str) -> Result<Header, ServerError> {
        let key = EntityKey::address("LocalizationTables", table)?;
        let value = self.fetch_entity_json(&key)?;
        Ok(Header {
            description: optional_text(&value, "description"),
            language_common: optional_text(&value, "languageCommon"),
            language_native: optional_text(&value, "languageNative"),
        })
    }

    fn delete_token(&self, table: &str, name: &str) -> Result<(), ServerError> {
        let target = ServiceTarget::entity("LocalizationTables", table)?;
        self.call_service(&target, "DeleteToken", &json!({ "name": name }), TIMEOUT)?;
        Ok(())
    }
}

fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn optional_text(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_string)
}
