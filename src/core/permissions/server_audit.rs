//! `permissions audit --server`: the server against the repository and the policy.
//!
//! Read-only. It compares each entity's permission sets with the server's (as `permissions diff`
//! does), the permission helper's tables in helper mode, and what an import cannot carry: the
//! policy's platform grants and memberships, and each role's organizational unit.

use super::audit::{Finding, Loaded, Severity};
use super::helper::{self, Row, Table};
use super::model::ModelEntity;
use super::policy::Platform;
use super::{from_json, Grant, Status};
use crate::core::config_table;
use crate::core::entity_carry::{self, Kind};
use crate::core::entity_key::EntityKey;
use crate::core::progress::{self, Progress};
use crate::core::push;
use crate::core::server::ServerError;
use crate::core::workspace::EntityFile;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// What the server audit reads.
pub trait Remote: entity_carry::Remote + config_table::Remote + push::Remote + Sync {}

impl<T: entity_carry::Remote + config_table::Remote + push::Remote + Sync + ?Sized> Remote for T {}

fn finding(
    severity: Severity,
    code: &'static str,
    entity: Option<String>,
    message: String,
) -> Finding {
    Finding {
        severity,
        code,
        entity,
        message,
        details: Vec::new(),
    }
}

/// The server's findings for one project.
pub fn audit(
    remote: &dyn Remote,
    loaded: &Loaded,
    helper_thing: Option<&ModelEntity>,
) -> Vec<Finding> {
    audit_with_progress(remote, loaded, helper_thing, &progress::NONE)
}

/// Like [`audit`], and report one step per entity read.
pub fn audit_with_progress(
    remote: &dyn Remote,
    loaded: &Loaded,
    helper_thing: Option<&ModelEntity>,
    progress: &dyn Progress,
) -> Vec<Finding> {
    let mut out = Vec::new();
    entities(remote, loaded, &mut out, progress);
    if let Some(helper_thing) = helper_thing {
        helper_tables(remote, helper_thing, &mut out);
    }
    platform(remote, loaded, &mut out);
    units(remote, loaded, &mut out);
    out
}

/// Each entity's permission sets, as `permissions diff` compares them.
fn entities(remote: &dyn Remote, loaded: &Loaded, out: &mut Vec<Finding>, progress: &dyn Progress) {
    let files: Vec<EntityFile> = loaded.entities.iter().map(|e| e.file.clone()).collect();
    let report = super::run_with_progress(&RemoteRef(remote), &files, false, progress);
    for entity in report.entities {
        let key = format!("{}/{}", entity.collection, entity.name);
        match entity.status {
            Status::Differs => out.push(Finding {
                severity: Severity::Error,
                code: "server-differs",
                entity: Some(key),
                message: format!(
                    "the server's permissions differ in {} grant(s); deploy, then `permissions push` makes them the repository's",
                    entity.differences.len()
                ),
                details: entity.differences.iter().map(ToString::to_string).collect(),
            }),
            Status::NotOnServer => out.push(finding(
                Severity::Warning,
                "not-on-server",
                Some(key),
                "the server has no such entity; deploy it".to_string(),
            )),
            Status::Failed => out.push(finding(
                Severity::Error,
                "server-unreadable",
                Some(key),
                entity.error.unwrap_or_default(),
            )),
            _ => {}
        }
    }
}

/// `&dyn Remote` as a sized type `permissions::run` can take.
struct RemoteRef<'a>(&'a dyn Remote);

impl entity_carry::Remote for RemoteRef<'_> {
    fn exists(&self, collection: &str, name: &str) -> Result<bool, ServerError> {
        self.0.exists(collection, name)
    }
    fn get(&self, collection: &str, name: &str, kind: Kind) -> Result<Value, ServerError> {
        self.0.get(collection, name, kind)
    }
    fn set(
        &self,
        collection: &str,
        name: &str,
        kind: Kind,
        value: &Value,
    ) -> Result<(), ServerError> {
        self.0.set(collection, name, kind, value)
    }
    fn differences(&self, collection: &str, name: &str, other: &str) -> Result<usize, ServerError> {
        self.0.differences(collection, name, other)
    }
}

/// A cell of an InfoTable row as the repository writes it.
fn cell(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => match number.as_f64() {
            Some(n) if n.fract() == 0.0 => format!("{n:.1}"),
            _ => number.to_string(),
        },
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn row_key(row: &Row) -> String {
    ["entityName", "resource", "type", "displayName"]
        .iter()
        .filter_map(|field| row.get(*field))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The helper's tables on the server against the repository's. Order does not count.
fn helper_tables(remote: &dyn Remote, helper_thing: &ModelEntity, out: &mut Vec<Finding>) {
    let entity = match crate::core::normalise::entity_of(&helper_thing.bytes) {
        Ok(entity) => entity,
        Err(e) => {
            out.push(finding(
                Severity::Error,
                "server-unreadable",
                Some(helper_thing.key()),
                e.to_string(),
            ));
            return;
        }
    };
    let repository = helper::read_tables(&entity);
    match remote.exists("Things", helper_thing.name()) {
        Ok(true) => {}
        // The entity audit says so already.
        Ok(false) => return,
        Err(e) => {
            out.push(finding(
                Severity::Error,
                "server-unreadable",
                Some(helper_thing.key()),
                e.to_string(),
            ));
            return;
        }
    }
    for name in [
        helper::ROLES_TABLE,
        helper::RUN_TIME_TABLE,
        helper::VISIBILITY_TABLE,
    ] {
        let Some(mine) = repository.get(name) else {
            continue;
        };
        let theirs = match config_table::fetch(&RemoteCall(remote), helper_thing.name(), name) {
            Ok(table) => table,
            Err(e) => {
                out.push(finding(
                    Severity::Error,
                    "server-unreadable",
                    Some(helper_thing.key()),
                    format!("{name}: {e}"),
                ));
                continue;
            }
        };
        let theirs_rows: Vec<Row> = theirs
            .rows
            .iter()
            .map(|row| row.iter().map(|(k, v)| (k.clone(), cell(v))).collect())
            .collect();
        let sorted = |rows: &[Row]| {
            let mut rows = rows.to_vec();
            rows.sort_by(|a, b| row_key(a).cmp(&row_key(b)).then_with(|| a.cmp(b)));
            rows
        };
        let server_fields: BTreeSet<String> = theirs
            .data_shape
            .get("fieldDefinitions")
            .and_then(Value::as_object)
            .map(|fields| fields.keys().cloned().collect())
            .unwrap_or_default();
        let mine_fields: BTreeSet<String> = mine.fields.iter().map(|f| f.name.clone()).collect();
        let mut details = Vec::new();
        if server_fields != mine_fields {
            details.push(format!(
                "{name}: columns differ: server {:?}, repository {:?}",
                server_fields, mine_fields
            ));
        }
        let left = Table {
            data_shape: mine.data_shape.clone(),
            fields: mine.fields.clone(),
            rows: sorted(&theirs_rows),
        };
        let right = Table {
            rows: sorted(&mine.rows),
            ..left.clone()
        };
        if left.rows != right.rows {
            details.extend(
                helper::describe(name, Some(&left), &right)
                    .into_iter()
                    .filter(|line| !line.ends_with("row order differs")),
            );
        }
        if !details.is_empty() {
            out.push(Finding {
                severity: Severity::Error,
                code: "server-helper-differs",
                entity: Some(helper_thing.key()),
                message: format!(
                    "the server's {name} differs from the repository's; an import adds rows but may keep the server's values, so check after a deploy"
                ),
                details,
            });
        }
    }
}

struct RemoteCall<'a>(&'a dyn Remote);

impl config_table::Remote for RemoteCall<'_> {
    fn call(
        &self,
        target: &crate::core::entity_key::ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError> {
        self.0.call(target, service, parameters)
    }
}

/// Whether the project a platform entry requires is on the server.
fn required(remote: &dyn Remote, requires: &Option<String>) -> Result<bool, ServerError> {
    match requires {
        Some(project) => remote.exists("Projects", project),
        None => Ok(true),
    }
}

/// The policy's platform grants and memberships.
fn platform(remote: &dyn Remote, loaded: &Loaded, out: &mut Vec<Finding>) {
    let policy = &loaded.policy;
    for entry in &policy.platform {
        let (requires, roles) = match entry {
            Platform::Grant {
                requires, roles, ..
            }
            | Platform::Member {
                requires, roles, ..
            } => (requires, roles),
        };
        match required(remote, requires) {
            Ok(true) => {}
            Ok(false) => {
                out.push(finding(
                    Severity::Note,
                    "platform-skipped",
                    None,
                    format!(
                        "{} is not on the server, so the entry that requires it is not checked",
                        requires.as_deref().unwrap_or_default()
                    ),
                ));
                continue;
            }
            Err(e) => {
                out.push(finding(
                    Severity::Error,
                    "server-unreadable",
                    None,
                    e.to_string(),
                ));
                continue;
            }
        }
        let groups: BTreeSet<String> = roles
            .iter()
            .flat_map(|role| policy.grantees(role))
            .map(|role| role.group.clone())
            .collect();
        match entry {
            Platform::Grant {
                entity,
                action,
                resource,
                ..
            } => {
                let (collection, name) = entity.split_once('/').unwrap_or((entity, ""));
                let grants = remote
                    .get(collection, name, Kind::RunTime)
                    .map_err(|e| {
                        if e.is_not_found() {
                            format!("the server has no {entity}")
                        } else {
                            e.to_string()
                        }
                    })
                    .and_then(|value| from_json(Kind::RunTime, &value).map_err(|e| e.to_string()));
                let grants = match grants {
                    Ok(grants) => grants,
                    Err(why) => {
                        out.push(finding(
                            Severity::Error,
                            "server-unreadable",
                            Some(entity.clone()),
                            why,
                        ));
                        continue;
                    }
                };
                for group in &groups {
                    let grant = Grant {
                        resource: resource.clone(),
                        action: action.clone(),
                        principal: group.clone(),
                        principal_type: "Group".to_string(),
                    };
                    if grants.get(&grant) != Some(&true) {
                        out.push(finding(
                            Severity::Error,
                            "platform-grant-missing",
                            Some(entity.clone()),
                            format!(
                                "{group} is not allowed {action} on {resource}; `permissions push --platform` grants it"
                            ),
                        ));
                    }
                }
            }
            Platform::Member { group: parent, .. } => {
                let members = match group_members(remote, parent) {
                    Ok(members) => members,
                    Err(why) => {
                        out.push(finding(
                            Severity::Error,
                            "server-unreadable",
                            Some(format!("Groups/{parent}")),
                            why,
                        ));
                        continue;
                    }
                };
                for group in &groups {
                    if !members.contains(group) {
                        out.push(finding(
                            Severity::Error,
                            "membership-missing",
                            Some(format!("Groups/{parent}")),
                            format!(
                                "{group} is not a member; `permissions push --platform` adds it"
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// The names of a group's direct members.
pub fn group_members(remote: &dyn Remote, group: &str) -> Result<BTreeSet<String>, String> {
    let target = crate::core::entity_key::ServiceTarget::entity("Groups", group)
        .map_err(|e| e.to_string())?;
    let reply = remote
        .call(&target, "GetGroupMembers", &json!({}))
        .map_err(|e| {
            if e.is_not_found() {
                format!("the server has no group {group}")
            } else {
                e.to_string()
            }
        })?
        .ok_or_else(|| format!("GetGroupMembers of {group} returned nothing"))?;
    let rows = reply
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("GetGroupMembers of {group} returned no rows"))?;
    Ok(rows
        .iter()
        // A user named like the group is not the group.
        .filter(|row| row.get("type").and_then(Value::as_str).unwrap_or("Group") == "Group")
        .filter_map(|row| row.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect())
}

/// Each role's organizational unit on the server: it must exist and hold the role's group.
fn units(remote: &dyn Remote, loaded: &Loaded, out: &mut Vec<Finding>) {
    let mut organizations: BTreeMap<String, Result<Option<ModelEntity>, String>> = BTreeMap::new();
    for role in &loaded.policy.roles {
        let Some(org) = &role.org else { continue };
        if org.principal_type != "OrganizationalUnit" {
            continue;
        }
        let Some((organization, unit)) = org.name.split_once(':') else {
            continue;
        };
        let key = Some(format!("Organizations/{organization}"));
        if !organizations.contains_key(organization) {
            let model = match EntityKey::address("Organizations", organization)
                .and_then(|key| remote.fetch(&key))
            {
                Ok(Some(bytes)) => {
                    let file = EntityFile {
                        path: std::path::PathBuf::from(format!("{organization}.xml")),
                        info: crate::core::entity::EntityInfo {
                            collection: "Organizations".to_string(),
                            name: organization.to_string(),
                            project: String::new(),
                        },
                        found_under: String::new(),
                    };
                    ModelEntity::of(&file, bytes)
                        .map(Some)
                        .map_err(|e| format!("the server's export does not read: {e}"))
                }
                Ok(None) => Ok(None),
                Err(e) => Err(e.to_string()),
            };
            if let Err(why) = &model {
                out.push(finding(
                    Severity::Error,
                    "server-unreadable",
                    key.clone(),
                    why.clone(),
                ));
            }
            organizations.insert(organization.to_string(), model);
        }
        match &organizations[organization] {
            Err(_) => {}
            Ok(None) => out.push(finding(
                Severity::Error,
                "server-unit-missing",
                key,
                format!("the server has no Organization {organization}, so role {} sees nothing", role.name),
            )),
            Ok(Some(model)) => match model.units.get(unit) {
                None => out.push(finding(
                    Severity::Error,
                    "server-unit-missing",
                    key,
                    format!("the server's Organization has no unit {unit}, so role {} sees nothing", role.name),
                )),
                Some(members) if !members.contains(&role.group) => out.push(finding(
                    Severity::Error,
                    "server-unit-without-group",
                    key,
                    format!("the server's unit {unit} does not hold {}, so role {} sees nothing through it", role.group, role.name),
                )),
                Some(_) => {}
            },
        }
    }
}
