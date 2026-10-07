//! `permissions push --platform`: the policy's grants and memberships on entities the project does
//! not own.
//!
//! These are what the Solution Framework's `DeployComponent` does after an import, and what no
//! import can carry: a run-time grant on a platform Resource, or a role group's membership of a
//! shared group. Those entities belong to every block on the server, so a push only adds, one
//! grant or member at a time (`AddRunTimePermission`, `AddMember`), and reads each back. It
//! never removes anything.

use super::audit::Loaded;
use super::from_json;
use super::policy::Platform;
use super::server_audit::{group_members, Remote};
use super::Grant;
use crate::core::entity_carry::Kind;
use crate::core::entity_key::ServiceTarget;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// The server has it.
    Present,
    /// The server lacks it; `--apply` adds it.
    Missing,
    /// Added and read back.
    Added,
    /// The `requires` project is not on the server.
    Skipped,
    Failed,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Present => "present",
            State::Missing => "missing",
            State::Added => "added",
            State::Skipped => "skipped",
            State::Failed => "failed",
        }
    }
}

/// One grant or membership for one role group.
#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub project: String,
    /// `Collection/Name` of the entity granted on, or of the group joined.
    pub entity: String,
    /// `ServiceInvoke ReadEntityDefinitionAsJSON`, or `member`.
    pub what: String,
    pub group: String,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PlatformReport {
    pub applied: bool,
    pub items: Vec<Item>,
}

impl PlatformReport {
    pub fn count(&self, state: State) -> usize {
        self.items.iter().filter(|item| item.state == state).count()
    }
}

/// Compare every project's platform entries with the server and, with `apply`, add what is
/// missing.
pub fn run(remote: &dyn Remote, loaded: &[Loaded], apply: bool) -> PlatformReport {
    let mut report = PlatformReport {
        applied: apply,
        items: Vec::new(),
    };
    for one in loaded {
        let policy = &one.policy;
        for entry in &policy.platform {
            let (requires, roles) = match entry {
                Platform::Grant {
                    requires, roles, ..
                }
                | Platform::Member {
                    requires, roles, ..
                } => (requires, roles),
            };
            let groups: BTreeSet<String> = roles
                .iter()
                .flat_map(|role| policy.grantees(role))
                .map(|role| role.group.clone())
                .collect();
            let (entity, what) = match entry {
                Platform::Grant {
                    entity,
                    action,
                    resource,
                    ..
                } => (entity.clone(), format!("{action} {resource}")),
                Platform::Member { group, .. } => (format!("Groups/{group}"), "member".to_string()),
            };
            let item = |group: &str, state: State, error: Option<String>| Item {
                project: policy.project.clone(),
                entity: entity.clone(),
                what: what.clone(),
                group: group.to_string(),
                state,
                error,
            };
            let present = match requires {
                Some(project) => remote.exists("Projects", project),
                None => Ok(true),
            };
            match present {
                Ok(true) => {}
                Ok(false) => {
                    report
                        .items
                        .extend(groups.iter().map(|g| item(g, State::Skipped, None)));
                    continue;
                }
                Err(e) => {
                    report.items.extend(
                        groups
                            .iter()
                            .map(|g| item(g, State::Failed, Some(e.to_string()))),
                    );
                    continue;
                }
            }
            let has = |group: &str| -> Result<bool, String> {
                match entry {
                    Platform::Grant {
                        entity,
                        action,
                        resource,
                        ..
                    } => {
                        let (collection, name) = entity.split_once('/').unwrap_or((entity, ""));
                        let value = remote
                            .get(collection, name, Kind::RunTime)
                            .map_err(|e| e.to_string())?;
                        let grants = from_json(Kind::RunTime, &value).map_err(|e| e.to_string())?;
                        Ok(grants.get(&Grant {
                            resource: resource.clone(),
                            action: action.clone(),
                            principal: group.to_string(),
                            principal_type: "Group".to_string(),
                        }) == Some(&true))
                    }
                    Platform::Member { group: parent, .. } => {
                        Ok(group_members(remote, parent)?.contains(group))
                    }
                }
            };
            for group in &groups {
                let state = match has(group) {
                    Ok(true) => State::Present,
                    Ok(false) if !apply => State::Missing,
                    Ok(false) => match add(remote, entry, group).and_then(|()| has(group)) {
                        Ok(true) => State::Added,
                        Ok(false) => {
                            report.items.push(item(
                                group,
                                State::Failed,
                                Some("the server answered, but does not show it after".to_string()),
                            ));
                            continue;
                        }
                        Err(why) => {
                            report.items.push(item(group, State::Failed, Some(why)));
                            continue;
                        }
                    },
                    Err(why) => {
                        report.items.push(item(group, State::Failed, Some(why)));
                        continue;
                    }
                };
                report.items.push(item(group, state, None));
            }
        }
    }
    report
}

/// Add one grant or membership, as `DeployComponent` does.
fn add(remote: &dyn Remote, entry: &Platform, group: &str) -> Result<(), String> {
    match entry {
        Platform::Grant {
            entity,
            action,
            resource,
            ..
        } => {
            let (collection, name) = entity.split_once('/').unwrap_or((entity, ""));
            let target = ServiceTarget::entity(collection, name).map_err(|e| e.to_string())?;
            remote
                .call(
                    &target,
                    "AddRunTimePermission",
                    &json!({
                        "allow": true,
                        "type": action,
                        "resource": resource,
                        "principal": group,
                        "principalType": "Group",
                    }),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        Platform::Member { group: parent, .. } => {
            let target = ServiceTarget::entity("Groups", parent).map_err(|e| e.to_string())?;
            remote
                .call(
                    &target,
                    "AddMember",
                    &json!({ "member": group, "type": "Group" }),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }
}
