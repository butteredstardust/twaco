use super::super::push::Decision;
use serde_json::Value;
use std::fmt;

#[derive(Clone, Debug)]
pub(crate) struct Script {
    pub(crate) entity: String,
    pub(crate) service: String,
    pub(crate) source: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Entity {
    pub(crate) collection: String,
    pub(crate) name: String,
    /// The one-entity source document whose body is present in the bundle.
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectBundle {
    pub(crate) project: String,
    pub(crate) file_name: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) entities: Vec<Entity>,
    pub(crate) scripts: Vec<Script>,
    pub(crate) deploy: Option<ServiceCall>,
    pub(crate) post_import: Vec<ServiceCall>,
}

/// A configured call as it may safely appear in plans: parameters still contain placeholders.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceCall {
    pub(crate) target: String,
    pub(crate) service: String,
    pub(crate) parameters: Value,
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
    #[allow(
        dead_code,
        reason = "read only by Debug; the plan keeps the project it was planned for"
    )]
    pub(crate) project: String,
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
    /// Network connection replacements an import will make. Removing a server connection is
    /// called out because importing a Network replaces its complete hierarchy.
    pub network_changes: Vec<NetworkChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkChange {
    pub name: String,
    pub adds: usize,
    /// Things the import puts under another parent or connects with another type.
    pub moves: usize,
    pub removes: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedCall {
    pub project: String,
    pub call: ServiceCall,
    pub(crate) post_import: bool,
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
pub(crate) struct PlanOptions<'a> {
    pub(crate) only_projects: &'a [String],
    pub(crate) only: &'a [String],
    pub(crate) backend_only: bool,
}
