//! `permissions apply`: write each project's policy into its entity XML.
//!
//! Only the blocks the policy owns change, and only where their grants differ. An entity whose
//! strict services are not all classified stops the plan, since writing would drop their grants
//! without anyone deciding so.

use super::audit::{self, AuditError, Finding, Severity};
use super::model::ModelEntity;
use super::policy::Policy;
use super::write::{self, Order};
use super::{differences, Change, Grants, KindKey};
use crate::core::config::Solution;
use crate::core::entity_carry::Kind;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

/// One entity file the plan changes.
#[derive(Clone, Debug, Serialize)]
pub struct FileChange {
    /// `Collection/Name`.
    pub entity: String,
    #[serde(serialize_with = "labels")]
    pub sets: Vec<Kind>,
    /// In helper mode, whether the helper's tables or a DataShape's columns change too.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub helper: bool,
    /// Grants the policy adds, and grants it takes away (a flipped deny counts in both).
    pub added: usize,
    pub removed: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub before: Vec<u8>,
    #[serde(skip)]
    pub after: Vec<u8>,
}

fn labels<S: serde::Serializer>(kinds: &[Kind], serializer: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(kinds.len()))?;
    for kind in kinds {
        seq.serialize_element(kind.label())?;
    }
    seq.end()
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectPlan {
    pub project: String,
    pub mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper: Option<String>,
    pub changes: Vec<FileChange>,
    /// Audit errors and warnings that writing the policy does not settle, such as a group in a
    /// visibility block of an entity the policy leaves alone.
    pub remaining: Vec<Finding>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ApplyPlan {
    pub projects: Vec<ProjectPlan>,
    pub without_policy: Vec<String>,
}

impl ApplyPlan {
    /// Errors the audit still finds once the plan is written.
    pub fn remaining_errors(&self) -> usize {
        self.projects
            .iter()
            .flat_map(|p| p.remaining.iter())
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    pub fn changes(&self) -> impl Iterator<Item = &FileChange> {
        self.projects.iter().flat_map(|p| p.changes.iter())
    }
}

#[derive(Debug)]
pub enum ApplyError {
    Audit(AuditError),
    /// Strict entities' services no rule names.
    Unclassified(Vec<String>),
    Write {
        entity: String,
        why: String,
    },
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyError::Audit(error) => error.fmt(f),
            ApplyError::Unclassified(items) => write!(
                f,
                "classify these services in a [[runtime]] rule first (roles = [] grants one to no one):\n  {}",
                items.join("\n  ")
            ),
            ApplyError::Write { entity, why } => write!(f, "{entity}: {why}"),
        }
    }
}

impl std::error::Error for ApplyError {}

impl crate::core::codes::Coded for ApplyError {
    fn code(&self) -> crate::core::codes::ErrorCode {
        use crate::core::codes::ErrorCode;
        match self {
            ApplyError::Audit(error) => error.code(),
            ApplyError::Unclassified(_) => ErrorCode::GuardRefused,
            ApplyError::Write { .. } => ErrorCode::InvalidData,
        }
    }
}

/// What writing every policy (or the named project's) would change.
pub fn plan(solution: &Solution, project: Option<&str>) -> Result<ApplyPlan, ApplyError> {
    let (loaded, without_policy) = audit::load(solution, project).map_err(ApplyError::Audit)?;
    let audited = loaded
        .iter()
        .map(audit::audit_loaded)
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApplyError::Audit)?;
    let unclassified: Vec<String> = audited
        .iter()
        .flat_map(|p| p.findings.iter())
        .filter(|f| f.code == "unclassified-service")
        .map(|f| format!("{}: {}", f.entity.as_deref().unwrap_or_default(), f.message))
        .collect();
    if !unclassified.is_empty() {
        return Err(ApplyError::Unclassified(unclassified));
    }
    let mut plan = ApplyPlan {
        projects: Vec::new(),
        without_policy,
    };
    for one in &loaded {
        let helper = one.helper().map_err(ApplyError::Audit)?;
        let mut changes = Vec::new();
        for entity in &one.entities {
            if one.policy.is_unmanaged(entity.name()) {
                continue;
            }
            if let Some(change) = change_of(&one.policy, entity)? {
                changes.push(change);
            }
        }
        if let Some(helper) = helper {
            let bytes_of = |entity: &ModelEntity| -> Result<Vec<u8>, super::PermissionsError> {
                Ok(match changes.iter().find(|c| c.path == entity.file.path) {
                    Some(change) => change.after.clone(),
                    None => entity.bytes.to_vec(),
                })
            };
            let edits =
                super::helper::edits(one, helper, &bytes_of).map_err(|e| ApplyError::Write {
                    entity: helper.key(),
                    why: e.to_string(),
                })?;
            for edit in edits {
                match changes.iter_mut().find(|c| c.path == edit.entity.file.path) {
                    Some(change) => {
                        change.after = edit.after;
                        change.helper = true;
                        change.details.extend(edit.details);
                    }
                    None => {
                        let before = edit.entity.bytes.to_vec();
                        changes.push(FileChange {
                            entity: edit.entity.key(),
                            sets: Vec::new(),
                            helper: true,
                            added: 0,
                            removed: 0,
                            details: edit.details,
                            path: edit.entity.file.path.clone(),
                            before,
                            after: edit.after,
                        });
                    }
                }
            }
        }
        // What is left once the files are written: the audit of the planned bytes.
        let remaining = remaining_after(one, &changes)?;
        plan.projects.push(ProjectPlan {
            project: one.policy.project.clone(),
            mode: if helper.is_some() { "helper" } else { "plain" },
            helper: helper.map(|h| h.name().to_string()),
            changes,
            remaining,
        });
    }
    Ok(plan)
}

/// How a policy ranks a principal it writes: its role's position, roles first.
fn principal_rank(policy: &Policy, principal: &str) -> usize {
    policy
        .roles
        .iter()
        .position(|role| {
            role.group == principal || role.org.as_ref().is_some_and(|org| org.name == principal)
        })
        .unwrap_or(usize::MAX)
}

pub(super) fn change_of(
    policy: &Policy,
    entity: &ModelEntity,
) -> Result<Option<FileChange>, ApplyError> {
    let mut kinds = Vec::new();
    if let Some(kind) = audit::run_time_kind(entity) {
        kinds.push(kind);
    }
    kinds.push(Kind::Visibility);
    let mut wanted: BTreeMap<KindKey, Grants> = BTreeMap::new();
    let mut details = Vec::new();
    let (mut added, mut removed) = (0, 0);
    let empty = Grants::new();
    for kind in kinds {
        let current = entity.sets.get(&KindKey::of(kind));
        let goal = audit::wanted(policy, entity, kind, current.unwrap_or(&empty));
        if current.is_none() && goal.is_empty() {
            continue;
        }
        if current == Some(&goal) {
            continue;
        }
        for difference in differences(kind, &goal, current.unwrap_or(&empty)) {
            match difference.change {
                Change::RepositoryOnly => added += 1,
                Change::ServerOnly => removed += 1,
                Change::Flipped => {
                    added += 1;
                    removed += 1;
                }
            }
            let what = match difference.change {
                Change::RepositoryOnly => "add",
                Change::ServerOnly => "remove",
                Change::Flipped => "allow",
            };
            details.push(format!(
                "{what:<6} {:<20} {}",
                kind.label(),
                difference.grant
            ));
        }
        wanted.insert(KindKey::of(kind), goal);
    }
    if wanted.is_empty() {
        return Ok(None);
    }
    let before = entity.bytes.to_vec();
    let principal = |name: &str| principal_rank(policy, name);
    let resource = |name: &str| usize::from(name != "*");
    let order = Order {
        principal: &principal,
        resource: &resource,
    };
    let after = write::rewrite(&before, &wanted, &order).map_err(|e| ApplyError::Write {
        entity: entity.key(),
        why: e.to_string(),
    })?;
    if after == before {
        return Ok(None);
    }
    Ok(Some(FileChange {
        entity: entity.key(),
        sets: wanted.keys().map(|key| key.kind()).collect(),
        helper: false,
        added,
        removed,
        details,
        path: entity.file.path.clone(),
        before,
        after,
    }))
}

/// The audit's errors and warnings over the project as the plan leaves it.
fn remaining_after(
    loaded: &audit::Loaded,
    changes: &[FileChange],
) -> Result<Vec<Finding>, ApplyError> {
    let mut all = (*loaded.all).clone();
    let mut entities = loaded.entities.clone();
    for change in changes {
        let Some(at) = entities.iter().position(|e| e.file.path == change.path) else {
            continue;
        };
        let model = ModelEntity::of(&entities[at].file, change.after.clone()).map_err(|e| {
            ApplyError::Write {
                entity: change.entity.clone(),
                why: e.to_string(),
            }
        })?;
        all.insert(model.name().to_string(), model.clone());
        entities[at] = model;
    }
    let after = audit::Loaded {
        policy: loaded.policy.clone(),
        entities,
        all: std::rc::Rc::new(all),
        projects: loaded.projects.clone(),
    };
    let audited = audit::audit_loaded(&after).map_err(ApplyError::Audit)?;
    Ok(audited
        .findings
        .into_iter()
        .filter(|f| f.severity != Severity::Note)
        .collect())
}
