use super::super::config::Solution;
use super::super::entity_key::EntityKey;
use super::super::scan::{self, Kind};
use super::super::workspace;
use super::guards::{Acknowledged, GuardCode, Refusal};
use super::method::Method;
use super::remote::{Dependent, Remote};
use super::report::{DeleteError, EntityResult, Report, Status};
use super::targets::{order_targets, resolve_targets, Prepared, Target};
use super::DEPENDENCY_LIMIT;
use std::collections::BTreeSet;

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
