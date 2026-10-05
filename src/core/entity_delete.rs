//! Planning and applying guarded entity deletion.
//!
//! A delete is planned from read-only server calls first. Structural dependents, repository
//! definitions and file-repository Things are made visible before an apply can remove anything.
//! Script strings and Mashup JSON are outside ThingWorx's incoming-dependency graph and the
//! result says so rather than presenting the guard as complete.

use super::backup;
use super::bundle::COLLECTION_ORDER;
use super::config::Solution;
use super::entity_key::{EntityKey, ServiceTarget};
use super::ledger::{Ledger, LedgerError};
use super::scan::{self, Kind};
use super::server::{Client, ServerError};
use super::workspace;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

pub const DEPENDENCY_LIMIT: &str =
    "GetIncomingDependencies sees structural dependents only, never names inside scripts or mashup JSON";
pub const FORCE_DEPRECATION: &str =
    "--force is deprecated for entity delete; it now means --allow-repository-defined --allow-outside-dependents and never covers FileRepository data loss";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    Service { service: String },
    RestDelete,
    Composer,
}

impl Serialize for Method {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Method::Service { service } => write!(f, "EntityServices.{service}"),
            Method::RestDelete => f.write_str("REST DELETE"),
            Method::Composer => f.write_str("delete it in Composer"),
        }
    }
}

/// The delete route is an explicit allow-list. Verified ThingWorx EntityServices metadata
/// reports one STRING input named `name` for every listed service: DeleteThing,
/// DeleteThingTemplate, DeleteThingShape, DeleteMediaEntity, DeleteGroup, DeleteOrganization,
/// DeleteProject and DeleteUser. Collections listed as REST use Composer's verified route.
pub fn method_for(collection: &str) -> Option<Method> {
    let service = match collection {
        "Things" => Some("DeleteThing"),
        "ThingTemplates" => Some("DeleteThingTemplate"),
        "ThingShapes" => Some("DeleteThingShape"),
        "MediaEntities" => Some("DeleteMediaEntity"),
        "Groups" => Some("DeleteGroup"),
        "Organizations" => Some("DeleteOrganization"),
        "Projects" => Some("DeleteProject"),
        "Users" => Some("DeleteUser"),
        _ => None,
    };
    if let Some(service) = service {
        return Some(Method::Service {
            service: service.to_string(),
        });
    }
    match collection {
        "ApplicationKeys"
        | "Dashboards"
        | "DataShapes"
        | "DataTables"
        | "Localizations"
        | "LocalizationTables"
        | "MashupGadgets"
        | "Mashups"
        | "Menus"
        | "ModelTags"
        | "Networks"
        | "NotificationContents"
        | "NotificationDefinitions"
        | "PersistenceProviders"
        | "Schedulers"
        | "StateDefinitions"
        | "Streams"
        | "StyleDefinitions"
        | "StyleThemes"
        | "ThingGroups"
        | "Timers"
        | "ValueStreams"
        | "Widgets"
        | "MCPNamespaces"
        | "AIAgents" => Some(Method::RestDelete),
        _ => None,
    }
}

pub trait Remote {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError>;
    fn incoming(&self, key: &EntityKey) -> Result<Vec<Dependent>, ServerError>;
    fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError>;
    fn delete_service(&self, service: &str, name: &str) -> Result<(), ServerError>;
    fn delete_rest(&self, key: &EntityKey) -> Result<(), ServerError>;
    /// Save the server's export of each entity before it is deleted; the set's folder, relative to
    /// the solution, or `None` when none of them exists.
    fn backup(
        &self,
        solution: &Solution,
        entities: &[EntityKey],
        stamp: &str,
    ) -> Result<Option<String>, backup::BackupError>;
}

impl Remote for Client {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        self.entity_exists(key.collection(), key.name())
    }

    fn incoming(&self, key: &EntityKey) -> Result<Vec<Dependent>, ServerError> {
        let target = ServiceTarget::Entity(key.clone());
        let value = self
            .call_service(
                &target,
                "GetIncomingDependencies",
                &serde_json::json!({}),
                Duration::from_secs(120),
            )?
            .unwrap_or_else(|| serde_json::json!({ "rows": [] }));
        let rows = value.get("rows").and_then(Value::as_array).ok_or_else(|| {
            ServerError::InvalidResponse {
                url: format!("{target}/Services/GetIncomingDependencies"),
                why: "expected an InfoTable with rows".to_string(),
            }
        })?;
        rows.iter()
            .map(|row| {
                let name = row.get("name").and_then(Value::as_str).ok_or_else(|| {
                    ServerError::InvalidResponse {
                        url: format!("{target}/Services/GetIncomingDependencies"),
                        why: "a row has no string name".to_string(),
                    }
                })?;
                let entity_type = row.get("type").and_then(Value::as_str).ok_or_else(|| {
                    ServerError::InvalidResponse {
                        url: format!("{target}/Services/GetIncomingDependencies"),
                        why: "a row has no string type".to_string(),
                    }
                })?;
                Ok(Dependent {
                    collection: dependency_collection(entity_type),
                    name: name.to_string(),
                })
            })
            .collect()
    }

    fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
        self.fetch_entity(key.collection(), key.name())
    }

    fn delete_service(&self, service: &str, name: &str) -> Result<(), ServerError> {
        self.call_service(
            &ServiceTarget::platform("Resources", "EntityServices"),
            service,
            &serde_json::json!({ "name": name }),
            Duration::from_secs(120),
        )?;
        Ok(())
    }

    fn delete_rest(&self, key: &EntityKey) -> Result<(), ServerError> {
        self.delete_entity_rest(key.collection(), key.name())
    }

    fn backup(
        &self,
        solution: &Solution,
        entities: &[EntityKey],
        stamp: &str,
    ) -> Result<Option<String>, backup::BackupError> {
        let entities = entities
            .iter()
            .map(|key| (key.collection().to_string(), key.name().to_string()))
            .collect::<Vec<_>>();
        let set = backup::save(self, solution, "entity delete", &entities, stamp)?;
        Ok(set.map(|set| backup::relative(solution, &set.dir)))
    }
}

fn dependency_collection(entity_type: &str) -> String {
    match entity_type {
        "DataShape" => "DataShapes",
        "ThingShape" => "ThingShapes",
        "ThingTemplate" => "ThingTemplates",
        "MediaEntity" => "MediaEntities",
        "StateDefinition" => "StateDefinitions",
        "StyleDefinition" => "StyleDefinitions",
        "StyleTheme" => "StyleThemes",
        other if other.ends_with('s') => other,
        other => return format!("{other}s"),
    }
    .to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Dependent {
    pub collection: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    Absent,
    Refused,
    Deleted,
    Failed,
}

/// Conditions a caller explicitly accepts before deleting an entity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Acknowledged {
    pub repository_defined: bool,
    pub outside_dependents: bool,
    pub file_repository_data_loss: bool,
}

/// A stable reason why an entity delete was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardCode {
    RepositoryDefined,
    OutsideDependents,
    FileRepositoryDataLoss,
    NoDeleteMethod,
}

impl GuardCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RepositoryDefined => "repository_defined",
            Self::OutsideDependents => "outside_dependents",
            Self::FileRepositoryDataLoss => "file_repository_data_loss",
            Self::NoDeleteMethod => "no_delete_method",
        }
    }
}

#[derive(Clone, Debug)]
struct Refusal {
    code: GuardCode,
    message: String,
}

/// Map the legacy alias and explicit acknowledgements into the core delete options.
/// `force` never acknowledges FileRepository data loss.
pub fn acknowledged(
    force: bool,
    repository_defined: bool,
    outside_dependents: bool,
    file_repository_data_loss: bool,
) -> (Acknowledged, bool) {
    (
        Acknowledged {
            repository_defined: force || repository_defined,
            outside_dependents: force || outside_dependents,
            file_repository_data_loss,
        },
        force,
    )
}

#[derive(Clone, Debug)]
pub struct EntityResult {
    pub collection: String,
    pub name: String,
    pub status: Status,
    pub method: Method,
    pub dependents: Vec<Dependent>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    refusals: Vec<Refusal>,
    ledger: Vec<LedgerLocation>,
}

impl EntityResult {
    fn key(&self) -> EntityKey {
        EntityKey::new(&self.collection, &self.name)
            .expect("entity results originate from validated delete targets")
    }

    pub fn refusals(&self) -> impl Iterator<Item = &str> {
        self.refusals.iter().map(|refusal| refusal.message.as_str())
    }

    pub fn refusal_codes(&self) -> impl Iterator<Item = GuardCode> + '_ {
        self.refusals.iter().map(|refusal| refusal.code)
    }

    pub fn refusal_pairs(&self) -> impl Iterator<Item = (GuardCode, &str)> {
        self.refusals
            .iter()
            .map(|refusal| (refusal.code, refusal.message.as_str()))
    }
}

impl Serialize for EntityResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            collection: &'a str,
            name: &'a str,
            status: &'a Status,
            method: &'a Method,
            dependents: &'a [Dependent],
            warnings: &'a [String],
            #[serde(skip_serializing_if = "Vec::is_empty")]
            refusals: Vec<&'a str>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            refusal_codes: Vec<GuardCode>,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: &'a Option<String>,
        }
        Wire {
            collection: &self.collection,
            name: &self.name,
            status: &self.status,
            method: &self.method,
            dependents: &self.dependents,
            warnings: &self.warnings,
            refusals: self.refusals().collect(),
            refusal_codes: self.refusal_codes().collect(),
            error: &self.error,
        }
        .serialize(serializer)
    }
}

#[derive(Clone, Debug)]
struct Target {
    key: EntityKey,
    method: Method,
    ledger: Vec<LedgerLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LedgerLocation {
    record: usize,
    entity: usize,
}

#[derive(Debug)]
pub enum DeleteError {
    Ledger {
        path: PathBuf,
        why: String,
    },
    Target(String),
    Remote {
        entity: String,
        why: ServerError,
    },
    Write {
        path: PathBuf,
        why: String,
    },
    /// A backup could not be taken, so nothing was deleted.
    Backup(String),
}

impl fmt::Display for DeleteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeleteError::Ledger { path, why } => {
                write!(f, "{}: invalid rename ledger: {why}", path.display())
            }
            DeleteError::Target(why) => f.write_str(why),
            DeleteError::Remote { entity, why } => write!(f, "{entity}: {why}"),
            DeleteError::Write { path, why } => write!(f, "cannot write {}: {why}", path.display()),
            DeleteError::Backup(why) => write!(
                f,
                "nothing was deleted, because the backup could not be taken: {why} (--no-backup deletes without one)"
            ),
        }
    }
}

impl std::error::Error for DeleteError {}

impl From<LedgerError> for DeleteError {
    fn from(error: LedgerError) -> Self {
        match error {
            LedgerError::Invalid { path, why } => DeleteError::Ledger { path, why },
            LedgerError::Write { path, why } => DeleteError::Write { path, why },
        }
    }
}

pub struct Prepared {
    ledger_path: PathBuf,
    ledger: Ledger,
    requested: Vec<String>,
    renamed: bool,
    /// The stamp of the backup set to save before deleting; `None` deletes without one.
    backup_stamp: Option<String>,
}

impl Prepared {
    /// Save the server's copy of everything this will delete under `.twaco/backups/<stamp>`
    /// first. A backup that cannot be taken stops the delete.
    pub fn with_backup(mut self, stamp: &str) -> Self {
        self.backup_stamp = Some(stamp.to_string());
        self
    }

    pub fn ledger_will_be_written(&self, apply: bool) -> bool {
        apply
            && !self
                .ledger
                .replaced(|entity| {
                    entity.deleted.is_none()
                        && (self.renamed
                            || self.requested.iter().any(|requested| {
                                requested == &entity.old
                                    || requested == &format!("{}/{}", entity.collection, entity.old)
                            }))
                })
                .is_empty()
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub applied: bool,
    pub entities: Vec<EntityResult>,
    pub dependency_limit: &'static str,
    /// The folder the entities were saved to before they were deleted, relative to the solution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    #[serde(skip)]
    pub ledger_changed: bool,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.entities
            .iter()
            .any(|entity| matches!(entity.status, Status::Refused | Status::Failed))
    }
}

pub fn prepare(
    solution: &Solution,
    requested: &[String],
    renamed: bool,
) -> Result<Prepared, DeleteError> {
    if requested.is_empty() && !renamed {
        return Err(DeleteError::Target(
            "entity delete needs at least one entity, or --renamed".to_string(),
        ));
    }
    let ledger_path = solution.root.join(super::ledger::RELATIVE_PATH);
    let ledger = Ledger::read(&ledger_path)?;
    Ok(Prepared {
        ledger_path,
        ledger,
        requested: requested.to_vec(),
        renamed,
        backup_stamp: None,
    })
}

pub fn run(
    remote: &dyn Remote,
    solution: &Solution,
    mut prepared: Prepared,
    apply: bool,
    acknowledged: Acknowledged,
    date: &str,
) -> Result<Report, DeleteError> {
    let mut targets = resolve_targets(remote, &prepared)?;
    order_targets(&mut targets);
    let target_set: BTreeSet<EntityKey> = targets.iter().map(|target| target.key.clone()).collect();
    let repository: BTreeSet<EntityKey> = workspace::discover(solution)
        .entities
        .into_iter()
        .filter_map(|entity| EntityKey::new(entity.info.collection, entity.info.name).ok())
        .collect();
    let mut entities = Vec::new();
    for target in targets {
        let Target {
            key,
            method,
            ledger,
        } = target;
        if method == Method::Composer {
            entities.push(EntityResult {
                collection: key.collection().to_string(),
                name: key.name().to_string(),
                status: Status::Refused,
                method,
                dependents: Vec::new(),
                warnings: Vec::new(),
                error: None,
                refusals: vec![Refusal {
                    code: GuardCode::NoDeleteMethod,
                    message:
                        "twaco has no delete method for this collection; delete it in Composer"
                            .to_string(),
                }],
                ledger,
            });
            continue;
        }
        let label = key.to_string();
        let exists = remote.exists(&key).map_err(|why| DeleteError::Remote {
            entity: label.clone(),
            why,
        })?;
        if !exists {
            entities.push(EntityResult {
                collection: key.collection().to_string(),
                name: key.name().to_string(),
                status: Status::Absent,
                method,
                dependents: Vec::new(),
                warnings: Vec::new(),
                error: None,
                refusals: Vec::new(),
                ledger,
            });
            continue;
        }
        let dependents = remote.incoming(&key).map_err(|why| DeleteError::Remote {
            entity: label.clone(),
            why,
        })?;
        let outside: Vec<&Dependent> = dependents
            .iter()
            .filter(|dependent| {
                EntityKey::new(&dependent.collection, &dependent.name)
                    .map_or(true, |key| !target_set.contains(&key))
            })
            .collect();
        let mut refusals = Vec::new();
        if !acknowledged.repository_defined && repository.contains(&key) {
            refusals.push(Refusal {
                code: GuardCode::RepositoryDefined,
                message: "the repository still defines this entity; deploying would create it again (pass --allow-repository-defined)".to_string(),
            });
        }
        if !acknowledged.outside_dependents && !outside.is_empty() {
            refusals.push(Refusal {
                code: GuardCode::OutsideDependents,
                message: format!(
                "incoming dependents outside this delete set: {} (pass --allow-outside-dependents)",
                outside
                    .iter()
                    .map(|d| format!("{}/{}", d.collection, d.name))
                    .collect::<Vec<_>>()
                    .join(", ")
                ),
            });
        }
        let mut warnings = Vec::new();
        if key.collection() == "Things" {
            let bytes = remote
                .fetch(&key)
                .map_err(|why| DeleteError::Remote { entity: label, why })?;
            if is_file_repository(&bytes) {
                let message = "deleting this FileRepository Thing deletes all of its files";
                if acknowledged.file_repository_data_loss {
                    warnings.push(message.to_string());
                } else {
                    refusals.push(Refusal {
                        code: GuardCode::FileRepositoryDataLoss,
                        message: format!("{message} (pass --allow-file-repository-data-loss)"),
                    });
                }
            }
        }
        entities.push(EntityResult {
            collection: key.collection().to_string(),
            name: key.name().to_string(),
            status: if refusals.is_empty() {
                Status::Ready
            } else {
                Status::Refused
            },
            method,
            dependents,
            warnings,
            error: None,
            refusals,
            ledger,
        });
    }

    delete_dependents_first(&mut entities);
    let mut backup_dir = None;
    if apply {
        if let Some(stamp) = &prepared.backup_stamp {
            let ready: Vec<EntityKey> = entities
                .iter()
                .filter(|entity| entity.status == Status::Ready)
                .map(EntityResult::key)
                .collect();
            if !ready.is_empty() {
                backup_dir = remote
                    .backup(solution, &ready, stamp)
                    .map_err(|error| DeleteError::Backup(error.to_string()))?;
            }
        }
        for entity in &mut entities {
            if entity.status != Status::Ready {
                continue;
            }
            let deleted = match &entity.method {
                Method::Service { service } => remote.delete_service(service, &entity.name),
                Method::RestDelete => remote.delete_rest(&entity.key()),
                Method::Composer => unreachable!("unsupported methods are refused during planning"),
            };
            if let Err(error) = deleted {
                entity.status = Status::Failed;
                entity.error = Some(error.to_string());
                continue;
            }
            match remote.exists(&entity.key()) {
                Ok(false) => entity.status = Status::Deleted,
                Ok(true) => {
                    entity.status = Status::Failed;
                    entity.error = Some(
                        "delete answered success, but the confirming GET still found the entity"
                            .to_string(),
                    );
                }
                Err(error) => {
                    entity.status = Status::Failed;
                    entity.error = Some(format!(
                        "delete was sent, but the confirming GET failed: {error}"
                    ));
                }
            }
        }
    }

    let mut ledger_changed = false;
    if apply {
        for entity in &entities {
            if matches!(entity.status, Status::Deleted | Status::Absent) {
                for location in &entity.ledger {
                    let item = prepared
                        .ledger
                        .entity_mut((location.record, location.entity));
                    if item.deleted.is_none() {
                        item.deleted = Some(date.to_string());
                        ledger_changed = true;
                    }
                }
            }
        }
        if ledger_changed {
            prepared.ledger.write(&prepared.ledger_path)?;
        }
    }
    Ok(Report {
        applied: apply,
        entities,
        dependency_limit: DEPENDENCY_LIMIT,
        backup: backup_dir,
        ledger_changed,
    })
}

fn resolve_targets(remote: &dyn Remote, prepared: &Prepared) -> Result<Vec<Target>, DeleteError> {
    let mut targets: BTreeMap<EntityKey, Target> = BTreeMap::new();
    for requested in &prepared.requested {
        let key = match requested.contains('/') {
            true => EntityKey::parse(requested).map_err(|_| {
                DeleteError::Target(format!(
                    "{requested:?} must be Collection/Name or a bare server entity name"
                ))
            })?,
            false => resolve_bare(remote, requested)?,
        };
        let method = method_for(key.collection()).unwrap_or(Method::Composer);
        targets.entry(key.clone()).or_insert(Target {
            key,
            method,
            ledger: Vec::new(),
        });
    }
    for (record_at, entity_at) in prepared.ledger.replaced(|entity| entity.deleted.is_none()) {
        let pending = prepared.ledger.entity((record_at, entity_at));
        let key =
            EntityKey::new(&pending.collection, &pending.old).map_err(|_| DeleteError::Ledger {
                path: prepared.ledger_path.clone(),
                why: format!(
                    "{}/{} is not a valid entity key",
                    pending.collection, pending.old
                ),
            })?;
        if !prepared.renamed && !targets.contains_key(&key) {
            continue;
        }
        let method = method_for(key.collection()).unwrap_or(Method::Composer);
        targets
            .entry(key.clone())
            .or_insert(Target {
                key,
                method,
                ledger: Vec::new(),
            })
            .ledger
            .push(LedgerLocation {
                record: record_at,
                entity: entity_at,
            });
    }
    Ok(targets.into_values().collect())
}

fn resolve_bare(remote: &dyn Remote, name: &str) -> Result<EntityKey, DeleteError> {
    if EntityKey::new("Things", name).is_err() {
        return Err(DeleteError::Target(format!(
            "{name:?} is not a valid bare entity name"
        )));
    }
    let mut found = Vec::new();
    for collection in known_collections() {
        let key =
            EntityKey::new(collection, name).expect("known collections are valid entity keys");
        if remote.exists(&key).map_err(|why| DeleteError::Remote {
            entity: name.to_string(),
            why,
        })? {
            found.push(key);
        }
    }
    match found.as_slice() {
        [] => Err(DeleteError::Target(format!(
            "no deletable server entity named {name}"
        ))),
        [key] => Ok(key.clone()),
        _ => Err(DeleteError::Target(format!(
            "{name} is ambiguous on the server; it exists in {}",
            found
                .iter()
                .map(|key| key.collection())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn known_collections() -> Vec<&'static str> {
    let mut collections: Vec<&str> = COLLECTION_ORDER
        .iter()
        .copied()
        .filter(|collection| method_for(collection).is_some())
        .collect();
    for extra in [
        "DataTables",
        "Localizations",
        "MashupGadgets",
        "Schedulers",
        "Streams",
        "Timers",
        "ValueStreams",
    ] {
        if !collections.contains(&extra) {
            collections.push(extra);
        }
    }
    collections
}

/// Within the set, an entity goes after every entity of the set that depends on it: a template
/// cannot be deleted while another template in the set still inherits from it. The collection order
/// is kept wherever there is no such dependency; a cycle keeps the order it had.
fn delete_dependents_first(entities: &mut Vec<EntityResult>) {
    let in_set: BTreeSet<EntityKey> = entities.iter().map(EntityResult::key).collect();
    let mut remaining: Vec<EntityResult> = std::mem::take(entities);
    let mut done: BTreeSet<EntityKey> = BTreeSet::new();
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .position(|entity| {
                entity.dependents.iter().all(|dependent| {
                    let Ok(key) = EntityKey::new(&dependent.collection, &dependent.name) else {
                        return true;
                    };
                    !in_set.contains(&key) || done.contains(&key) || key == entity.key()
                })
            })
            .unwrap_or(0);
        let entity = remaining.remove(next);
        done.insert(entity.key());
        entities.push(entity);
    }
}

fn order_targets(targets: &mut [Target]) {
    targets.sort_by(|left, right| {
        let rank = |collection: &str| {
            COLLECTION_ORDER
                .iter()
                .position(|known| *known == collection)
        };
        match (rank(left.key.collection()), rank(right.key.collection())) {
            (None, None) => (left.key.name(), left.key.collection())
                .cmp(&(right.key.name(), right.key.collection())),
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(left_rank), Some(right_rank)) => right_rank
                .cmp(&left_rank)
                .then_with(|| left.key.name().cmp(right.key.name())),
        }
    });
}

fn is_file_repository(bytes: &[u8]) -> bool {
    let Ok(tokens) = scan::tokenize(bytes) else {
        return false;
    };
    let Some(entity) = tokens
        .iter()
        .filter(|token| matches!(token.kind, Kind::Start | Kind::Empty))
        .nth(2)
    else {
        return false;
    };
    scan::attribute(bytes, entity, "thingTemplate")
        .ok()
        .flatten()
        .is_some_and(|span| {
            scan::decode_entities(&String::from_utf8_lossy(span.of(bytes))) == "FileRepository"
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        held: RefCell<BTreeSet<(String, String)>>,
        dependencies: BTreeMap<(String, String), Vec<Dependent>>,
        repository_things: BTreeSet<String>,
        calls: RefCell<Vec<String>>,
        keep_after_delete: BTreeSet<(String, String)>,
        backup_fails: bool,
    }

    impl Fake {
        fn new(held: &[(&str, &str)]) -> Self {
            Self {
                held: RefCell::new(
                    held.iter()
                        .map(|(c, n)| ((*c).to_string(), (*n).to_string()))
                        .collect(),
                ),
                dependencies: BTreeMap::new(),
                repository_things: BTreeSet::new(),
                calls: RefCell::new(Vec::new()),
                keep_after_delete: BTreeSet::new(),
                backup_fails: false,
            }
        }
    }

    impl Remote for Fake {
        fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
            self.calls.borrow_mut().push(format!("GET {key}"));
            Ok(self
                .held
                .borrow()
                .contains(&(key.collection().to_string(), key.name().to_string())))
        }
        fn incoming(&self, key: &EntityKey) -> Result<Vec<Dependent>, ServerError> {
            self.calls.borrow_mut().push(format!("DEPS {key}"));
            Ok(self
                .dependencies
                .get(&(key.collection().to_string(), key.name().to_string()))
                .cloned()
                .unwrap_or_default())
        }
        fn backup(
            &self,
            _: &Solution,
            entities: &[EntityKey],
            stamp: &str,
        ) -> Result<Option<String>, backup::BackupError> {
            let names: Vec<&str> = entities.iter().map(EntityKey::name).collect();
            self.calls
                .borrow_mut()
                .push(format!("BACKUP {stamp} {}", names.join(",")));
            if self.backup_fails {
                return Err(backup::BackupError::Unreadable {
                    entity: "Things/X".to_string(),
                });
            }
            Ok(Some(format!(".twaco/backups/{stamp}")))
        }
        fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
            self.calls.borrow_mut().push(format!("FETCH {key}"));
            let template = if self.repository_things.contains(key.name()) {
                "FileRepository"
            } else {
                "GenericThing"
            };
            Ok(format!("<Entities><Things><Thing name=\"{}\" thingTemplate=\"{template}\"/></Things></Entities>", key.name()).into_bytes())
        }
        fn delete_service(&self, service: &str, name: &str) -> Result<(), ServerError> {
            self.calls
                .borrow_mut()
                .push(format!("SERVICE {service} {name}"));
            let key = self
                .held
                .borrow()
                .iter()
                .find(|(_, held_name)| held_name == name)
                .cloned();
            if let Some(key) = key {
                if !self.keep_after_delete.contains(&key) {
                    self.held.borrow_mut().remove(&key);
                }
            }
            Ok(())
        }
        fn delete_rest(&self, key: &EntityKey) -> Result<(), ServerError> {
            self.calls.borrow_mut().push(format!("DELETE {key}"));
            let held_key = (key.collection().to_string(), key.name().to_string());
            if !self.keep_after_delete.contains(&held_key) {
                self.held.borrow_mut().remove(&held_key);
            }
            Ok(())
        }
    }

    fn solution() -> (PathBuf, Solution) {
        let root = std::env::temp_dir().join(format!(
            "twaco-delete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\nroot = \".\"\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    fn execute(
        fake: &Fake,
        solution: &Solution,
        names: &[&str],
        apply: bool,
        force: bool,
    ) -> Report {
        execute_ack(
            fake,
            solution,
            names,
            apply,
            acknowledged(force, false, false, false).0,
        )
    }

    fn execute_ack(
        fake: &Fake,
        solution: &Solution,
        names: &[&str],
        apply: bool,
        acknowledged: Acknowledged,
    ) -> Report {
        let names = names
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        let prepared = prepare(solution, &names, false).unwrap();
        run(fake, solution, prepared, apply, acknowledged, "2026-10-02").unwrap()
    }

    #[test]
    fn a_plan_sends_only_reads_and_reports_absent_entities() {
        let (root, solution) = solution();
        let fake = Fake::new(&[("Things", "T")]);
        let report = execute(
            &fake,
            &solution,
            &["Things/T", "Mashups/Missing"],
            false,
            false,
        );
        assert_eq!(
            report
                .entities
                .iter()
                .map(|e| &e.status)
                .collect::<Vec<_>>(),
            [&Status::Absent, &Status::Ready]
        );
        assert!(!fake
            .calls
            .borrow()
            .iter()
            .any(|call| call.starts_with("DELETE") || call.starts_with("SERVICE")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reverse_collection_order_deletes_dependents_first_and_uses_both_methods() {
        let (root, solution) = solution();
        let fake = Fake::new(&[("DataShapes", "D"), ("ThingShapes", "S"), ("Things", "T")]);
        let report = execute(
            &fake,
            &solution,
            &["DataShapes/D", "Things/T", "ThingShapes/S"],
            true,
            false,
        );
        assert!(report.entities.iter().all(|e| e.status == Status::Deleted));
        let deletes: Vec<String> = fake
            .calls
            .borrow()
            .iter()
            .filter(|call| call.starts_with("DELETE") || call.starts_with("SERVICE"))
            .cloned()
            .collect();
        assert_eq!(
            deletes,
            [
                "SERVICE DeleteThing T",
                "SERVICE DeleteThingShape S",
                "DELETE DataShapes/D"
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn outside_dependents_and_repository_definitions_refuse_unless_forced() {
        let (root, solution) = solution();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(
            root.join("Things/T.xml"),
            "<Entities><Things><Thing name=\"T\" projectName=\"P\"/></Things></Entities>",
        )
        .unwrap();
        let mut fake = Fake::new(&[("Things", "T")]);
        fake.dependencies.insert(
            ("Things".into(), "T".into()),
            vec![Dependent {
                collection: "Mashups".into(),
                name: "M".into(),
            }],
        );
        let refused = execute(&fake, &solution, &["Things/T"], true, false);
        assert_eq!(refused.entities[0].status, Status::Refused);
        assert_eq!(refused.entities[0].refusals().count(), 2);
        let forced = execute(&fake, &solution, &["Things/T"], true, true);
        assert_eq!(forced.entities[0].status, Status::Deleted);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn acknowledgements_independently_guard_repository_dependents_and_file_data() {
        let (root, solution) = solution();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(
            root.join("Things/Repo.xml"),
            "<Entities><Things><Thing name=\"Repo\" projectName=\"P\"/></Things></Entities>",
        )
        .unwrap();
        let mut fake = Fake::new(&[("Things", "Repo")]);
        fake.repository_things.insert("Repo".into());
        fake.dependencies.insert(
            ("Things".into(), "Repo".into()),
            vec![Dependent {
                collection: "Mashups".into(),
                name: "Outside".into(),
            }],
        );
        for repository_defined in [false, true] {
            for outside_dependents in [false, true] {
                for file_repository_data_loss in [false, true] {
                    let report = execute_ack(
                        &fake,
                        &solution,
                        &["Things/Repo"],
                        false,
                        Acknowledged {
                            repository_defined,
                            outside_dependents,
                            file_repository_data_loss,
                        },
                    );
                    let entity = &report.entities[0];
                    let expected = [
                        (!repository_defined).then_some(GuardCode::RepositoryDefined),
                        (!outside_dependents).then_some(GuardCode::OutsideDependents),
                        (!file_repository_data_loss).then_some(GuardCode::FileRepositoryDataLoss),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                    assert_eq!(entity.refusal_codes().collect::<Vec<_>>(), expected);
                    assert_eq!(
                        entity.status,
                        if expected.is_empty() {
                            Status::Ready
                        } else {
                            Status::Refused
                        }
                    );
                    assert_eq!(
                        entity.warnings.len(),
                        usize::from(file_repository_data_loss)
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_force_acknowledges_only_its_two_original_guards() {
        assert_eq!(
            acknowledged(false, false, false, false),
            (Acknowledged::default(), false)
        );
        assert_eq!(
            acknowledged(true, false, false, false),
            (
                Acknowledged {
                    repository_defined: true,
                    outside_dependents: true,
                    file_repository_data_loss: false,
                },
                true,
            )
        );
        assert_eq!(
            acknowledged(true, false, false, true),
            (
                Acknowledged {
                    repository_defined: true,
                    outside_dependents: true,
                    file_repository_data_loss: true,
                },
                true,
            )
        );
        assert_eq!(
            acknowledged(false, true, false, true),
            (
                Acknowledged {
                    repository_defined: true,
                    outside_dependents: false,
                    file_repository_data_loss: true,
                },
                false,
            )
        );
        assert_eq!(
            acknowledged(false, false, true, false),
            (
                Acknowledged {
                    repository_defined: false,
                    outside_dependents: true,
                    file_repository_data_loss: false,
                },
                false,
            )
        );
    }

    #[test]
    fn refusals_serialize_as_parallel_messages_and_codes() {
        let (root, solution) = solution();
        let fake = Fake::new(&[("Unknowns", "T")]);
        let prepared = prepare(&solution, &["Unknowns/T".into()], false).unwrap();
        let refused = run(
            &fake,
            &solution,
            prepared,
            false,
            Acknowledged::default(),
            "2026-10-02",
        )
        .unwrap();
        let value = serde_json::to_value(&refused.entities[0]).unwrap();
        assert_eq!(
            value["refusals"],
            serde_json::json!([
                "twaco has no delete method for this collection; delete it in Composer"
            ])
        );
        assert_eq!(
            value["refusal_codes"],
            serde_json::json!(["no_delete_method"])
        );

        let prepared = prepare(&solution, &["Unknowns/T".into()], false).unwrap();
        let still_refused = run(
            &fake,
            &solution,
            prepared,
            false,
            Acknowledged {
                repository_defined: true,
                outside_dependents: true,
                file_repository_data_loss: true,
            },
            "2026-10-02",
        )
        .unwrap();
        assert_eq!(still_refused.entities[0].status, Status::Refused);
        assert_eq!(
            still_refused.entities[0]
                .refusal_codes()
                .collect::<Vec<_>>(),
            [GuardCode::NoDeleteMethod]
        );

        let absent = execute(&fake, &solution, &["Things/Missing"], false, false);
        let value = serde_json::to_value(&absent.entities[0]).unwrap();
        assert!(value.get("refusals").is_none());
        assert!(value.get("refusal_codes").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_qualified_names_keep_the_target_error() {
        let (root, solution) = solution();
        let fake = Fake::new(&[]);
        for requested in ["Things/..", "Things/A/B", "/X"] {
            let prepared = prepare(&solution, &[requested.to_string()], false).unwrap();
            let error = run(
                &fake,
                &solution,
                prepared,
                false,
                Acknowledged::default(),
                "2026-10-02",
            )
            .unwrap_err()
            .to_string();
            assert_eq!(
                error,
                format!("{requested:?} must be Collection/Name or a bare server entity name")
            );
        }
        assert!(fake.calls.borrow().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_invalid_ledger_entity_key_is_reported_without_a_request() {
        let (root, solution) = solution();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(
            root.join(".twaco/renames.json"),
            r#"[{"date":"2026-10-01","kind":"entity","old":"Bad","new":"New","entities":[{"collection":"Things","old":"..","new":"New"}]}]"#,
        )
        .unwrap();
        let fake = Fake::new(&[]);
        let prepared = prepare(&solution, &[], true).unwrap();
        let error = run(
            &fake,
            &solution,
            prepared,
            false,
            Acknowledged::default(),
            "2026-10-02",
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            format!(
                "{}: invalid rename ledger: Things/.. is not a valid entity key",
                root.join(".twaco/renames.json").display()
            )
        );
        assert!(fake.calls.borrow().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_invalid_dependent_key_is_outside_the_delete_set() {
        let (root, solution) = solution();
        let mut fake = Fake::new(&[("Things", "T")]);
        fake.dependencies.insert(
            ("Things".into(), "T".into()),
            vec![Dependent {
                collection: "Mashups".into(),
                name: "..".into(),
            }],
        );
        let report = execute(&fake, &solution, &["Things/T"], false, false);
        let entity = &report.entities[0];
        assert_eq!(entity.status, Status::Refused);
        assert_eq!(
            entity.refusal_codes().collect::<Vec<_>>(),
            [GuardCode::OutsideDependents]
        );
        assert_eq!(
            entity.refusals().collect::<Vec<_>>(),
            ["incoming dependents outside this delete set: Mashups/.. (pass --allow-outside-dependents)"]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refused_and_deleted_entity_results_keep_their_json_shape() {
        let (root, solution) = solution();
        let fake = Fake::new(&[("Unknowns", "T"), ("Mashups", "M")]);
        let refused = execute(&fake, &solution, &["Unknowns/T"], false, false);
        assert_eq!(
            serde_json::to_value(&refused.entities[0]).unwrap(),
            serde_json::json!({
                "collection": "Unknowns",
                "name": "T",
                "status": "refused",
                "method": "delete it in Composer",
                "dependents": [],
                "warnings": [],
                "refusals": ["twaco has no delete method for this collection; delete it in Composer"],
                "refusal_codes": ["no_delete_method"]
            })
        );
        let deleted = execute(&fake, &solution, &["Mashups/M"], true, false);
        assert_eq!(
            serde_json::to_value(&deleted.entities[0]).unwrap(),
            serde_json::json!({
                "collection": "Mashups",
                "name": "M",
                "status": "deleted",
                "method": "REST DELETE",
                "dependents": [],
                "warnings": []
            })
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_dependent_inside_the_set_is_allowed_and_repository_files_are_warned() {
        let (root, solution) = solution();
        let mut fake = Fake::new(&[("ThingTemplates", "Base"), ("Things", "Repo")]);
        fake.dependencies.insert(
            ("ThingTemplates".into(), "Base".into()),
            vec![Dependent {
                collection: "Things".into(),
                name: "Repo".into(),
            }],
        );
        fake.repository_things.insert("Repo".into());
        let report = execute_ack(
            &fake,
            &solution,
            &["ThingTemplates/Base", "Things/Repo"],
            false,
            Acknowledged {
                file_repository_data_loss: true,
                ..Acknowledged::default()
            },
        );
        assert!(report
            .entities
            .iter()
            .all(|entity| entity.status == Status::Ready));
        assert!(report
            .entities
            .iter()
            .find(|entity| entity.name == "Repo")
            .unwrap()
            .warnings[0]
            .contains("deletes all"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_success_response_that_did_not_delete_is_a_per_entity_failure_and_the_run_continues() {
        let (root, solution) = solution();
        let mut fake = Fake::new(&[("Mashups", "A"), ("Mashups", "B")]);
        fake.keep_after_delete
            .insert(("Mashups".into(), "A".into()));
        let report = execute(&fake, &solution, &["Mashups/A", "Mashups/B"], true, false);
        assert!(report.failed());
        assert_eq!(
            report
                .entities
                .iter()
                .find(|e| e.name == "A")
                .unwrap()
                .status,
            Status::Failed
        );
        assert_eq!(
            report
                .entities
                .iter()
                .find(|e| e.name == "B")
                .unwrap()
                .status,
            Status::Deleted
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn renamed_marks_deleted_and_absent_entries_and_skips_already_marked_and_member_records() {
        let (root, solution) = solution();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        let ledger = serde_json::json!([
            {"date":"2026-10-01","kind":"entity","old":"Old","new":"New","entities":[
                {"collection":"Things","old":"Old","new":"New"},
                {"collection":"Mashups","old":"Gone","new":"NewMashup","deleted":"2026-10-01"}
            ]},
            {"date":"2026-10-01","kind":"field","old":"a","new":"b","entities":[
                {"collection":"DataShapes","old":"D","new":"D"}
            ]}
        ]);
        std::fs::write(
            root.join(".twaco/renames.json"),
            serde_json::to_vec_pretty(&ledger).unwrap(),
        )
        .unwrap();
        let fake = Fake::new(&[("Things", "Old")]);
        let prepared = prepare(&solution, &[], true).unwrap();
        let report = run(
            &fake,
            &solution,
            prepared,
            true,
            Acknowledged::default(),
            "2026-10-02",
        )
        .unwrap();
        assert_eq!(report.entities.len(), 1);
        assert!(report.ledger_changed);
        let updated: Value =
            serde_json::from_slice(&std::fs::read(root.join(".twaco/renames.json")).unwrap())
                .unwrap();
        assert_eq!(updated[0]["entities"][0]["deleted"], "2026-10-02");
        assert_eq!(updated[0]["entities"][1]["deleted"], "2026-10-01");
        assert!(updated[1]["entities"][0].get("deleted").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_delete_set_is_backed_up_before_the_first_delete_and_a_failed_backup_deletes_nothing() {
        let (root, solution) = solution();
        let fake = Fake::new(&[("Things", "A"), ("Things", "B")]);
        let names = vec![
            "Things/A".to_string(),
            "Things/B".to_string(),
            "Things/Absent".to_string(),
        ];
        let prepared = prepare(&solution, &names, false)
            .unwrap()
            .with_backup("20261002-1");
        let report = run(
            &fake,
            &solution,
            prepared,
            true,
            Acknowledged::default(),
            "2026-10-02",
        )
        .unwrap();
        assert_eq!(report.backup.as_deref(), Some(".twaco/backups/20261002-1"));
        let calls = fake.calls.borrow();
        let backup_at = calls
            .iter()
            .position(|call| call.starts_with("BACKUP"))
            .unwrap();
        let first_delete = calls
            .iter()
            .position(|call| call.starts_with("SERVICE"))
            .unwrap();
        assert!(backup_at < first_delete, "{calls:?}");
        assert_eq!(
            calls[backup_at], "BACKUP 20261002-1 A,B",
            "only what will be deleted is saved"
        );
        drop(calls);
        // A plan takes no backup, and --no-backup (no stamp) takes none.
        let plan_fake = Fake::new(&[("Things", "A")]);
        let planned = prepare(&solution, &["Things/A".to_string()], false)
            .unwrap()
            .with_backup("s");
        run(
            &plan_fake,
            &solution,
            planned,
            false,
            Acknowledged::default(),
            "d",
        )
        .unwrap();
        let unbacked = Fake::new(&[("Things", "A")]);
        execute(&unbacked, &solution, &["Things/A"], true, false);
        assert!(plan_fake
            .calls
            .borrow()
            .iter()
            .chain(unbacked.calls.borrow().iter())
            .all(|call| !call.starts_with("BACKUP")));
        // A backup that fails stops everything before any delete.
        let mut failing = Fake::new(&[("Things", "A")]);
        failing.backup_fails = true;
        let prepared = prepare(&solution, &["Things/A".to_string()], false)
            .unwrap()
            .with_backup("s");
        let error = run(
            &failing,
            &solution,
            prepared,
            true,
            Acknowledged::default(),
            "d",
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("nothing was deleted") && error.contains("--no-backup"),
            "{error}"
        );
        assert!(failing
            .calls
            .borrow()
            .iter()
            .all(|call| !call.starts_with("SERVICE") && !call.starts_with("DELETE")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_entity_is_deleted_after_the_entities_of_the_set_that_depend_on_it() {
        let (root, solution) = solution();
        let mut fake = Fake::new(&[
            ("ThingTemplates", "A_TT"),
            ("ThingTemplates", "B_TT"),
            ("ThingTemplates", "C_TT"),
        ]);
        // B and C inherit from A: both must go first, though A sorts before them by name.
        let dependent = |name: &str| Dependent {
            collection: "ThingTemplates".to_string(),
            name: name.to_string(),
        };
        fake.dependencies.insert(
            ("ThingTemplates".to_string(), "A_TT".to_string()),
            vec![dependent("B_TT"), dependent("C_TT")],
        );
        let report = execute(
            &fake,
            &solution,
            &[
                "ThingTemplates/A_TT",
                "ThingTemplates/B_TT",
                "ThingTemplates/C_TT",
            ],
            true,
            false,
        );
        let order: Vec<&str> = report
            .entities
            .iter()
            .map(|entity| entity.name.as_str())
            .collect();
        assert_eq!(order, ["B_TT", "C_TT", "A_TT"]);
        assert!(
            report
                .entities
                .iter()
                .all(|entity| entity.status == Status::Deleted),
            "{:?}",
            report
                .entities
                .iter()
                .map(|e| (&e.name, &e.status))
                .collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_ledger_and_unknown_collection_refuse_before_any_request() {
        let (root, solution) = solution();
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join(".twaco/renames.json"), "not json").unwrap();
        let fake = Fake::new(&[]);
        assert!(prepare(&solution, &["Things/T".into()], false).is_err());
        assert!(fake.calls.borrow().is_empty());
        std::fs::write(root.join(".twaco/renames.json"), "[]").unwrap();
        let prepared = prepare(&solution, &["Unknowns/T".into()], false).unwrap();
        let report = run(
            &fake,
            &solution,
            prepared,
            false,
            Acknowledged::default(),
            "2026-10-02",
        )
        .unwrap();
        assert_eq!(report.entities[0].status, Status::Refused);
        assert_eq!(report.entities[0].method, Method::Composer);
        assert!(fake.calls.borrow().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_collection_without_a_delete_method_is_refused_whatever_is_acknowledged() {
        let (root, solution) = solution();
        let fake = Fake::new(&[]);
        let everything = Acknowledged {
            repository_defined: true,
            outside_dependents: true,
            file_repository_data_loss: true,
        };
        let report = execute_ack(&fake, &solution, &["Unknowns/T"], true, everything);
        let entity = &report.entities[0];
        assert_eq!(entity.status, Status::Refused);
        assert_eq!(
            entity.refusal_codes().collect::<Vec<_>>(),
            [GuardCode::NoDeleteMethod]
        );
        assert!(
            fake.calls.borrow().is_empty(),
            "nothing is asked of the server for it"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bare_name_resolves_on_the_server_and_ambiguity_lists_its_collections() {
        let (root, solution) = solution();
        let one = Fake::new(&[("Mashups", "Shared")]);
        let report = execute(&one, &solution, &["Shared"], false, false);
        assert_eq!(
            (
                report.entities[0].collection.as_str(),
                report.entities[0].name.as_str()
            ),
            ("Mashups", "Shared")
        );

        let several = Fake::new(&[("Mashups", "Shared"), ("Things", "Shared")]);
        let prepared = prepare(&solution, &["Shared".into()], false).unwrap();
        let error = run(
            &several,
            &solution,
            prepared,
            false,
            Acknowledged::default(),
            "2026-10-02",
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("ambiguous") && error.contains("Mashups") && error.contains("Things"),
            "{error}"
        );
        assert!(!several
            .calls
            .borrow()
            .iter()
            .any(|call| call.starts_with("DELETE") || call.starts_with("SERVICE")));
        let _ = std::fs::remove_dir_all(root);
    }
}
