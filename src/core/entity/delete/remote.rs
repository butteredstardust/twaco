use crate::core::backup;
use crate::core::config::Solution;
use crate::core::entity::key::{EntityKey, ServiceTarget};
use crate::core::server::{Client, ServerError};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

pub trait Remote {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError>;
    fn incoming(&self, key: &EntityKey) -> Result<Vec<Dependent>, ServerError>;
    fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError>;
    fn delete_service(&self, service: &str, name: &str) -> Result<(), ServerError>;
    fn delete_rest(&self, key: &EntityKey) -> Result<(), ServerError>;
    /// Save the server's export of each entity before it is deleted; the set's folder, relative to
    /// the solution, or `None` when none of them exists.
    fn backup(
        &self,
        solution: &Solution,
        entities: &[EntityKey],
        stamp: &str,
    ) -> Result<Option<String>, backup::BackupError>;
}

impl Remote for Client {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        self.entity_exists(key)
    }

    fn incoming(&self, key: &EntityKey) -> Result<Vec<Dependent>, ServerError> {
        incoming_dependencies(self, key)
    }

    fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
        self.fetch_entity(key)
    }

    fn delete_service(&self, service: &str, name: &str) -> Result<(), ServerError> {
        self.call_service(
            &ServiceTarget::platform("Resources", "EntityServices"),
            service,
            &serde_json::json!({ "name": name }),
            Duration::from_secs(120),
        )?;
        Ok(())
    }

    fn delete_rest(&self, key: &EntityKey) -> Result<(), ServerError> {
        self.delete_entity_rest(key)
    }

    fn backup(
        &self,
        solution: &Solution,
        entities: &[EntityKey],
        stamp: &str,
    ) -> Result<Option<String>, backup::BackupError> {
        let set = backup::save(self, solution, "entity delete", entities, stamp)?;
        Ok(set.map(|set| solution.relative(&set.dir)))
    }
}

/// The entities the server records as depending on `key`: `GetIncomingDependencies`, which sees
/// structural references only (see `DEPENDENCY_LIMIT`).
pub(crate) fn incoming_dependencies(
    client: &Client,
    key: &EntityKey,
) -> Result<Vec<Dependent>, ServerError> {
    let target = ServiceTarget::Entity(key.clone());
    let value = client
        .call_service(
            &target,
            "GetIncomingDependencies",
            &serde_json::json!({}),
            Duration::from_secs(120),
        )?
        .unwrap_or_else(|| serde_json::json!({ "rows": [] }));
    let rows = value.get("rows").and_then(Value::as_array).ok_or_else(|| {
        ServerError::InvalidResponse {
            url: format!("{target}/Services/GetIncomingDependencies"),
            why: "expected an InfoTable with rows".to_string(),
        }
    })?;
    rows.iter()
        .map(|row| {
            let name = row.get("name").and_then(Value::as_str).ok_or_else(|| {
                ServerError::InvalidResponse {
                    url: format!("{target}/Services/GetIncomingDependencies"),
                    why: "a row has no string name".to_string(),
                }
            })?;
            let entity_type = row.get("type").and_then(Value::as_str).ok_or_else(|| {
                ServerError::InvalidResponse {
                    url: format!("{target}/Services/GetIncomingDependencies"),
                    why: "a row has no string type".to_string(),
                }
            })?;
            Ok(Dependent {
                collection: dependency_collection(entity_type),
                name: name.to_string(),
            })
        })
        .collect()
}

/// A Thing's dependency services run only while it is enabled; a disabled Thing answers
/// `HTTP 503: Thing [...] is not enabled`, so its dependents are unknown rather than none.
pub(crate) fn is_disabled_thing(key: &EntityKey, error: &ServerError) -> bool {
    key.collection() == "Things"
        && matches!(error, ServerError::Http { status: 503, body, .. } if body.contains("is not enabled"))
}

fn dependency_collection(entity_type: &str) -> String {
    match entity_type {
        "DataShape" => "DataShapes",
        "ThingShape" => "ThingShapes",
        "ThingTemplate" => "ThingTemplates",
        "MediaEntity" => "MediaEntities",
        "StateDefinition" => "StateDefinitions",
        "StyleDefinition" => "StyleDefinitions",
        "StyleTheme" => "StyleThemes",
        other if other.ends_with('s') => other,
        other => return format!("{other}s"),
    }
    .to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Dependent {
    pub collection: String,
    pub name: String,
}
