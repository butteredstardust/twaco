use super::super::bundle::COLLECTION_ORDER;
use super::super::config::Solution;
use super::super::entity_key::EntityKey;
use super::super::ledger::Ledger;
use super::method::{known_collections, method_for, Method};
use super::remote::Remote;
use super::report::DeleteError;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(super) struct Target {
    pub(super) key: EntityKey,
    pub(super) method: Method,
    pub(super) ledger: Vec<LedgerLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LedgerLocation {
    pub(super) record: usize,
    pub(super) entity: usize,
}

pub struct Prepared {
    pub(super) ledger_path: PathBuf,
    pub(super) ledger: Ledger,
    pub(super) requested: Vec<String>,
    pub(super) renamed: bool,
    /// The stamp of the backup set to save before deleting; `None` deletes without one.
    pub(super) backup_stamp: Option<String>,
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

pub(super) fn resolve_targets(
    remote: &dyn Remote,
    prepared: &Prepared,
) -> Result<Vec<Target>, DeleteError> {
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

pub(super) fn order_targets(targets: &mut [Target]) {
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
