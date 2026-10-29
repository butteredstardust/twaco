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
    pub(crate) server_owned: Vec<super::super::server_owned::Part>,
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
    /// `deploy` is a building block's deploy step, run as the framework runs it.
    pub(crate) framework: bool,
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
    /// Owned parts the import carries from the server.
    pub server_owned: Vec<String>,
    /// Owned parts holding an encrypted value, which the import clears (a deploy hook sets it).
    pub cleared: Vec<String>,
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
    /// For a Project, the projects the server added to its `dependsOn` when that is all that
    /// differs: entities sent with it need them.
    pub added_dependencies: Vec<String>,
    /// Up to five paths where the read-back differs from what was sent, as the hash compares
    /// them; names only, never values.
    pub differences: Vec<String>,
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
    /// Agents imported first without allowances the deploy's namespaces drop, which the server
    /// refuses while an agent on it allows them.
    pub agents_narrowed: Vec<super::AgentNarrowing>,
    /// Each building block's deploy step (`[project.deploy] framework = true`).
    pub blocks: Vec<super::BlockStep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkChange {
    pub name: String,
    pub adds: usize,
    /// Things the import puts under another parent or connects with another type.
    pub moves: usize,
    pub removes: usize,
}

impl PlannedCall {
    /// Whether it runs after the project's deploy service rather than being it.
    pub fn is_post_import(&self) -> bool {
        self.post_import
    }
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
    pub(crate) changed_since: Option<&'a str>,
    pub(crate) exclude: &'a [String],
}
