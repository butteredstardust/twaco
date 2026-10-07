//! `permissions init`: draft a project's `permissions.toml` from what the project grants today.
//!
//! The draft is exact, not tidy: each rule names its roles outright (no `includes`), so applying
//! it straight after changes nothing, except what its notes name (helper tables that disagree
//! with the entity XML). Roles come from the permission helper's
//! `RoleGroupsAndOrganizations` when the project has a helper (so its columns survive), and from
//! the groups the run-time blocks grant otherwise. Grants come from the entity XML, or with
//! `from_helper` from the helper's tables, which is how a matrix edited in the helper's mashup
//! comes into the repository. An entity whose run-time block holds what a policy cannot say (a
//! deny, a principal that is not a group) is left `unmanaged`, with a note.

use super::audit::run_time_kind;
use super::helper::{self, Row};
use super::model::ModelEntity;
use super::policy::{self, Policy};
use super::{Grant, Grants, KindKey, PermissionsError};
use crate::core::config::Solution;
use crate::core::entity_carry::Kind;
use crate::core::normalise;
use crate::core::workspace;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize)]
pub struct Draft {
    pub project: String,
    #[serde(skip)]
    pub path: PathBuf,
    /// Where the grants came from: `entity XML` or the helper Thing.
    pub source: String,
    pub text: String,
    /// What the draft could not say, and so left alone.
    pub notes: Vec<String>,
}

#[derive(Debug)]
pub enum InitError {
    Project(String),
    Unreadable(Vec<String>),
    Entity(PermissionsError),
    /// `from_helper` for a project without one.
    NoHelper(String),
    /// Every project asked for already has a policy.
    Nothing(Vec<String>),
    /// The helper's tables cannot be drafted from as they are.
    Helper(String),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InitError::Project(name) => write!(f, "this solution has no project named {name}"),
            InitError::Unreadable(items) => f.write_str(&items.join("\n")),
            InitError::Helper(why) => f.write_str(why),
            InitError::Entity(error) => error.fmt(f),
            InitError::NoHelper(project) => write!(
                f,
                "project {project} has no permission helper Thing; draft from the entity XML instead"
            ),
            InitError::Nothing(projects) => write!(
                f,
                "{} already {} a permissions.toml; edit it, or remove it to draft again",
                projects.join(", "),
                if projects.len() == 1 { "has" } else { "have" }
            ),
        }
    }
}

impl std::error::Error for InitError {}

impl crate::core::codes::Coded for InitError {
    fn code(&self) -> crate::core::codes::ErrorCode {
        use crate::core::codes::ErrorCode;
        match self {
            InitError::Project(_) | InitError::NoHelper(_) | InitError::Nothing(_) => {
                ErrorCode::InvalidArguments
            }
            InitError::Unreadable(_) | InitError::Entity(_) | InitError::Helper(_) => {
                ErrorCode::InvalidData
            }
        }
    }
}

/// A run-time rule being drafted: action, roles, resources, and whether it is entity-wide.
type RuleKey = (String, Vec<String>, Vec<String>, bool);

/// A role being drafted.
#[derive(Clone, Debug)]
struct DraftRole {
    name: String,
    group: String,
    /// `None`: the default unit; `Some("none")`, `Some("organization")` or a full principal.
    org: Option<String>,
}

/// Draft a policy for each project without one (or the one named).
pub fn draft(
    solution: &Solution,
    project: Option<&str>,
    from_helper: bool,
) -> Result<Vec<Draft>, InitError> {
    if let Some(name) = project {
        if solution.project(name).is_none() {
            return Err(InitError::Project(name.to_string()));
        }
    }
    let found = workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(InitError::Unreadable(found.unreadable));
    }
    let mut out = Vec::new();
    let mut have = Vec::new();
    for configured in &solution.projects {
        if project.is_some_and(|name| name != configured.name) {
            continue;
        }
        let root = solution.project_root(configured);
        if root.join(policy::FILE_NAME).exists() {
            have.push(configured.name.clone());
            continue;
        }
        let entities = found
            .entities
            .iter()
            .filter(|file| file.found_under == configured.name)
            .map(ModelEntity::read)
            .collect::<Result<Vec<_>, _>>()
            .map_err(InitError::Entity)?;
        let all: BTreeMap<String, ModelEntity> = found
            .entities
            .iter()
            .filter(|file| file.info.collection == "Organizations")
            .filter_map(|file| ModelEntity::read(file).ok())
            .map(|e| (e.name().to_string(), e))
            .collect();
        out.push(draft_project(
            &configured.name,
            root.join(policy::FILE_NAME),
            &entities,
            &all,
            from_helper,
        )?);
    }
    if out.is_empty() && !have.is_empty() {
        return Err(InitError::Nothing(have));
    }
    Ok(out)
}

/// `DashboardViewer_UG` -> `dashboardViewer`, as the permission helper names a role.
fn role_name(project: &str, group: &str) -> String {
    let short = group
        .strip_prefix(project)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or_else(|| group.rsplit('.').next().unwrap_or(group));
    let short = short.strip_suffix("_UG").unwrap_or(short);
    let mut chars = short.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => "role".to_string(),
    }
}

/// The name a draft writes for an entity: the part after the project's prefix, unless that
/// would also name another entity of the project (one called `X` beside `Acme.App.X`).
fn written_name(project: &str, name: &str, entities: &[ModelEntity]) -> String {
    let candidate = short(project, name);
    let clash = entities.iter().any(|other| {
        other.name() != name && policy::names_entity(project, candidate, other.name())
    });
    if clash {
        name.to_string()
    } else {
        candidate.to_string()
    }
}

fn short<'a>(project: &str, name: &'a str) -> &'a str {
    name.strip_prefix(project)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(name)
}

fn quote(text: &str) -> String {
    toml::Value::String(text.to_string()).to_string()
}

fn list(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|i| quote(i))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// A list on several lines once it is long, as a person would write it.
fn long_list(items: &[String]) -> String {
    let one = list(items);
    if one.len() <= 80 {
        return one;
    }
    let mut out = "[\n".to_string();
    for item in items {
        out.push_str(&format!("    {},\n", quote(item)));
    }
    out.push(']');
    out
}

fn draft_project(
    project: &str,
    path: PathBuf,
    entities: &[ModelEntity],
    organizations: &BTreeMap<String, ModelEntity>,
    from_helper: bool,
) -> Result<Draft, InitError> {
    let mut notes = Vec::new();
    let organization = format!("{project}.Default_OR");
    let helpers: Vec<&ModelEntity> = entities.iter().filter(|e| e.is_helper()).collect();
    let tables = match helpers.as_slice() {
        [one] => Some(helper::read_tables(
            &normalise::entity_of(&one.bytes)
                .map_err(|e| InitError::Entity(PermissionsError(e.to_string())))?,
        )),
        _ => None,
    };
    if from_helper && tables.is_none() {
        return Err(InitError::NoHelper(project.to_string()));
    }

    // Roles: the helper's, in its column order; then any other group a block grants.
    let mut roles: Vec<DraftRole> = Vec::new();
    if let Some(tables) = &tables {
        let mut order: Vec<(u32, String)> = tables
            .get(helper::RUN_TIME_TABLE)
            .map(|t| {
                t.fields
                    .iter()
                    .filter_map(|f| {
                        f.name
                            .strip_suffix("Group")
                            .map(|n| (f.ordinal, n.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        order.sort();
        let rows: Vec<&Row> = tables
            .get(helper::ROLES_TABLE)
            .map(|t| t.rows.iter().collect())
            .unwrap_or_default();
        let lookup = |display: &str| {
            rows.iter()
                .find(|r| r.get("displayName").map(String::as_str) == Some(display))
        };
        let mut names: Vec<String> = order.into_iter().map(|(_, n)| n).collect();
        for row in &rows {
            if let Some(name) = row.get("displayName").and_then(|d| d.strip_suffix("Group")) {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
        }
        for name in names {
            let Some(group) = lookup(&format!("{name}Group")).and_then(|r| r.get("principal"))
            else {
                continue;
            };
            if !group.contains('.') {
                return Err(InitError::Helper(format!(
                    "role {name} of the permission helper maps to group {group}; a policy reads a \
                     name without a dot as the project's, so rename the group or map the role to a \
                     dotted one first"
                )));
            }
            let org = match lookup(&format!("{name}Org")) {
                None => Some("none".to_string()),
                Some(row) => {
                    let principal = row.get("principal").cloned().unwrap_or_default();
                    if principal == format!("{organization}:{group}") {
                        None
                    } else if principal == organization {
                        Some("organization".to_string())
                    } else {
                        Some(principal)
                    }
                }
            };
            roles.push(DraftRole {
                name,
                group: group.clone(),
                org,
            });
        }
    }
    let unit_exists = |group: &str| {
        organizations
            .get(&organization)
            .is_some_and(|org| org.units.contains_key(group))
    };

    // Grants per entity: from the blocks, or from the helper's matrix.
    let mut run_time: BTreeMap<String, Grants> = BTreeMap::new();
    let mut unmanaged: BTreeSet<String> = BTreeSet::new();
    for entity in entities {
        let Some(kind) = run_time_kind(entity) else {
            continue;
        };
        let grants = if from_helper {
            from_rows(tables.as_ref().unwrap(), entity, &roles)
        } else {
            entity
                .sets
                .get(&KindKey::of(kind))
                .cloned()
                .unwrap_or_default()
        };
        let odd: Vec<String> = grants
            .iter()
            .filter(|(g, allowed)| !**allowed || g.principal_type != "Group")
            .map(|(g, allowed)| format!("{g}{}", if *allowed { "" } else { " (deny)" }))
            .collect();
        if !odd.is_empty() {
            notes.push(format!(
                "{} is left unmanaged: a policy only allows groups, and its {} block has {}",
                entity.key(),
                kind.label(),
                odd.join(", ")
            ));
            unmanaged.insert(entity.name().to_string());
            continue;
        }
        let dotless: Vec<&str> = grants
            .keys()
            .filter(|g| !g.principal.contains('.'))
            .map(|g| g.principal.as_str())
            .collect();
        if !dotless.is_empty() {
            notes.push(format!(
                "{} is left unmanaged: a policy names a group outside the project with its dotted name, and {} has none",
                entity.key(),
                dotless.join(", ")
            ));
            unmanaged.insert(entity.name().to_string());
            continue;
        }
        for grant in grants.keys() {
            if !roles.iter().any(|r| r.group == grant.principal) {
                let mut name = role_name(project, &grant.principal);
                while roles.iter().any(|r| r.name == name) {
                    name.push('_');
                }
                roles.push(DraftRole {
                    name,
                    org: if unit_exists(&grant.principal) {
                        None
                    } else {
                        Some("none".to_string())
                    },
                    group: grant.principal.clone(),
                });
            }
        }
        run_time.insert(entity.name().to_string(), grants);
    }
    let role_of = |group: &str| {
        roles
            .iter()
            .find(|r| r.group == group)
            .map(|r| r.name.clone())
    };
    let org_principal = |role: &DraftRole| -> Option<(String, String)> {
        match role.org.as_deref() {
            None => Some((
                format!("{organization}:{}", role.group),
                "OrganizationalUnit".to_string(),
            )),
            Some("none") => None,
            Some("organization") => Some((organization.clone(), "Organization".to_string())),
            Some(other) if other.contains(':') => {
                Some((other.to_string(), "OrganizationalUnit".to_string()))
            }
            Some(other) => Some((other.to_string(), "Organization".to_string())),
        }
    };

    // A visibility deny of a principal the draft's roles own cannot be said either: the policy
    // would own that principal and drop the deny.
    let owned: Vec<(String, String)> = roles.iter().filter_map(org_principal).collect();
    for entity in entities {
        if unmanaged.contains(entity.name()) {
            continue;
        }
        let denied: Vec<String> = entity
            .sets
            .get(&KindKey::of(Kind::Visibility))
            .into_iter()
            .flatten()
            .filter(|(g, allowed)| {
                !**allowed
                    && owned
                        .iter()
                        .any(|(name, kind)| *name == g.principal && *kind == g.principal_type)
            })
            .map(|(g, _)| g.principal.clone())
            .collect();
        if !denied.is_empty() {
            notes.push(format!(
                "{} is left unmanaged: a policy only allows, and its visibility block denies {}",
                entity.key(),
                denied.join(", ")
            ));
            unmanaged.insert(entity.name().to_string());
            run_time.remove(entity.name());
        }
    }

    // Run-time rules: entities granting the same resources of one action to the same roles share one.
    let role_rank = |name: &str| {
        roles
            .iter()
            .position(|r| r.name == name)
            .unwrap_or(usize::MAX)
    };
    let mut rules: BTreeMap<RuleKey, Vec<String>> = BTreeMap::new();
    let mut first_seen: Vec<RuleKey> = Vec::new();
    for entity in entities {
        let Some(grants) = run_time.get(entity.name()) else {
            continue;
        };
        // (action, roles) -> resources
        let mut by_resource: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
        for grant in grants.keys() {
            if let Some(role) = role_of(&grant.principal) {
                by_resource
                    .entry((grant.action.clone(), grant.resource.clone()))
                    .or_default()
                    .insert(role);
            }
        }
        let mut groups: BTreeMap<(String, Vec<String>), (Vec<String>, bool)> = BTreeMap::new();
        for ((action, resource), granted) in by_resource {
            let mut granted: Vec<String> = granted.into_iter().collect();
            granted.sort_by_key(|r| role_rank(r));
            let slot = groups.entry((action, granted)).or_default();
            if resource == "*" {
                slot.1 = true;
            } else {
                slot.0.push(resource);
            }
        }
        for ((action, granted), (resources, entity_wide)) in groups {
            let key = (action, granted, resources, entity_wide);
            if !rules.contains_key(&key) {
                first_seen.push(key.clone());
            }
            rules
                .entry(key)
                .or_default()
                .push(written_name(project, entity.name(), entities));
        }
    }

    // Visibility: who sees each entity; the most common set is the default.
    let mut seen_by: Vec<(&ModelEntity, Vec<String>)> = Vec::new();
    let visibility_rows: Option<&Vec<Row>> = if from_helper {
        tables
            .as_ref()
            .and_then(|t| t.get(helper::VISIBILITY_TABLE))
            .map(|t| &t.rows)
    } else {
        None
    };
    for entity in entities {
        if unmanaged.contains(entity.name()) {
            continue;
        }
        let mut who: Vec<String> = match visibility_rows {
            Some(rows) => {
                let row = rows
                    .iter()
                    .find(|r| r.get("entityName").map(String::as_str) == Some(entity.name()));
                roles
                    .iter()
                    .filter(|role| {
                        org_principal(role).is_some()
                            && row
                                .and_then(|r| r.get(&format!("{}Org", role.name)))
                                .map(String::as_str)
                                == Some("true")
                    })
                    .map(|r| r.name.clone())
                    .collect()
            }
            None => {
                let current = entity.sets.get(&KindKey::of(Kind::Visibility));
                roles
                    .iter()
                    .filter(|role| {
                        org_principal(role).is_some_and(|(name, kind)| {
                            current.is_some_and(|grants| {
                                grants.get(&Grant {
                                    resource: String::new(),
                                    action: "Visibility".to_string(),
                                    principal: name,
                                    principal_type: kind,
                                }) == Some(&true)
                            })
                        })
                    })
                    .map(|r| r.name.clone())
                    .collect()
            }
        };
        who.sort_by_key(|r| role_rank(r));
        seen_by.push((entity, who));
    }
    let mut counts: BTreeMap<&Vec<String>, usize> = BTreeMap::new();
    for (_, who) in &seen_by {
        *counts.entry(who).or_default() += 1;
    }
    let default: Vec<String> = counts
        .iter()
        .max_by_key(|(who, n)| (**n, who.len()))
        .map(|(who, _)| (*who).clone())
        .unwrap_or_default();
    let all_visible: Vec<String> = roles.iter().map(|r| r.name.clone()).collect();
    let mut exceptions: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
    for (entity, who) in &seen_by {
        // An entity without a visibility block keeps none, so it needs a rule of its own.
        let has_block = entity.sets.contains_key(&KindKey::of(Kind::Visibility));
        if *who != default || (!has_block && !default.is_empty()) {
            let key = if has_block { who.clone() } else { Vec::new() };
            exceptions
                .entry(key)
                .or_default()
                .push(written_name(project, entity.name(), entities));
        }
    }

    // The text.
    let source = if from_helper {
        format!("the permission helper {}", helpers[0].name())
    } else {
        "the entity XML".to_string()
    };
    let mut text = format!(
        "# Who may use {project}: drafted by `twaco permissions init` from {source}.\n\
         # Each rule names its roles outright; `includes` on a role, and patterns such as \"Get*\",\n\
         # can make it shorter. `twaco permissions audit` checks the repository against it.\n\
         project = {}\n",
        quote(project)
    );
    if !unmanaged.is_empty() {
        let names: Vec<String> = unmanaged
            .iter()
            .map(|n| short(project, n).to_string())
            .collect();
        text.push_str(&format!("unmanaged = {}\n", long_list(&names)));
    }
    for role in &roles {
        // A name without a dot is read as the project's: a group from elsewhere is written whole.
        let group = if role.group.starts_with(&format!("{project}.")) {
            short(project, &role.group)
        } else {
            &role.group
        };
        text.push_str(&format!(
            "\n[[role]]\nname = {}\ngroup = {}\n",
            quote(&role.name),
            quote(group)
        ));
        if let Some(org) = &role.org {
            text.push_str(&format!("org = {}\n", quote(org)));
        }
    }
    for key in &first_seen {
        let (action, granted, resources, entity_wide) = key;
        let entities = &rules[key];
        text.push_str(&format!(
            "\n[[runtime]]\nentities = {}\n",
            long_list(entities)
        ));
        if action != "ServiceInvoke" {
            text.push_str(&format!("action = {}\n", quote(action)));
        }
        if !resources.is_empty() {
            text.push_str(&format!("resources = {}\n", long_list(resources)));
        }
        if *entity_wide {
            text.push_str("entity_wide = true\n");
        }
        text.push_str(&format!("roles = {}\n", list(granted)));
    }
    if default != all_visible || !exceptions.is_empty() {
        text.push_str("\n[visibility]\n");
        if default != all_visible {
            text.push_str(&format!("roles = {}\n", list(&default)));
        }
    }
    for (who, names) in &exceptions {
        text.push_str(&format!(
            "\n[[visibility.rule]]\nnames = {}\nroles = {}\n",
            long_list(names),
            list(who)
        ));
    }
    // The draft must read as a policy, and say what the project has: apply it in memory.
    let parsed = Policy::parse(project, &path, &text)
        .map_err(|e| InitError::Entity(PermissionsError(e.to_string())))?;
    let mut blocks = Vec::new();
    for entity in entities {
        if parsed.is_unmanaged(entity.name()) {
            continue;
        }
        if let Some(change) = super::apply::change_of(&parsed, entity)
            .map_err(|e| InitError::Entity(PermissionsError(e.to_string())))?
        {
            blocks.push(change.entity);
        }
    }
    if !blocks.is_empty() {
        notes.push(if from_helper {
            format!(
                "`permissions apply` will write the helper's matrix into {}",
                blocks.join(", ")
            )
        } else {
            format!(
                "`permissions apply` would still change {}",
                blocks.join(", ")
            )
        });
    }
    if let [helper] = helpers.as_slice() {
        let loaded = super::audit::Loaded {
            policy: parsed,
            entities: entities.to_vec(),
            all: Default::default(),
            projects: Default::default(),
        };
        let read = |entity: &ModelEntity| Ok(entity.bytes.to_vec());
        let edits = super::helper::edits(&loaded, helper, &read)
            .map_err(|e| InitError::Helper(format!("{}: {e}", helper.key())))?;
        let details: Vec<String> = edits.into_iter().flat_map(|edit| edit.details).collect();
        if !details.is_empty() {
            if from_helper {
                return Err(InitError::Helper(format!(
                    "the permission helper's tables do not say everything a policy needs, so a \
                     draft from them would not keep them; `permissions init` from the entity XML \
                     rewrites them instead:\n  {}",
                    details.join("\n  ")
                )));
            }
            notes.push(format!(
                "`permissions apply` will rewrite the permission helper's tables to agree with the \
                 entity XML: {}",
                details.join("; ")
            ));
        }
    }
    Ok(Draft {
        project: project.to_string(),
        path,
        source,
        text,
        notes,
    })
}

/// The run-time grants the helper's matrix gives an entity.
fn from_rows(
    tables: &BTreeMap<String, helper::Table>,
    entity: &ModelEntity,
    roles: &[DraftRole],
) -> Grants {
    let mut grants = Grants::new();
    let Some(table) = tables.get(helper::RUN_TIME_TABLE) else {
        return grants;
    };
    for row in &table.rows {
        if row.get("entityName").map(String::as_str) != Some(entity.name()) {
            continue;
        }
        let (Some(resource), Some(action)) = (row.get("resource"), row.get("type")) else {
            continue;
        };
        for role in roles {
            if row.get(&format!("{}Group", role.name)).map(String::as_str) == Some("true") {
                grants.insert(
                    Grant {
                        resource: resource.clone(),
                        action: action.clone(),
                        principal: role.group.clone(),
                        principal_type: "Group".to_string(),
                    },
                    true,
                );
            }
        }
    }
    grants
}
