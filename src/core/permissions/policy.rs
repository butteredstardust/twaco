//! A project's permission policy: `permissions.toml` in the project's root folder.
//!
//! The policy is the source of truth. It names the roles (a group each, and the organizational
//! unit that makes entities visible to it), which roles may use which resources, and who sees
//! which entity. From it twaco derives every run-time block it manages and every visibility block;
//! with a Solution Framework permission helper in the project, the helper's tables too (helper
//! mode). Nothing here needs the Solution Framework itself.
//!
//! Rules only allow. A role's grants also reach every role that `includes` it, so a role column
//! reads as that role's whole set, as the permission helper shows it.

use super::model::ModelEntity;
use super::{Grant, Grants};
use crate::core::adopt::glob_matches;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The file name, in a project's root folder.
pub const FILE_NAME: &str = "permissions.toml";

pub const RUN_TIME_ACTIONS: [&str; 5] = super::RUN_TIME_ACTIONS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Helper mode when the project has a permission helper Thing, plain otherwise.
    #[default]
    Auto,
    Helper,
    Plain,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    project: Option<String>,
    #[serde(default)]
    mode: Mode,
    organization: Option<String>,
    #[serde(default)]
    strict: Vec<String>,
    #[serde(default)]
    unmanaged: Vec<String>,
    #[serde(default)]
    role: Vec<RoleFile>,
    #[serde(default)]
    runtime: Vec<RuntimeFile>,
    #[serde(default)]
    visibility: VisibilityFile,
    #[serde(default)]
    platform: Vec<PlatformFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleFile {
    name: String,
    group: String,
    org: Option<String>,
    #[serde(default)]
    includes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeFile {
    entities: Vec<String>,
    action: Option<String>,
    #[serde(default)]
    resources: Vec<String>,
    #[serde(default)]
    except: Vec<String>,
    #[serde(default)]
    entity_wide: bool,
    roles: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct VisibilityFile {
    roles: Option<Vec<String>>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    rule: Vec<VisibilityRuleFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VisibilityRuleFile {
    #[serde(default)]
    types: Vec<String>,
    #[serde(default)]
    names: Vec<String>,
    roles: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlatformFile {
    grant: Option<PlatformGrantFile>,
    member_of: Option<String>,
    roles: Vec<String>,
    requires: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlatformGrantFile {
    entity: String,
    action: Option<String>,
    resource: Option<String>,
}

/// A principal as a permission names it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct Principal {
    pub name: String,
    #[serde(rename = "type")]
    pub principal_type: String,
}

#[derive(Clone, Debug)]
pub struct Role {
    pub name: String,
    /// The group's full name.
    pub group: String,
    /// The principal that makes an entity visible to the role; none for `org = "none"`.
    pub org: Option<Principal>,
    pub includes: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RuntimeRule {
    pub entities: Vec<String>,
    pub action: String,
    /// Patterns over the named resources; `*` is every one of them, never the entity-wide `*`.
    pub resources: Vec<String>,
    /// Names or patterns the rule leaves out.
    pub except: Vec<String>,
    /// Also the entity-wide resource, which ThingWorx writes `*`.
    pub entity_wide: bool,
    pub roles: Vec<String>,
}

impl RuntimeRule {
    /// Whether the rule names this named resource.
    pub fn names_resource(&self, resource: &str) -> bool {
        resource != "*"
            && self.resources.iter().any(|p| glob_matches(p, resource))
            && !self.except.iter().any(|p| glob_matches(p, resource))
    }
}

#[derive(Clone, Debug)]
pub struct VisibilityRule {
    pub types: Vec<String>,
    pub names: Vec<String>,
    pub roles: Vec<String>,
}

/// A grant on, or membership of, an entity the project does not own, which an import cannot
/// carry: the Solution Framework's `DeployComponent` makes these.
#[derive(Clone, Debug)]
pub enum Platform {
    Grant {
        /// `Collection/Name`.
        entity: String,
        action: String,
        resource: String,
        roles: Vec<String>,
        requires: Option<String>,
    },
    Member {
        group: String,
        roles: Vec<String>,
        requires: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub project: String,
    pub path: PathBuf,
    pub mode: Mode,
    pub organization: String,
    pub strict: Vec<String>,
    pub unmanaged: Vec<String>,
    pub roles: Vec<Role>,
    pub runtime: Vec<RuntimeRule>,
    pub visibility_roles: Vec<String>,
    pub visibility_remove: Vec<String>,
    pub visibility_rules: Vec<VisibilityRule>,
    pub platform: Vec<Platform>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {}", .path.display(), .why)]
pub struct PolicyError {
    pub path: PathBuf,
    pub why: String,
}

/// A name in the project unless it is already qualified: `Viewer_UG` in project `Acme.App` is
/// `Acme.App.Viewer_UG`, and `PTC.Base.Default_UG` stays as written.
pub fn qualify(project: &str, name: &str) -> String {
    if name.contains('.') {
        name.to_string()
    } else {
        format!("{project}.{name}")
    }
}

/// Whether an entity pattern names this entity, by its full name or by the part after the
/// project's prefix.
pub fn names_entity(project: &str, pattern: &str, name: &str) -> bool {
    glob_matches(pattern, name)
        || name
            .strip_prefix(project)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|short| glob_matches(pattern, short))
}

impl Policy {
    /// Read and check a project's policy. `Ok(None)` when the project has none.
    pub fn load(project: &str, root: &Path) -> Result<Option<Policy>, PolicyError> {
        let path = root.join(FILE_NAME);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(PolicyError {
                    path,
                    why: e.to_string(),
                })
            }
        };
        Policy::parse(project, &path, &text).map(Some)
    }

    pub fn parse(project: &str, path: &Path, text: &str) -> Result<Policy, PolicyError> {
        let fail = |why: String| PolicyError {
            path: path.to_path_buf(),
            why,
        };
        let file: PolicyFile = toml::from_str(text).map_err(|e| fail(e.to_string()))?;
        if let Some(named) = &file.project {
            if named != project {
                return Err(fail(format!(
                    "it names project {named}, but it is in the folder of project {project}"
                )));
            }
        }
        let organization = qualify(
            project,
            file.organization.as_deref().unwrap_or("Default_OR"),
        );
        let mut roles = Vec::new();
        for role in &file.role {
            if role.name.is_empty() || roles.iter().any(|r: &Role| r.name == role.name) {
                return Err(fail(format!(
                    "role names must be unique and not empty: {:?}",
                    role.name
                )));
            }
            let group = qualify(project, &role.group);
            let org = match role.org.as_deref() {
                None => Some(Principal {
                    name: format!("{organization}:{group}"),
                    principal_type: "OrganizationalUnit".to_string(),
                }),
                Some("none") => None,
                Some("organization") => Some(Principal {
                    name: organization.clone(),
                    principal_type: "Organization".to_string(),
                }),
                Some(other) if other.contains(':') => Some(Principal {
                    name: other.to_string(),
                    principal_type: "OrganizationalUnit".to_string(),
                }),
                Some(other) => Some(Principal {
                    name: qualify(project, other),
                    principal_type: "Organization".to_string(),
                }),
            };
            roles.push(Role {
                name: role.name.clone(),
                group,
                org,
                includes: role.includes.clone(),
            });
        }
        let known = |name: &str| roles.iter().any(|role| role.name == name);
        let check_roles = |what: &str, list: &[String]| -> Result<(), PolicyError> {
            match list.iter().find(|name| !known(name)) {
                Some(name) => Err(fail(format!(
                    "{what} names role {name}, which no [[role]] defines"
                ))),
                None => Ok(()),
            }
        };
        for role in &roles {
            check_roles(&format!("role {}", role.name), &role.includes)?;
        }
        // A cycle would make every role in it include every other: almost certainly a slip.
        for role in &roles {
            let mut seen = BTreeSet::new();
            let mut stack = role.includes.clone();
            while let Some(next) = stack.pop() {
                if next == role.name {
                    return Err(fail(format!("role {} includes itself", role.name)));
                }
                if seen.insert(next.clone()) {
                    if let Some(found) = roles.iter().find(|r| r.name == next) {
                        stack.extend(found.includes.iter().cloned());
                    }
                }
            }
        }
        let mut runtime = Vec::new();
        for (at, rule) in file.runtime.iter().enumerate() {
            let what = format!("[[runtime]] {}", at + 1);
            let action = rule
                .action
                .clone()
                .unwrap_or_else(|| "ServiceInvoke".to_string());
            if !RUN_TIME_ACTIONS.contains(&action.as_str()) {
                return Err(fail(format!(
                    "{what}: action {action} is not one of {}",
                    RUN_TIME_ACTIONS.join(", ")
                )));
            }
            if rule.entities.is_empty() {
                return Err(fail(format!("{what} names no entities")));
            }
            if rule.resources.is_empty() && !rule.entity_wide {
                return Err(fail(format!(
                    "{what} names no resources and is not entity_wide"
                )));
            }
            check_roles(&what, &rule.roles)?;
            runtime.push(RuntimeRule {
                entities: rule.entities.clone(),
                action,
                resources: rule.resources.clone(),
                except: rule.except.clone(),
                entity_wide: rule.entity_wide,
                roles: rule.roles.clone(),
            });
        }
        let visibility_roles = match &file.visibility.roles {
            Some(list) => {
                check_roles("[visibility] roles", list)?;
                list.clone()
            }
            None => roles.iter().map(|role| role.name.clone()).collect(),
        };
        let mut visibility_rules = Vec::new();
        for (at, rule) in file.visibility.rule.iter().enumerate() {
            let what = format!("[[visibility.rule]] {}", at + 1);
            if rule.types.is_empty() && rule.names.is_empty() {
                return Err(fail(format!("{what} names no types and no names")));
            }
            check_roles(&what, &rule.roles)?;
            visibility_rules.push(VisibilityRule {
                types: rule.types.clone(),
                names: rule.names.clone(),
                roles: rule.roles.clone(),
            });
        }
        let mut platform = Vec::new();
        for (at, entry) in file.platform.iter().enumerate() {
            let what = format!("[[platform]] {}", at + 1);
            check_roles(&what, &entry.roles)?;
            match (&entry.grant, &entry.member_of) {
                (Some(grant), None) => {
                    let Some((collection, name)) = grant.entity.split_once('/') else {
                        return Err(fail(format!(
                            "{what}: grant entity {} must be Collection/Name",
                            grant.entity
                        )));
                    };
                    if collection.is_empty() || name.is_empty() {
                        return Err(fail(format!(
                            "{what}: grant entity {} must be Collection/Name",
                            grant.entity
                        )));
                    }
                    let action = grant
                        .action
                        .clone()
                        .unwrap_or_else(|| "ServiceInvoke".to_string());
                    if !RUN_TIME_ACTIONS.contains(&action.as_str()) {
                        return Err(fail(format!(
                            "{what}: action {action} is not a run-time action"
                        )));
                    }
                    platform.push(Platform::Grant {
                        entity: grant.entity.clone(),
                        action,
                        resource: grant.resource.clone().unwrap_or_else(|| "*".to_string()),
                        roles: entry.roles.clone(),
                        requires: entry.requires.clone(),
                    });
                }
                (None, Some(group)) => platform.push(Platform::Member {
                    group: group.clone(),
                    roles: entry.roles.clone(),
                    requires: entry.requires.clone(),
                }),
                _ => {
                    return Err(fail(format!(
                        "{what} needs exactly one of `grant` and `member_of`"
                    )))
                }
            }
        }
        Ok(Policy {
            project: project.to_string(),
            path: path.to_path_buf(),
            mode: file.mode,
            organization,
            strict: file.strict,
            unmanaged: file.unmanaged,
            roles,
            runtime,
            visibility_roles,
            visibility_remove: file.visibility.remove,
            visibility_rules,
            platform,
        })
    }

    pub fn role(&self, name: &str) -> Option<&Role> {
        self.roles.iter().find(|role| role.name == name)
    }

    /// The role itself and every role that includes it, directly or through another.
    pub fn grantees(&self, name: &str) -> Vec<&Role> {
        let mut found: Vec<&Role> = Vec::new();
        let mut stack = vec![name.to_string()];
        while let Some(next) = stack.pop() {
            if found.iter().any(|role| role.name == next) {
                continue;
            }
            if let Some(role) = self.role(&next) {
                found.push(role);
                for other in &self.roles {
                    if other.includes.contains(&next) {
                        stack.push(other.name.clone());
                    }
                }
            }
        }
        found.sort_by_key(|role| self.roles.iter().position(|r| r.name == role.name));
        found
    }

    fn names(&self, pattern: &str, name: &str) -> bool {
        names_entity(&self.project, pattern, name)
    }

    /// Whether the policy leaves this entity's blocks alone.
    pub fn is_unmanaged(&self, name: &str) -> bool {
        self.unmanaged
            .iter()
            .any(|pattern| self.names(pattern, name))
    }

    pub fn is_strict(&self, name: &str) -> bool {
        self.strict.iter().any(|pattern| self.names(pattern, name))
    }

    pub fn rule_names(&self, rule: &RuntimeRule, name: &str) -> bool {
        rule.entities
            .iter()
            .any(|pattern| self.names(pattern, name))
    }

    /// The resources of one action a rule may name on an entity: what it defines, what its block
    /// already lists, and a rule's literal names (an inherited service is not defined here).
    fn resources(&self, entity: &ModelEntity, current: &Grants, action: &str) -> BTreeSet<String> {
        let mut found: BTreeSet<String> = entity.defined(action).clone();
        found.extend(
            current
                .keys()
                .filter(|grant| grant.action == action && grant.resource != "*")
                .map(|grant| grant.resource.clone()),
        );
        for rule in &self.runtime {
            if rule.action == action && self.rule_names(rule, entity.name()) {
                found.extend(rule.resources.iter().filter(|r| !r.contains('*')).cloned());
            }
        }
        found
    }

    /// The run-time grants the policy wants on an entity, given its current block. Every grant
    /// allows, and names a role's group.
    pub fn wanted_run_time(&self, entity: &ModelEntity, current: &Grants) -> Grants {
        let mut wanted = Grants::new();
        for rule in &self.runtime {
            if !self.rule_names(rule, entity.name()) {
                continue;
            }
            let mut resources: Vec<String> = self
                .resources(entity, current, &rule.action)
                .into_iter()
                .filter(|resource| rule.names_resource(resource))
                .collect();
            if rule.entity_wide {
                resources.push("*".to_string());
            }
            for resource in resources {
                for role in &rule.roles {
                    for grantee in self.grantees(role) {
                        wanted.insert(
                            Grant {
                                resource: resource.clone(),
                                action: rule.action.clone(),
                                principal: grantee.group.clone(),
                                principal_type: "Group".to_string(),
                            },
                            true,
                        );
                    }
                }
            }
        }
        wanted
    }

    /// Whether some `ServiceInvoke` rule names this service of the entity. A strict entity's
    /// services must each be classified, even if only to no role.
    pub fn classifies(&self, entity: &str, service: &str) -> bool {
        self.runtime.iter().any(|rule| {
            rule.action == "ServiceInvoke"
                && self.rule_names(rule, entity)
                && rule.names_resource(service)
        })
    }

    /// The roles that see an entity: the first visibility rule that names it, or the default.
    pub fn visible_to(&self, entity: &ModelEntity) -> &[String] {
        for rule in &self.visibility_rules {
            let type_ok =
                rule.types.is_empty() || rule.types.iter().any(|t| t == &entity.entity_type);
            let name_ok =
                rule.names.is_empty() || rule.names.iter().any(|p| self.names(p, entity.name()));
            if type_ok && name_ok {
                return &rule.roles;
            }
        }
        &self.visibility_roles
    }

    /// Every principal a role's visibility uses: the ones the policy owns in a visibility block.
    pub fn managed_visibility(&self) -> BTreeSet<Principal> {
        self.roles
            .iter()
            .filter_map(|role| role.org.clone())
            .collect()
    }

    /// Whether a principal is one the policy removes from every visibility block it manages.
    pub fn removes(&self, principal: &str) -> bool {
        self.visibility_remove
            .iter()
            .any(|pattern| glob_matches(pattern, principal))
    }

    /// The visibility grants the policy wants, given the current block: principals the policy does
    /// not own are kept, unless `remove` names them.
    pub fn wanted_visibility(&self, entity: &ModelEntity, current: &Grants) -> Grants {
        let managed = self.managed_visibility();
        let mut wanted: Grants = current
            .iter()
            .filter(|(grant, _)| {
                !managed.contains(&Principal {
                    name: grant.principal.clone(),
                    principal_type: grant.principal_type.clone(),
                }) && !self.removes(&grant.principal)
            })
            .map(|(grant, allowed)| (grant.clone(), *allowed))
            .collect();
        for name in self.visible_to(entity) {
            if let Some(org) = self.role(name).and_then(|role| role.org.clone()) {
                wanted.insert(
                    Grant {
                        resource: String::new(),
                        action: "Visibility".to_string(),
                        principal: org.name,
                        principal_type: org.principal_type,
                    },
                    true,
                );
            }
        }
        wanted
    }
}

/// Every role's group, keyed by role name.
pub fn groups(policy: &Policy) -> BTreeMap<String, String> {
    policy
        .roles
        .iter()
        .map(|role| (role.name.clone(), role.group.clone()))
        .collect()
}
