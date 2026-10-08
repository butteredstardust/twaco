use super::super::entity_key::{EntityKey, ServiceTarget};
use super::super::server::Client;
use serde_json::{json, Value};
use std::time::Duration;

/// The server operations are deliberately small so the entire destructive recipe is testable
/// without a server. Implementations must not retain or log service parameter bodies.
pub trait Remote {
    fn thing(&self, name: &str) -> Result<Value, String>;
    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), String>;
    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, String>;
    fn delete_thing(&self, name: &str) -> Result<(), String>;
    fn thing_exists(&self, name: &str) -> Result<bool, String>;
}

impl Remote for Client {
    fn thing(&self, name: &str) -> Result<Value, String> {
        EntityKey::address("Things", name)
            .and_then(|key| self.fetch_entity_json(&key))
            .map_err(|error| error.to_string())
    }

    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), String> {
        self.import_entity(file_name, xml)
            .map_err(|error| error.to_string())
    }

    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, String> {
        self.call_service(target, service, parameters, timeout)
            .map_err(|error| error.to_string())
    }

    fn delete_thing(&self, name: &str) -> Result<(), String> {
        self.call_service(
            &ServiceTarget::platform("Resources", "EntityServices"),
            "DeleteThing",
            &json!({ "name": name }),
            Duration::from_secs(120),
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn thing_exists(&self, name: &str) -> Result<bool, String> {
        EntityKey::address("Things", name)
            .and_then(|key| self.entity_exists(&key))
            .map_err(|error| error.to_string())
    }
}
