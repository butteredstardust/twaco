use super::guards::{Acknowledged, GuardCode, Refusal};
use super::method::Method;
use super::remote::{is_disabled_thing, Dependent, Remote};
use super::report::{DeleteError, EntityResult, Report, Status};
use super::targets::{order_targets, resolve_targets, Prepared, Target};
use super::DEPENDENCY_LIMIT;
use crate::core::config::Solution;
use crate::core::entity::key::EntityKey;
use crate::core::scan::{self, Kind};
use crate::core::workspace;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn run(
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
    let project_dependents = project_dependents(remote, &targets)?;
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
                unlink: None,
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
                unlink: None,
            });
            continue;
        }
        let (mut dependents, disabled) = match remote.incoming(&key) {
            Ok(dependents) => (dependents, false),
            Err(why) if is_disabled_thing(&key, &why) => (Vec::new(), true),
            Err(why) => {
                return Err(DeleteError::Remote {
                    entity: label.clone(),
                    why,
                })
            }
        };
        if let Some(needing) = project_dependents.get(key.name()) {
            if key.collection() == "Projects" {
                dependents.extend(needing.iter().map(|name| Dependent {
                    collection: "Projects".to_string(),
                    name: name.clone(),
                }));
            }
        }
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
        if disabled {
            let message = "the Thing is disabled, so the server cannot list what depends on it";
            if acknowledged.outside_dependents {
                warnings.push(message.to_string());
            } else {
                refusals.push(Refusal {
                    code: GuardCode::OutsideDependents,
                    message: format!(
                        "{message} (enable it to see them, or pass --allow-outside-dependents)"
                    ),
                });
            }
        }
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
            unlink: None,
        });
    }

    super::cycle::plan(remote, &mut entities)
        .map_err(|(entity, why)| DeleteError::Remote { entity, why })?;
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
        // Break each cycle first; an entity that is then not deleted gets its defaults back.
        let mut unlinked = Vec::new();
        for (index, entity) in entities.iter_mut().enumerate() {
            let Some((_, stripped)) = &entity.unlink else {
                continue;
            };
            if entity.status != Status::Ready {
                continue;
            }
            let file = format!("{}.xml", entity.name);
            match remote.import(&file, stripped) {
                Ok(()) => unlinked.push(index),
                Err(error) => {
                    entity.status = Status::Failed;
                    entity.error = Some(format!(
                        "clearing the defaults that close the cycle failed: {error}"
                    ));
                }
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
        for index in unlinked {
            let entity = &mut entities[index];
            if entity.status == Status::Deleted {
                continue;
            }
            let Some((original, _)) = &entity.unlink else {
                continue;
            };
            let file = format!("{}.xml", entity.name);
            let restored = match remote.import(&file, original) {
                Ok(()) => "its cleared defaults were put back".to_string(),
                Err(error) => format!(
                    "its cleared defaults could not be put back ({error}); `twaco entity restore` has the backup"
                ),
            };
            entity.error = Some(match entity.error.take() {
                Some(error) => format!("{error}; {restored}"),
                None => restored,
            });
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

/// For each Project in the set, the Projects of the set whose `dependsOn` names it: the server
/// does not count those as incoming dependencies, but refuses to delete a Project another needs.
fn project_dependents(
    remote: &dyn Remote,
    targets: &[Target],
) -> Result<BTreeMap<String, Vec<String>>, DeleteError> {
    let projects: Vec<&Target> = targets
        .iter()
        .filter(|target| target.key.collection() == "Projects")
        .collect();
    let mut needed_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for target in &projects {
        // Unread, the order could delete a needed Project first, which the server refuses
        // part-way through: refuse before anything is deleted instead.
        let remote_error = |why| DeleteError::Remote {
            entity: target.key.to_string(),
            why,
        };
        let unread = |why: String| {
            remote_error(crate::core::server::ServerError::InvalidResponse {
                url: target.key.to_string(),
                why,
            })
        };
        if !remote.exists(&target.key).map_err(remote_error)? {
            continue;
        }
        let bytes = remote.fetch(&target.key).map_err(remote_error)?;
        let project = crate::core::normalise::entity_of(&bytes)
            .map_err(|error| unread(format!("its export does not read: {error}")))?;
        let depends_on = project
            .attributes
            .iter()
            .find(|(name, _)| name == b"dependsOn")
            .map(|(_, value)| {
                serde_json::from_slice::<serde_json::Value>(value)
                    .map_err(|error| unread(format!("its dependsOn is not JSON: {error}")))
            })
            .transpose()?;
        let names = depends_on
            .as_ref()
            .and_then(|value| value["projects"].as_str())
            .unwrap_or_default();
        for name in names
            .split(',')
            .filter_map(|item| item.trim().split(':').next())
        {
            if !name.is_empty() && projects.iter().any(|other| other.key.name() == name) {
                needed_by
                    .entry(name.to_string())
                    .or_default()
                    .push(target.key.name().to_string());
            }
        }
    }
    Ok(needed_by)
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
