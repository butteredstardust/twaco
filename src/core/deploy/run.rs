use super::super::entity_key::{EntityKey, ServiceTarget};
use super::super::normalise;
use super::super::parallel;
use super::super::profile::Profile;
use super::super::progress::{self, Progress, NONE};
use super::super::push::Decision;
use super::super::server::ServerError;
use super::calls::{redact_placeholder_values, resolve_call};
use super::{
    decide_all_with_progress, BaselineStore, DeployError, Entity, EntityPlan, NotKept,
    ParseFailure, PlannedCall, ProjectBundle, Remote, Report, Script, ServiceCall,
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
    run_with_progress(
        remote,
        baselines,
        profile,
        projects,
        RunOptions { apply, force, only },
        &NONE,
    )
}

/// What a deploy run does: send changes, override conflicts, skip post-import services.
#[derive(Clone, Copy, Debug)]
pub struct RunOptions {
    pub apply: bool,
    pub force: bool,
    pub only: bool,
}

/// Like [`run`], and report progress. Messages hold project and entity names only.
pub fn run_with_progress(
    remote: &dyn Remote,
    baselines: &dyn BaselineStore,
    profile: &Profile,
    projects: &[ProjectBundle],
    options: RunOptions,
    progress: &dyn Progress,
) -> Result<Report, DeployError> {
    let RunOptions { apply, force, only } = options;
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
    let checks = {
        let _phase = progress::phase(progress, "checking scripts", Some(scripts.len() as u64));
        parallel::map_progress(&scripts, progress, |script| {
            let script = *script;
            progress.message(&script.entity);
            remote
                .check_script(&script.source)
                .map(|checked| (script, checked))
                .map_err(|source| DeployError::ParseUnavailable {
                    entity: script.entity.clone(),
                    service: script.service.clone(),
                    source,
                })
        })
    };
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

    tracing::info!(scripts = report.scripts_checked, "deploy: scripts checked");
    let mut baseline = baselines.load().map_err(DeployError::Baseline)?;
    report.plans = {
        let entity_count = projects.iter().map(|p| p.entities.len() as u64).sum();
        let _phase = progress::phase(progress, "comparing entities", Some(entity_count));
        decide_all_with_progress(remote, &baseline, projects, progress)?
    };

    let conflicts: Vec<EntityPlan> = report
        .plans
        .iter()
        .filter(|plan| matches!(plan.decision, Decision::Refuse(_)))
        .cloned()
        .collect();
    tracing::info!(
        entities = report.plans.len(),
        conflicts = conflicts.len(),
        apply,
        force,
        "deploy: planned"
    );
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
    let import_phase = progress::phase(progress, "importing", Some(projects.len() as u64));
    for project in projects {
        progress.message(&project.project);
        let imported = remote.import(&project.file_name, &project.bytes);
        progress.advance(1);
        match imported {
            Ok(()) => {
                tracing::info!(project = %project.project, "deploy: project imported");
                report.imported.push(project.project.clone());
            }
            Err(source) => {
                tracing::info!(project = %project.project, "deploy: project import failed");
                import_failure = Some(DeployError::Import {
                    project: project.project.clone(),
                    source,
                    imported: report.imported.clone(),
                });
                break;
            }
        }
    }

    drop(import_phase);

    let imported_entities: Vec<&Entity> = projects
        .iter()
        .filter(|project| report.imported.contains(&project.project))
        .flat_map(|project| &project.entities)
        .collect();
    let read_back_phase = progress::phase(
        progress,
        "reading back",
        Some(imported_entities.len() as u64),
    );
    let read_backs = parallel::map_progress(&imported_entities, progress, |entity| {
        let entity = *entity;
        progress.message(&entity.name);
        let sent = normalise::hash(&entity.bytes).map_err(|error| DeployError::Working {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            why: error.to_string(),
        })?;
        let fetched =
            EntityKey::address(&entity.collection, &entity.name).and_then(|key| remote.fetch(&key));
        let mut only_permissions = false;
        let (read_back, error) = match fetched {
            Ok(Some(bytes)) => match normalise::hash(&bytes) {
                Ok(hash) => {
                    if hash != sent {
                        only_permissions =
                            normalise::differ_only_in_permissions(&entity.bytes, &bytes);
                    }
                    (Some(hash), None)
                }
                Err(why) => (None, Some(why.to_string())),
            },
            Ok(None) => (None, None),
            Err(why) => (None, Some(why.to_string())),
        };
        Ok((entity, sent, read_back, error, only_permissions))
    });
    drop(read_back_phase);
    let mut first_read_back = BTreeMap::<(String, String), String>::new();
    for result in read_backs {
        let (entity, sent, read_back, error, only_permissions) = result?;
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
                only_permissions,
            });
        }
    }

    tracing::info!(
        kept = report.kept.len(),
        not_kept = report.not_kept.len(),
        "deploy: read back"
    );
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
    let call_total = projects
        .iter()
        .map(|project| {
            let (deploy, post_import) = &resolved_calls[&project.project];
            deploy.iter().count() + if only { 0 } else { post_import.len() }
        })
        .sum::<usize>();
    let call_phase = progress::phase(progress, "calling services", Some(call_total as u64));
    'projects: for project in projects {
        let (deploy, post_import) = resolved_calls
            .get(&project.project)
            .expect("every project was resolved before server traffic");
        let calls = deploy
            .iter()
            .chain((!only).then_some(post_import).into_iter().flatten());
        for call in calls {
            progress.message(&project.project);
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
            tracing::info!(
                project = %project.project,
                ok = outcome.is_ok(),
                "deploy: service called"
            );
            progress.advance(1);
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

    drop(call_phase);
    if call_failure.is_none() {
        let re_read_phase = progress::phase(
            progress,
            "reading back again",
            Some(imported_entities.len() as u64),
        );
        let re_reads = parallel::map_progress(&imported_entities, progress, |entity| {
            let entity = *entity;
            progress.message(&entity.name);
            let bytes = EntityKey::address(&entity.collection, &entity.name)
                .and_then(|key| remote.fetch(&key))
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
        drop(re_read_phase);
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
    tracing::info!("deploy: baseline written");
    if let Some(failure) = call_failure {
        Err(failure)
    } else {
        Ok(report)
    }
}
