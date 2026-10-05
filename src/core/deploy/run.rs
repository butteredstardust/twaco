use super::super::entity_key::ServiceTarget;
use super::super::normalise;
use super::super::parallel;
use super::super::profile::Profile;
use super::super::push::Decision;
use super::super::server::ServerError;
use super::calls::{redact_placeholder_values, resolve_call};
use super::{
    decide_all, BaselineStore, DeployError, Entity, EntityPlan, NotKept, ParseFailure, PlannedCall,
    ProjectBundle, Remote, Report, Script, ServiceCall,
};
use std::collections::BTreeMap;
use std::time::Duration;

/// Plan or apply all project bundles. Projects must already be in dependency order.
pub fn run(
    remote: &dyn Remote,
    baselines: &dyn BaselineStore,
    profile: &Profile,
    projects: &[ProjectBundle],
    apply: bool,
    force: bool,
    only: bool,
) -> Result<Report, DeployError> {
    let mut report = Report {
        projects: projects
            .iter()
            .map(|project| project.project.clone())
            .collect(),
        ..Report::default()
    };

    // Resolve every placeholder before the first server operation (including live parse). The
    // resolved copies are deliberately kept out of Report and every error type, so a plan and
    // diagnostics can show `${profile:key}` but can never print the substituted secret.
    let mut resolved_calls: BTreeMap<String, (Option<ServiceCall>, Vec<ServiceCall>)> =
        BTreeMap::new();
    for project in projects {
        let deploy = project
            .deploy
            .as_ref()
            .map(|call| resolve_call(call, profile, &project.project))
            .transpose()?;
        let post_import = project
            .post_import
            .iter()
            .map(|call| resolve_call(call, profile, &project.project))
            .collect::<Result<Vec<_>, _>>()?;
        resolved_calls.insert(project.project.clone(), (deploy, post_import));
        if let Some(call) = &project.deploy {
            report.calls.push(PlannedCall {
                project: project.project.clone(),
                call: call.clone(),
                post_import: false,
                skipped: false,
            });
        }
        report
            .calls
            .extend(project.post_import.iter().cloned().map(|call| PlannedCall {
                project: project.project.clone(),
                call,
                post_import: true,
                skipped: only,
            }));
    }

    let scripts: Vec<&Script> = projects
        .iter()
        .flat_map(|project| &project.scripts)
        .collect();
    let checks = parallel::map(&scripts, |script| {
        let script = *script;
        remote
            .check_script(&script.source)
            .map(|checked| (script, checked))
            .map_err(|source| DeployError::ParseUnavailable {
                entity: script.entity.clone(),
                service: script.service.clone(),
                source,
            })
    });
    let mut parse_failures = Vec::new();
    for result in checks {
        let (script, checked) = result?;
        report.scripts_checked += 1;
        if !checked.status {
            parse_failures.push(ParseFailure {
                entity: script.entity.clone(),
                service: script.service.clone(),
                line: checked.line_number,
                column: checked.column_number,
                message: checked.message,
            });
        }
    }
    if !parse_failures.is_empty() {
        return Err(DeployError::ParseFailed(parse_failures));
    }

    let mut baseline = baselines.load().map_err(DeployError::Baseline)?;
    report.plans = decide_all(remote, &baseline, projects)?;

    let conflicts: Vec<EntityPlan> = report
        .plans
        .iter()
        .filter(|plan| matches!(plan.decision, Decision::Refuse(_)))
        .cloned()
        .collect();
    if !conflicts.is_empty() && !force {
        return Err(DeployError::Conflicts(conflicts));
    }
    if !apply {
        return Ok(report);
    }

    // A failed import stops the projects after it, but the ones before it are on the server by
    // then. They are still read back and recorded, so a partial deploy does not later look like
    // someone else's change to entities this deploy wrote.
    let mut import_failure = None;
    for project in projects {
        match remote.import(&project.file_name, &project.bytes) {
            Ok(()) => report.imported.push(project.project.clone()),
            Err(source) => {
                import_failure = Some(DeployError::Import {
                    project: project.project.clone(),
                    source,
                    imported: report.imported.clone(),
                });
                break;
            }
        }
    }

    let imported_entities: Vec<&Entity> = projects
        .iter()
        .filter(|project| report.imported.contains(&project.project))
        .flat_map(|project| &project.entities)
        .collect();
    let read_backs = parallel::map(&imported_entities, |entity| {
        let entity = *entity;
        let sent = normalise::hash(&entity.bytes).map_err(|error| DeployError::Working {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            why: error.to_string(),
        })?;
        let fetched = remote.fetch(&entity.collection, &entity.name);
        let (read_back, error) = match fetched {
            Ok(Some(bytes)) => match normalise::hash(&bytes) {
                Ok(hash) => (Some(hash), None),
                Err(why) => (None, Some(why.to_string())),
            },
            Ok(None) => (None, None),
            Err(why) => (None, Some(why.to_string())),
        };
        Ok((entity, sent, read_back, error))
    });
    let mut first_read_back = BTreeMap::<(String, String), String>::new();
    for result in read_backs {
        let (entity, sent, read_back, error) = result?;
        if read_back.as_deref() == Some(sent.as_str()) {
            let read_back = read_back.expect("matching read-back is present");
            baseline.set(
                &entity.collection,
                &entity.name,
                sent.clone(),
                read_back.clone(),
            );
            first_read_back.insert((entity.collection.clone(), entity.name.clone()), read_back);
            report
                .kept
                .push((entity.collection.clone(), entity.name.clone()));
        } else {
            report.not_kept.push(NotKept {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                sent,
                read_back,
                error,
            });
        }
    }

    // Import and read-back failures stop before service calls. What did import is nevertheless
    // persisted below, preserving the partial-import rule with the same single baseline write.
    if let Some(failure) = import_failure {
        // The import's failure is the news; a baseline write failing after it is added to it,
        // never put in its place.
        if let Err(why) = baselines.write(&baseline) {
            return Err(DeployError::Unrecorded {
                failure: Box::new(failure),
                why,
                imported: report.imported.clone(),
            });
        }
        return Err(failure);
    }
    if !report.not_kept.is_empty() {
        baselines.write(&baseline).map_err(DeployError::Baseline)?;
        return Err(DeployError::NotKept(Box::new(report)));
    }

    let mut call_failure = None;
    'projects: for project in projects {
        let (deploy, post_import) = resolved_calls
            .get(&project.project)
            .expect("every project was resolved before server traffic");
        let calls = deploy
            .iter()
            .chain((!only).then_some(post_import).into_iter().flatten());
        for call in calls {
            let outcome = ServiceTarget::parse(&call.target)
                .map_err(ServerError::from)
                .and_then(|target| {
                    remote.call_service(
                        &target,
                        &call.service,
                        &call.parameters,
                        Duration::from_secs(300),
                    )
                });
            if let Err(source) = outcome {
                call_failure = Some(DeployError::Call {
                    project: project.project.clone(),
                    target: call.target.clone(),
                    service: call.service.clone(),
                    why: redact_placeholder_values(&source.to_string(), profile, projects),
                    imported: report.imported.clone(),
                });
                break 'projects;
            }
        }
    }

    if call_failure.is_none() {
        let re_reads = parallel::map(&imported_entities, |entity| {
            let entity = *entity;
            let bytes = remote
                .fetch(&entity.collection, &entity.name)
                .map_err(|why| DeployError::Server {
                    collection: entity.collection.clone(),
                    name: entity.name.clone(),
                    why: why.to_string(),
                })?
                .ok_or_else(|| DeployError::Server {
                    collection: entity.collection.clone(),
                    name: entity.name.clone(),
                    why: "the entity disappeared after its deploy calls".to_string(),
                })?;
            let hash = normalise::hash(&bytes).map_err(|why| DeployError::Server {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                why: why.to_string(),
            })?;
            Ok((entity, hash))
        });
        for result in re_reads {
            match result {
                Ok((entity, hash)) => {
                    let key = (entity.collection.clone(), entity.name.clone());
                    if first_read_back
                        .get(&key)
                        .is_some_and(|before| before != &hash)
                    {
                        // Deploy-service mutations (for example restoring an imported Database
                        // password) are part of this deploy. The working file correctly omits
                        // them; advancing only the server side keeps both sides in sync.
                        baseline
                            .set_server(&entity.collection, &entity.name, hash)
                            .map_err(DeployError::Baseline)?;
                        report.changed_by_deploy.push(key);
                    }
                }
                Err(error) => {
                    // Every project had imported by now; say so with the failure.
                    call_failure = Some(DeployError::AfterImport {
                        imported: report.imported.clone(),
                        source: Box::new(error),
                    });
                    break;
                }
            }
        }
    }

    if let Err(why) = baselines.write(&baseline) {
        return Err(match call_failure {
            Some(failure) => DeployError::Unrecorded {
                failure: Box::new(failure),
                why,
                imported: report.imported.clone(),
            },
            None => DeployError::Baseline(why),
        });
    }
    if let Some(failure) = call_failure {
        Err(failure)
    } else {
        Ok(report)
    }
}
