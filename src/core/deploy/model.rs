use super::super::push::Decision;
use serde_json::Value;
use std::fmt;

#[derive(Clone, Debug)]
pub struct Script {
    pub entity: String,
    pub service: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub collection: String,
    pub name: String,
    /// The one-entity source document whose body is present in the bundle.
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ProjectBundle {
    pub project: String,
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub entities: Vec<Entity>,
    pub scripts: Vec<Script>,
    pub deploy: Option<ServiceCall>,
    pub post_import: Vec<ServiceCall>,
}

/// A configured call as it may safely appear in plans: parameters still contain placeholders.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceCall {
    pub target: String,
    pub service: String,
    pub parameters: Value,
}

impl fmt::Display for ServiceCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parameters = serde_json::to_string(&self.parameters)
            .expect("configured service parameters are JSON");
        write!(f, "{}.{} {parameters}", self.target, self.service)
    }
}

#[derive(Clone, Debug)]
pub struct EntityPlan {
    pub project: String,
    pub collection: String,
    pub name: String,
    pub decision: Decision,
}

#[derive(Clone, Debug)]
pub struct NotKept {
    pub collection: String,
    pub name: String,
    pub sent: String,
    pub read_back: Option<String>,
    pub error: Option<String>,
    /// The read-back differs from what was sent only in its permissions, which an import
    /// cannot take away; `permissions push` can.
    pub only_permissions: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub scripts_checked: usize,
    pub projects: Vec<String>,
    pub plans: Vec<EntityPlan>,
    pub imported: Vec<String>,
    pub kept: Vec<(String, String)>,
    pub not_kept: Vec<NotKept>,
    /// Placeholder-bearing calls only; resolved values never enter a report.
    pub calls: Vec<PlannedCall>,
    pub changed_by_deploy: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedCall {
    pub project: String,
    pub call: ServiceCall,
    pub post_import: bool,
    pub skipped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseFailure {
    pub entity: String,
    pub service: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// What to deploy: which projects, which entities, and whether to leave out UI collections.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlanOptions<'a> {
    pub only_projects: &'a [String],
    pub only: &'a [String],
    pub backend_only: bool,
}
