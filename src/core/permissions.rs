//! An entity's permissions in the repository against the same entity's on the server.
//!
//! An import only adds permissions. Verified on a live 10.1 server (2026-10-07): a grant removed
//! from the repository stays on the server after the next import, and a principal the server
//! already lists keeps its own allow or deny whatever the import says, so a deny written in the
//! repository can be silently ignored. `diff` shows those differences; `push` makes each of the
//! entity's three permission sets exactly the repository's, through the platform's
//! `Set{RunTime,DesignTime,Visibility}PermissionsAsJSON` services, which replace a whole set
//! (verified the same day), and reads each set back.
//!
//! The repository's sets come from the entity XML (`RunTimePermissions`,
//! `DesignTimePermissions`, `VisibilityPermissions`, and a ThingShape's or ThingTemplate's
//! `Instance...Permissions`), the server's from the matching `Get...AsJSON` services. Both are reduced to the same grants: a set, a resource (run time
//! only, `*` for the whole entity), an action, a principal and its type, allowed or denied. A
//! permission set missing from the entity XML is not managed and is never written.

pub mod apply;
pub mod audit;
pub mod helper;
pub mod init;
mod model;
pub mod platform;
pub mod policy;
pub mod server_audit;
pub mod write;

use super::entity_carry::{Kind, Remote};
use super::normalise::{self, Element, Node};
use super::parallel;
use super::workspace::EntityFile;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fmt;

/// The design-time actions, in the order the platform lists them.
const DESIGN_TIME_ACTIONS: [&str; 5] = ["Create", "Read", "Update", "Delete", "Metadata"];
/// The run-time actions of one resource.
const RUN_TIME_ACTIONS: [&str; 5] = [
    "PropertyRead",
    "PropertyWrite",
    "ServiceInvoke",
    "EventInvoke",
    "EventSubscribe",
];

/// One grant, without whether it allows or denies.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Grant {
    /// The run-time resource (`*` for the whole entity); empty for the other two sets.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub resource: String,
    pub action: String,
    pub principal: String,
    pub principal_type: String,
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.resource.is_empty() {
            write!(f, "{} ", self.resource)?;
        }
        write!(
            f,
            "{}: {} {}",
            self.action, self.principal_type, self.principal
        )
    }
}

/// Every grant of one permission set, and whether it allows.
pub type Grants = BTreeMap<Grant, bool>;

/// The permission sets an entity's XML declares. A set the XML has no block for is absent.
pub type Sets = BTreeMap<KindKey, Grants>;

/// [`Kind`] as an ordered map key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KindKey(u8);

impl KindKey {
    pub fn of(kind: Kind) -> Self {
        KindKey(match kind {
            Kind::RunTime => 0,
            Kind::DesignTime => 1,
            Kind::Visibility => 2,
            Kind::InstanceRunTime => 3,
            Kind::InstanceDesignTime => 4,
            Kind::InstanceVisibility => 5,
        })
    }

    pub fn kind(self) -> Kind {
        match self.0 {
            0 => Kind::RunTime,
            1 => Kind::DesignTime,
            2 => Kind::Visibility,
            3 => Kind::InstanceRunTime,
            4 => Kind::InstanceDesignTime,
            _ => Kind::InstanceVisibility,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PermissionsError(String);

fn error(message: impl Into<String>) -> PermissionsError {
    PermissionsError(message.into())
}

/// The permission sets of one entity document.
pub fn from_xml(src: &[u8]) -> Result<Sets, PermissionsError> {
    let entity = normalise::entity_of(src).map_err(|e| error(e.to_string()))?;
    let mut sets = Sets::new();
    for child in elements(&entity) {
        let Some(kind) = Kind::of_element(&child.name) else {
            continue;
        };
        if sets.contains_key(&KindKey::of(kind)) {
            return Err(error(format!(
                "the entity has two {} permission blocks",
                kind.label()
            )));
        }
        let mut grants = Grants::new();
        if kind.is_run_time() {
            for permissions in elements(child).filter(|e| e.name == b"Permissions") {
                let resource = attribute(permissions, "resourceName").unwrap_or("*");
                for action in elements(permissions) {
                    add_principals(&mut grants, resource, action)?;
                }
            }
        } else {
            for action in elements(child) {
                add_principals(&mut grants, "", action)?;
            }
        }
        sets.insert(KindKey::of(kind), grants);
    }
    Ok(sets)
}

fn elements(element: &Element) -> impl Iterator<Item = &Element> {
    element.children.iter().filter_map(|child| match child {
        Node::Element(element) => Some(element),
        _ => None,
    })
}

fn attribute<'a>(element: &'a Element, name: &str) -> Option<&'a str> {
    element
        .attributes
        .iter()
        .find(|(key, _)| key == name.as_bytes())
        .and_then(|(_, value)| std::str::from_utf8(value).ok())
}

fn add_principals(
    grants: &mut Grants,
    resource: &str,
    action: &Element,
) -> Result<(), PermissionsError> {
    let action_name = String::from_utf8_lossy(&action.name).into_owned();
    for principal in elements(action).filter(|e| e.name == b"Principal") {
        let name = attribute(principal, "name")
            .ok_or_else(|| error(format!("a {action_name} principal has no name")))?;
        let principal_type = attribute(principal, "type").unwrap_or_default();
        let permitted = attribute(principal, "isPermitted") != Some("false");
        insert(
            grants,
            Grant {
                resource: resource.to_string(),
                action: action_name.clone(),
                principal: name.to_string(),
                principal_type: principal_type.to_string(),
            },
            permitted,
        )?;
    }
    Ok(())
}

/// Add one grant. A grant listed twice is refused, never resolved by whichever came last: a push
/// replaces whole sets, so guessing could write the wrong allow or deny.
fn insert(grants: &mut Grants, grant: Grant, permitted: bool) -> Result<(), PermissionsError> {
    if grants.contains_key(&grant) {
        return Err(error(format!("{grant} is listed twice")));
    }
    grants.insert(grant, permitted);
    Ok(())
}

/// The grants of one set as the matching `Get...AsJSON` service returns them.
pub fn from_json(kind: Kind, value: &Value) -> Result<Grants, PermissionsError> {
    let mut grants = Grants::new();
    let add = |grants: &mut Grants, resource: &str, action: &str, list: &Value| {
        for principal in list.as_array().into_iter().flatten() {
            let name = principal
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| error(format!("a {action} principal has no name")))?;
            insert(
                grants,
                Grant {
                    resource: resource.to_string(),
                    action: action.to_string(),
                    principal: name.to_string(),
                    principal_type: principal
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                },
                principal.get("isPermitted").and_then(Value::as_bool) != Some(false),
            )?;
        }
        Ok::<_, PermissionsError>(())
    };
    if kind.is_run_time() {
        let resources = value
            .get("permissions")
            .and_then(Value::as_array)
            .ok_or_else(|| error("run-time permissions have no `permissions` list"))?;
        for resource in resources {
            let name = resource
                .get("resourceName")
                .and_then(Value::as_str)
                .unwrap_or("*");
            for (action, list) in resource.as_object().into_iter().flatten() {
                if list.is_array() {
                    add(&mut grants, name, action, list)?;
                }
            }
        }
    } else {
        let object = value
            .as_object()
            .ok_or_else(|| error(format!("{} permissions are not an object", kind.label())))?;
        for (action, list) in object {
            if list.is_array() {
                add(&mut grants, "", action, list)?;
            }
        }
    }
    Ok(grants)
}

/// One set's grants in the form its `Set...AsJSON` service takes.
pub fn to_json(kind: Kind, grants: &Grants) -> Value {
    let principal = |grant: &Grant, permitted: bool| {
        json!({
            "isPermitted": permitted,
            "name": grant.principal,
            "type": grant.principal_type,
        })
    };
    if kind.is_run_time() {
        let mut resources: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
        for (grant, permitted) in grants {
            let resource = resources.entry(grant.resource.as_str()).or_insert_with(|| {
                let mut actions = Map::new();
                for action in RUN_TIME_ACTIONS {
                    actions.insert(action.to_string(), json!([]));
                }
                actions
            });
            if let Some(Value::Array(list)) = resource.get_mut(&grant.action) {
                list.push(principal(grant, *permitted));
            } else {
                resource.insert(grant.action.clone(), json!([principal(grant, *permitted)]));
            }
        }
        let list: Vec<Value> = resources
            .into_iter()
            .map(|(name, mut actions)| {
                actions.insert("resourceName".to_string(), json!(name));
                Value::Object(actions)
            })
            .collect();
        json!({ "permissions": list })
    } else {
        let mut actions = Map::new();
        if kind.is_design_time() {
            for action in DESIGN_TIME_ACTIONS {
                actions.insert(action.to_string(), json!([]));
            }
        } else {
            actions.insert("Visibility".to_string(), json!([]));
        }
        for (grant, permitted) in grants {
            match actions.get_mut(&grant.action) {
                Some(Value::Array(list)) => list.push(principal(grant, *permitted)),
                _ => {
                    actions.insert(grant.action.clone(), json!([principal(grant, *permitted)]));
                }
            }
        }
        Value::Object(actions)
    }
}

/// How a grant differs between the repository and the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Change {
    /// Only the server has it; an import never removes it.
    ServerOnly,
    /// Only the repository has it; the next import adds it.
    RepositoryOnly,
    /// Both have it, one allowing and the other denying; an import keeps the server's.
    Flipped,
}

impl Change {
    pub fn label(self) -> &'static str {
        match self {
            Change::ServerOnly => "server only",
            Change::RepositoryOnly => "repository only",
            Change::Flipped => "allow/deny differs",
        }
    }
}

/// One differing grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Difference {
    #[serde(serialize_with = "serialize_kind")]
    pub set: Kind,
    pub change: Change,
    #[serde(flatten)]
    pub grant: Grant,
    /// Whether the repository allows it; absent when only the server has it.
    pub repository: Option<bool>,
    /// Whether the server allows it; absent when only the repository has it.
    pub server: Option<bool>,
}

fn serialize_kind<S: serde::Serializer>(kind: &Kind, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(kind.label())
}

fn allow(permitted: bool) -> &'static str {
    if permitted {
        "allow"
    } else {
        "deny"
    }
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<20} {:<18} {}",
            self.set.label(),
            self.change.label(),
            self.grant
        )?;
        match (self.repository, self.server) {
            (Some(repository), Some(server)) => write!(
                f,
                " (repository {}, server {})",
                allow(repository),
                allow(server)
            ),
            (Some(permitted), None) | (None, Some(permitted)) => {
                write!(f, " ({})", allow(permitted))
            }
            (None, None) => Ok(()),
        }
    }
}

/// The grants of one set that differ, in grant order.
pub fn differences(kind: Kind, repository: &Grants, server: &Grants) -> Vec<Difference> {
    let mut found = Vec::new();
    for (grant, &permitted) in repository {
        match server.get(grant) {
            None => found.push(Difference {
                set: kind,
                change: Change::RepositoryOnly,
                grant: grant.clone(),
                repository: Some(permitted),
                server: None,
            }),
            Some(&other) if other != permitted => found.push(Difference {
                set: kind,
                change: Change::Flipped,
                grant: grant.clone(),
                repository: Some(permitted),
                server: Some(other),
            }),
            Some(_) => {}
        }
    }
    for (grant, &permitted) in server {
        if !repository.contains_key(grant) {
            found.push(Difference {
                set: kind,
                change: Change::ServerOnly,
                grant: grant.clone(),
                repository: None,
                server: Some(permitted),
            });
        }
    }
    found.sort_by(|a, b| a.grant.cmp(&b.grant));
    found
}

/// Where one entity stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Every managed set is the repository's.
    Same,
    /// At least one set differs; `push --apply` would write it.
    Differs,
    /// `push --apply` wrote the differing sets and read them back equal.
    Pushed,
    /// The server has no such entity; deploy it first.
    NotOnServer,
    /// The entity XML declares no permission set.
    Unmanaged,
    Failed,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Same => "same",
            Status::Differs => "differs",
            Status::Pushed => "pushed",
            Status::NotOnServer => "not on server",
            Status::Unmanaged => "no permissions in the XML",
            Status::Failed => "failed",
        }
    }
}

/// One entity's comparison, and what a push did.
#[derive(Clone, Debug, Serialize)]
pub struct EntityReport {
    pub collection: String,
    pub name: String,
    pub status: Status,
    pub differences: Vec<Difference>,
    /// The sets a push wrote, or would write.
    #[serde(serialize_with = "serialize_kinds")]
    pub sets: Vec<Kind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn serialize_kinds<S: serde::Serializer>(kinds: &[Kind], serializer: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(kinds.len()))?;
    for kind in kinds {
        seq.serialize_element(kind.label())?;
    }
    seq.end()
}

/// Every entity's report, in input order.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub applied: bool,
    pub entities: Vec<EntityReport>,
}

impl Report {
    pub fn count(&self, status: Status) -> usize {
        self.entities
            .iter()
            .filter(|entity| entity.status == status)
            .count()
    }
}

/// Compare each entity's permissions with the server's and, with `apply`, make every differing
/// set the repository's and read it back. Entities run in parallel; one failing never stops the
/// rest.
pub fn run(remote: &(dyn Remote + Sync), entities: &[EntityFile], apply: bool) -> Report {
    let entities = parallel::map(entities, |entity| one(remote, entity, apply));
    Report {
        applied: apply,
        entities,
    }
}

fn one(remote: &(dyn Remote + Sync), entity: &EntityFile, apply: bool) -> EntityReport {
    let collection = entity.info.collection.clone();
    let name = entity.info.name.clone();
    let mut report = EntityReport {
        collection: collection.clone(),
        name: name.clone(),
        status: Status::Same,
        differences: Vec::new(),
        sets: Vec::new(),
        error: None,
    };
    let failed = |mut report: EntityReport, message: String| {
        report.status = Status::Failed;
        report.error = Some(message);
        report
    };
    let sets = match std::fs::read(&entity.path)
        .map_err(|e| error(e.to_string()))
        .and_then(|bytes| from_xml(&bytes))
    {
        Ok(sets) => sets,
        Err(e) => return failed(report, format!("{}: {e}", entity.path.display())),
    };
    if sets.is_empty() {
        report.status = Status::Unmanaged;
        return report;
    }
    match remote.exists(&collection, &name) {
        Ok(true) => {}
        Ok(false) => {
            report.status = Status::NotOnServer;
            return report;
        }
        Err(e) => return failed(report, e.to_string()),
    }
    for (key, wanted) in &sets {
        let kind = key.kind();
        let server = match remote
            .get(&collection, &name, kind)
            .map_err(|e| e.to_string())
            .and_then(|value| from_json(kind, &value).map_err(|e| e.to_string()))
        {
            Ok(grants) => grants,
            Err(e) => return failed(report, format!("reading {} permissions: {e}", kind.label())),
        };
        let found = differences(kind, wanted, &server);
        if !found.is_empty() {
            report.sets.push(kind);
            report.differences.extend(found);
        }
    }
    if report.differences.is_empty() {
        return report;
    }
    report.status = Status::Differs;
    if !apply {
        return report;
    }
    for &kind in &report.sets.clone() {
        let wanted = &sets[&KindKey::of(kind)];
        if let Err(e) = remote.set(&collection, &name, kind, &to_json(kind, wanted)) {
            return failed(report, format!("writing {} permissions: {e}", kind.label()));
        }
        let back = remote
            .get(&collection, &name, kind)
            .map_err(|e| e.to_string())
            .and_then(|value| from_json(kind, &value).map_err(|e| e.to_string()));
        match back {
            Ok(back) if back == *wanted => {}
            Ok(back) => {
                let left = differences(kind, wanted, &back);
                return failed(
                    report,
                    format!(
                        "{} permissions read back with {} difference(s), first: {}",
                        kind.label(),
                        left.len(),
                        left.first().map(ToString::to_string).unwrap_or_default()
                    ),
                );
            }
            Err(e) => {
                return failed(
                    report,
                    format!("reading {} permissions back: {e}", kind.label()),
                )
            }
        }
    }
    report.status = Status::Pushed;
    report
}

#[cfg(test)]
mod apply_tests;
#[cfg(test)]
mod policy_tests;
#[cfg(test)]
mod server_audit_tests;
#[cfg(test)]
mod tests;
