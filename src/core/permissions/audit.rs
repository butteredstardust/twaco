//! `permissions audit`: each project's repository against its policy, without a server.
//!
//! An error is something `permissions apply` would change, or a permission the server refuses or
//! that grants nothing; a warning is probably a slip (a rule that matches nothing, a principal
//! no entity defines); a note is worth knowing (an explicit deny).

use super::model::ModelEntity;
use super::policy::{Mode, Policy, PolicyError};
use super::{differences, Change, Grants, KindKey, PermissionsError};
use crate::core::adopt::glob_matches;
use crate::core::config::Solution;
use crate::core::entity_carry::Kind;
use crate::core::progress::{self, Progress};
use crate::core::workspace;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub severity: Severity,
    /// A stable kebab-case name for the kind of finding.
    pub code: &'static str,
    /// `Collection/Name`, when the finding is about one entity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    pub message: String,
    /// One line per grant, for `--detail`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:<7} ", self.severity.label())?;
        if let Some(entity) = &self.entity {
            write!(f, "{entity}: ")?;
        }
        write!(f, "{} [{}]", self.message, self.code)
    }
}

/// One project's audit.
#[derive(Clone, Debug, Serialize)]
pub struct ProjectAudit {
    pub project: String,
    pub policy: String,
    /// `plain` or `helper`, after `auto` is resolved.
    pub mode: &'static str,
    /// The permission helper Thing, in helper mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper: Option<String>,
    pub entities: usize,
    pub findings: Vec<Finding>,
}

impl ProjectAudit {
    pub fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.severity == severity)
            .count()
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct AuditReport {
    /// Whether the server was compared too.
    pub server: bool,
    pub projects: Vec<ProjectAudit>,
    /// Projects without a `permissions.toml`.
    pub without_policy: Vec<String>,
}

impl AuditReport {
    pub fn count(&self, severity: Severity) -> usize {
        self.projects.iter().map(|p| p.count(severity)).sum()
    }
}

#[derive(Debug)]
pub enum AuditError {
    Project(String),
    NoPolicy(Vec<String>),
    Policy(PolicyError),
    Unreadable(Vec<String>),
    Entity(PermissionsError),
    Helper(String),
}

impl fmt::Display for AuditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuditError::Project(name) => write!(f, "this solution has no project named {name}"),
            AuditError::NoPolicy(projects) => write!(
                f,
                "no permissions.toml in the root folder of {}; documentation/CONFIGURATION.md describes the file",
                projects.join(", ")
            ),
            AuditError::Policy(error) => error.fmt(f),
            AuditError::Unreadable(items) => f.write_str(&items.join("\n")),
            AuditError::Entity(error) => error.fmt(f),
            AuditError::Helper(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for AuditError {}

impl crate::core::codes::Coded for AuditError {
    fn code(&self) -> crate::core::codes::ErrorCode {
        use crate::core::codes::ErrorCode;
        match self {
            AuditError::Project(_) | AuditError::NoPolicy(_) => ErrorCode::InvalidArguments,
            AuditError::Policy(_)
            | AuditError::Helper(_)
            | AuditError::Unreadable(_)
            | AuditError::Entity(_) => ErrorCode::InvalidData,
        }
    }
}

/// A project with its policy and its entities read.
pub struct Loaded {
    pub policy: Policy,
    pub entities: Vec<ModelEntity>,
    /// Every entity of the solution, by name, to tell whether a principal exists.
    pub all: std::rc::Rc<BTreeMap<String, ModelEntity>>,
    /// The solution's project names: a principal under one of them should exist.
    pub projects: std::rc::Rc<BTreeSet<String>>,
}

impl Loaded {
    pub fn helper(&self) -> Result<Option<&ModelEntity>, AuditError> {
        let helpers: Vec<&ModelEntity> = self.entities.iter().filter(|e| e.is_helper()).collect();
        match (self.policy.mode, helpers.as_slice()) {
            (Mode::Plain, _) | (Mode::Auto, []) => Ok(None),
            (Mode::Helper, []) => Err(AuditError::Helper(format!(
                "{}: mode is helper, but project {} has no Thing from {}",
                self.policy.path.display(),
                self.policy.project,
                super::model::HELPER_TEMPLATE
            ))),
            (_, [one]) => Ok(Some(one)),
            (_, many) => Err(AuditError::Helper(format!(
                "project {} has {} permission helper Things ({}); set mode = \"plain\" or keep one",
                self.policy.project,
                many.len(),
                many.iter().map(|e| e.name()).collect::<Vec<_>>().join(", ")
            ))),
        }
    }
}

/// Read the policy and entities of every project that has a policy, or of the one named.
pub fn load(
    solution: &Solution,
    project: Option<&str>,
) -> Result<(Vec<Loaded>, Vec<String>), AuditError> {
    if let Some(name) = project {
        if solution.project(name).is_none() {
            return Err(AuditError::Project(name.to_string()));
        }
    }
    let found = workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(AuditError::Unreadable(found.unreadable));
    }
    let mut all = BTreeMap::new();
    for file in &found.entities {
        let model = ModelEntity::read(file).map_err(AuditError::Entity)?;
        all.insert(file.info.name.clone(), model);
    }
    let mut loaded = Vec::new();
    let mut without = Vec::new();
    for configured in &solution.projects {
        if project.is_some_and(|name| name != configured.name) {
            continue;
        }
        let root = solution.project_root(configured);
        match Policy::load(&configured.name, &root).map_err(AuditError::Policy)? {
            Some(policy) => {
                let entities = found
                    .entities
                    .iter()
                    .filter(|file| file.found_under == configured.name)
                    .map(|file| all[&file.info.name].clone())
                    .collect();
                loaded.push(Loaded {
                    policy,
                    entities,
                    all: Default::default(),
                    projects: Default::default(),
                });
            }
            None => without.push(configured.name.clone()),
        }
    }
    if loaded.is_empty() {
        return Err(AuditError::NoPolicy(without));
    }
    let all = std::rc::Rc::new(all);
    let projects: std::rc::Rc<BTreeSet<String>> =
        std::rc::Rc::new(solution.projects.iter().map(|p| p.name.clone()).collect());
    for one in &mut loaded {
        one.all = all.clone();
        one.projects = projects.clone();
    }
    Ok((loaded, without))
}

/// The run-time set the policy manages on an entity: its own for a Thing, its instances' for a
/// ThingShape or ThingTemplate; none for anything else.
pub fn run_time_kind(entity: &ModelEntity) -> Option<Kind> {
    match entity.entity_type.as_str() {
        "Thing" => Some(Kind::RunTime),
        "ThingShape" | "ThingTemplate" => Some(Kind::InstanceRunTime),
        _ => None,
    }
}

/// Audit every project with a policy, or the one named.
pub fn audit(solution: &Solution, project: Option<&str>) -> Result<AuditReport, AuditError> {
    audit_with(solution, project, None)
}

/// As [`audit`], and with a server also what the server holds against the repository.
pub fn audit_with(
    solution: &Solution,
    project: Option<&str>,
    server: Option<&dyn super::server_audit::Remote>,
) -> Result<AuditReport, AuditError> {
    audit_with_progress(solution, project, server, &progress::NONE)
}

/// Like [`audit_with`], and report one step per entity audited. With a server, one more phase
/// per project reports one step per entity read from the server. Messages hold entity names only.
pub fn audit_with_progress(
    solution: &Solution,
    project: Option<&str>,
    server: Option<&dyn super::server_audit::Remote>,
    progress: &dyn Progress,
) -> Result<AuditReport, AuditError> {
    let (loaded, without_policy) = load(solution, project)?;
    let mut report = AuditReport {
        server: server.is_some(),
        projects: Vec::new(),
        without_policy,
    };
    {
        let total = loaded.iter().map(|one| one.entities.len() as u64).sum();
        let _phase = progress::phase(progress, "auditing entities", Some(total));
        for one in &loaded {
            report
                .projects
                .push(audit_loaded_with_progress(one, progress)?);
        }
    }
    if let Some(remote) = server {
        for (one, audited) in loaded.iter().zip(&mut report.projects) {
            let helper = one.helper()?;
            audited
                .findings
                .extend(super::server_audit::audit_with_progress(
                    remote, one, helper, progress,
                ));
            audited.findings.sort_by(|a, b| {
                (a.severity, &a.entity, a.code, &a.message)
                    .cmp(&(b.severity, &b.entity, b.code, &b.message))
            });
        }
    }
    Ok(report)
}

/// Audit one loaded project.
pub fn audit_loaded(loaded: &Loaded) -> Result<ProjectAudit, AuditError> {
    audit_loaded_with_progress(loaded, &progress::NONE)
}

/// Like [`audit_loaded`], and report one step per entity.
pub fn audit_loaded_with_progress(
    loaded: &Loaded,
    progress: &dyn Progress,
) -> Result<ProjectAudit, AuditError> {
    let policy = &loaded.policy;
    let helper = loaded.helper()?;
    let mut findings = Vec::new();
    for entity in &loaded.entities {
        progress.message(entity.name());
        if !policy.is_unmanaged(entity.name()) {
            managed_blocks(policy, entity, &mut findings);
        }
        hygiene(loaded, entity, &mut findings);
        if policy.is_strict(entity.name()) {
            for service in &entity.services {
                if !policy.classifies(entity.name(), service) {
                    findings.push(Finding {
                        severity: Severity::Error,
                        code: "unclassified-service",
                        entity: Some(entity.key()),
                        message: format!(
                            "service {service} matches no ServiceInvoke rule; the entity is strict, so name it in a [[runtime]] rule (roles = [] grants it to no one)"
                        ),
                        details: Vec::new(),
                    });
                }
            }
        }
        progress.advance(1);
    }
    unused_rules(loaded, &mut findings);
    role_units(loaded, &mut findings);
    if let Some(helper) = helper {
        findings.extend(super::helper::findings(loaded, helper).map_err(AuditError::Entity)?);
    }
    findings.sort_by(|a, b| {
        (a.severity, &a.entity, a.code, &a.message)
            .cmp(&(b.severity, &b.entity, b.code, &b.message))
    });
    Ok(ProjectAudit {
        project: policy.project.clone(),
        policy: policy.path.display().to_string(),
        mode: if helper.is_some() { "helper" } else { "plain" },
        helper: helper.map(|h| h.name().to_string()),
        entities: loaded.entities.len(),
        findings,
    })
}

/// The blocks the policy writes, against what the entity XML holds.
fn managed_blocks(policy: &Policy, entity: &ModelEntity, findings: &mut Vec<Finding>) {
    if let Some(kind) = run_time_kind(entity) {
        compare(policy, entity, kind, findings);
    }
    compare(policy, entity, Kind::Visibility, findings);
}

fn compare(policy: &Policy, entity: &ModelEntity, kind: Kind, findings: &mut Vec<Finding>) {
    let Some(current) = entity.sets.get(&KindKey::of(kind)) else {
        let empty = Grants::new();
        let wanted = wanted(policy, entity, kind, &empty);
        if !wanted.is_empty() {
            findings.push(Finding {
                severity: Severity::Error,
                code: "missing-block",
                entity: Some(entity.key()),
                message: format!(
                    "no {} block, and the policy grants {} here; `permissions apply` adds it",
                    kind.element(),
                    wanted.len()
                ),
                details: Vec::new(),
            });
        }
        return;
    };
    let wanted = wanted(policy, entity, kind, current);
    let found = differences(kind, &wanted, current);
    if found.is_empty() {
        return;
    }
    let missing = found
        .iter()
        .filter(|d| d.change == Change::RepositoryOnly)
        .count();
    let extra = found
        .iter()
        .filter(|d| d.change == Change::ServerOnly)
        .count();
    let flipped = found.iter().filter(|d| d.change == Change::Flipped).count();
    let mut parts = Vec::new();
    if missing > 0 {
        parts.push(format!("{missing} the policy grants are missing"));
    }
    if extra > 0 {
        parts.push(format!("{extra} the policy does not grant"));
    }
    if flipped > 0 {
        parts.push(format!("{flipped} denied where the policy allows"));
    }
    findings.push(Finding {
        severity: Severity::Error,
        code: "differs-from-policy",
        entity: Some(entity.key()),
        message: format!(
            "{} permissions: {}; `permissions apply` writes the policy's",
            kind.label(),
            parts.join(", ")
        ),
        details: found
            .iter()
            .map(|d| {
                let what = match d.change {
                    Change::RepositoryOnly => "policy grants",
                    Change::ServerOnly => "not in policy",
                    Change::Flipped => "denied, policy allows",
                };
                format!("{what:<22} {}", d.grant)
            })
            .collect(),
    });
}

pub fn wanted(policy: &Policy, entity: &ModelEntity, kind: Kind, current: &Grants) -> Grants {
    if kind == Kind::Visibility {
        policy.wanted_visibility(entity, current)
    } else {
        policy.wanted_run_time(entity, current)
    }
}

/// What is wrong in any block, managed or not: what the server refuses, what grants nothing.
fn hygiene(loaded: &Loaded, entity: &ModelEntity, findings: &mut Vec<Finding>) {
    let policy = &loaded.policy;
    let mut unknown: BTreeSet<String> = BTreeSet::new();
    for (key, grants) in &entity.sets {
        let kind = key.kind();
        let visibility = matches!(kind, Kind::Visibility | Kind::InstanceVisibility);
        for (grant, allowed) in grants {
            let principal = &grant.principal;
            if visibility
                && !matches!(
                    grant.principal_type.as_str(),
                    "Organization" | "OrganizationalUnit"
                )
            {
                findings.push(Finding {
                    severity: Severity::Error,
                    code: "visibility-not-an-organization",
                    entity: Some(entity.key()),
                    message: format!(
                        "{} names {} {principal}; visibility takes only an Organization or an OrganizationalUnit (a group is refused with HTTP 500)",
                        kind.label(),
                        if grant.principal_type.is_empty() {
                            "a principal without a type,"
                        } else {
                            grant.principal_type.as_str()
                        }
                    ),
                    details: Vec::new(),
                });
            }
            if kind.is_run_time() && grant.principal_type == "Organization" {
                findings.push(Finding {
                    severity: Severity::Error,
                    code: "organization-in-run-time",
                    entity: Some(entity.key()),
                    message: format!(
                        "{} names Organization {principal}, which run-time permissions do not accept",
                        kind.label()
                    ),
                    details: Vec::new(),
                });
            }
            if visibility && policy.removes(principal) {
                findings.push(Finding {
                    severity: if policy.is_unmanaged(entity.name()) {
                        Severity::Warning
                    } else {
                        Severity::Error
                    },
                    code: "removed-principal",
                    entity: Some(entity.key()),
                    message: format!(
                        "{} names {principal}, which [visibility] remove names",
                        kind.label()
                    ),
                    details: Vec::new(),
                });
            }
            if !allowed {
                findings.push(Finding {
                    severity: Severity::Note,
                    code: "deny",
                    entity: Some(entity.key()),
                    message: format!("{} denies {grant}", kind.label()),
                    details: Vec::new(),
                });
            }
            if !exists(loaded, &grant.principal, &grant.principal_type) {
                unknown.insert(format!("{} {principal}", grant.principal_type));
            }
        }
    }
    for principal in unknown {
        findings.push(Finding {
            severity: Severity::Warning,
            code: "unknown-principal",
            entity: Some(entity.key()),
            message: format!(
                "names {principal}, which no entity of a project in this solution defines"
            ),
            details: Vec::new(),
        });
    }
}

/// Whether a principal this solution should define is defined. A principal outside every
/// project of the solution (a platform group, another block's) is not checked.
fn exists(loaded: &Loaded, name: &str, principal_type: &str) -> bool {
    let (entity_name, unit) = match principal_type {
        "OrganizationalUnit" => match name.split_once(':') {
            Some((org, unit)) => (org, Some(unit)),
            None => (name, None),
        },
        _ => (name, None),
    };
    let ours = loaded
        .projects
        .iter()
        .any(|project| entity_name.starts_with(&format!("{project}.")));
    if !ours {
        return true;
    }
    let Some(entity) = loaded.all.get(entity_name) else {
        return false;
    };
    let wanted = match principal_type {
        "Group" => "Group",
        "User" => "User",
        "Organization" | "OrganizationalUnit" => "Organization",
        _ => return true,
    };
    if entity.entity_type != wanted {
        return false;
    }
    match unit {
        Some(unit) => entity.units.contains_key(unit),
        None => true,
    }
}

/// Rules and patterns that match nothing: usually a typo or a renamed entity.
fn unused_rules(loaded: &Loaded, findings: &mut Vec<Finding>) {
    let policy = &loaded.policy;
    for (at, rule) in policy.runtime.iter().enumerate() {
        let matched: Vec<&ModelEntity> = loaded
            .entities
            .iter()
            .filter(|entity| policy.rule_names(rule, entity.name()))
            .collect();
        if matched.is_empty() {
            findings.push(Finding {
                severity: Severity::Warning,
                code: "rule-matches-nothing",
                entity: None,
                message: format!(
                    "[[runtime]] {} ({}) names no entity of the project",
                    at + 1,
                    rule.entities.join(", ")
                ),
                details: Vec::new(),
            });
            continue;
        }
        for pattern in &rule.resources {
            if !pattern.contains('*') {
                continue;
            }
            let any = matched.iter().any(|entity| {
                let current = run_time_kind(entity)
                    .and_then(|kind| entity.sets.get(&KindKey::of(kind)))
                    .cloned()
                    .unwrap_or_default();
                entity
                    .defined(&rule.action)
                    .iter()
                    .chain(
                        current
                            .keys()
                            .filter(|g| g.action == rule.action)
                            .map(|g| &g.resource),
                    )
                    .any(|resource| resource != "*" && glob_matches(pattern, resource))
            });
            if !any {
                findings.push(Finding {
                    severity: Severity::Warning,
                    code: "pattern-matches-nothing",
                    entity: None,
                    message: format!(
                        "[[runtime]] {}: {} matches no {} resource of {}",
                        at + 1,
                        pattern,
                        rule.action,
                        rule.entities.join(", ")
                    ),
                    details: Vec::new(),
                });
            }
        }
    }
    for (at, rule) in policy.visibility_rules.iter().enumerate() {
        let any = loaded.entities.iter().any(|entity| {
            (rule.types.is_empty() || rule.types.contains(&entity.entity_type))
                && (rule.names.is_empty()
                    || rule
                        .names
                        .iter()
                        .any(|p| super::policy::names_entity(&policy.project, p, entity.name())))
        });
        if !any {
            findings.push(Finding {
                severity: Severity::Warning,
                code: "rule-matches-nothing",
                entity: None,
                message: format!(
                    "[[visibility.rule]] {} names no entity of the project",
                    at + 1
                ),
                details: Vec::new(),
            });
        }
    }
}

/// Each role's organizational unit, where the repository holds its Organization: the unit must
/// exist and have the role's group as a member, or the role sees nothing.
fn role_units(loaded: &Loaded, findings: &mut Vec<Finding>) {
    for role in &loaded.policy.roles {
        let Some(org) = &role.org else { continue };
        if org.principal_type != "OrganizationalUnit" {
            continue;
        }
        let Some((organization, unit)) = org.name.split_once(':') else {
            continue;
        };
        let Some(entity) = loaded.all.get(organization) else {
            continue;
        };
        match entity.units.get(unit) {
            None => findings.push(Finding {
                severity: Severity::Error,
                code: "role-unit-missing",
                entity: Some(entity.key()),
                message: format!(
                    "role {} is seen through unit {unit}, which the Organization does not declare",
                    role.name
                ),
                details: Vec::new(),
            }),
            Some(members) if !members.contains(&role.group) => findings.push(Finding {
                severity: Severity::Warning,
                code: "role-unit-without-group",
                entity: Some(entity.key()),
                message: format!(
                    "unit {unit} of role {} does not have {} as a member, so the role's members see nothing through it",
                    role.name, role.group
                ),
                details: Vec::new(),
            }),
            Some(_) => {}
        }
    }
}
