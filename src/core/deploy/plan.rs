use super::super::baseline::Baseline;
use super::super::entity_key::EntityKey;
use super::super::normalise;
use super::super::parallel;
use super::super::progress::{Progress, NONE};
use super::super::push;
use super::{
    DeployError, Entity, EntityPlan, PlanOptions, ProjectBundle, Remote, Script, ServiceCall,
};

/// What deploying each entity of the bundles would do: the working copy, the server's copy and the
/// baseline through the push decision table. Reads from the server; changes nothing.
pub fn decide_all(
    remote: &dyn Remote,
    baseline: &Baseline,
    projects: &[ProjectBundle],
) -> Result<Vec<EntityPlan>, DeployError> {
    decide_all_with_progress(remote, baseline, projects, &NONE)
}

/// Like [`decide_all`], and report one step per entity. The caller starts the phase.
pub fn decide_all_with_progress(
    remote: &dyn Remote,
    baseline: &Baseline,
    projects: &[ProjectBundle],
    progress: &dyn Progress,
) -> Result<Vec<EntityPlan>, DeployError> {
    let entities: Vec<(&ProjectBundle, &Entity)> = projects
        .iter()
        .flat_map(|project| project.entities.iter().map(move |entity| (project, entity)))
        .collect();
    let plans = parallel::map_progress(&entities, progress, |(project, entity)| {
        progress.message(&entity.name);
        let working = normalise::hash(&entity.bytes).map_err(|error| DeployError::Working {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            why: error.to_string(),
        })?;
        let server = EntityKey::address(&entity.collection, &entity.name)
            .and_then(|key| remote.fetch(&key))
            .map_err(|error| DeployError::Server {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                why: error.to_string(),
            })?
            .map(|bytes| normalise::hash(&bytes))
            .transpose()
            .map_err(|error| DeployError::Server {
                collection: entity.collection.clone(),
                name: entity.name.clone(),
                why: error.to_string(),
            })?;
        let decision = push::decide(
            &working,
            server.as_deref(),
            baseline
                .get(&entity.collection, &entity.name)
                .map(|entry| (entry.local.as_str(), entry.server.as_str())),
        );
        Ok(EntityPlan {
            project: project.project.clone(),
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            decision,
        })
    });
    plans.into_iter().collect()
}

/// The per-project bundles a deploy would import, in dependency order, each with the entities it
/// carries, the scripts to parse and the calls to make afterwards. Also a note per project.
/// Shared by the CLI and the MCP server, so both deploy exactly the same thing.
pub fn plan_bundles(
    solution: &super::config::Solution,
    options: PlanOptions,
) -> Result<(Vec<ProjectBundle>, Vec<String>), String> {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    let mut notes = Vec::new();
    let order = match solution.deploy_order() {
        Ok(order) => order,
        Err(error) => {
            return Err(format!("{error}"));
        }
    };
    let selected_projects: BTreeSet<&str> = if options.only_projects.is_empty() {
        order.iter().map(|project| project.name.as_str()).collect()
    } else {
        let mut selected = BTreeSet::new();
        for name in options.only_projects {
            if solution.project(name).is_none() {
                return Err(format!("this solution has no project named {name}"));
            }
            selected.insert(name.as_str());
        }
        selected
    };

    let found = super::workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(found.unreadable.join("; "));
    }
    let backend_only = options.backend_only;
    let selection = if backend_only {
        super::bundle::Selection::backend(solution)
    } else {
        super::bundle::Selection::everything()
    };

    // `projectName` owns attribution. Folder membership is used only for the rare undeclared
    // entity, which workspace already reports as such.
    let mut pool: Vec<super::workspace::EntityFile> = found
        .entities
        .into_iter()
        .filter(|entity| {
            let owner = if entity.info.project.is_empty() {
                entity.found_under.as_str()
            } else {
                entity.info.project.as_str()
            };
            selected_projects.contains(owner) && selection.wants(&entity.info.collection)
        })
        .collect();

    if !options.only.is_empty() {
        let mut narrowed = Vec::new();
        let mut seen = BTreeSet::new();
        for name in options.only {
            let entity = match super::workspace::resolve(&pool, name) {
                Ok(entity) => entity.clone(),
                Err(error) => {
                    return Err(format!("{error}"));
                }
            };
            if seen.insert(entity.path.clone()) {
                narrowed.push(entity);
            }
        }
        pool = narrowed;
    }
    if pool.is_empty() {
        return Err("the deploy selection contains no entities".to_string());
    }

    let source_order = super::bundle::source_files(solution);
    let mut projects = Vec::new();
    for project in order
        .into_iter()
        .filter(|project| selected_projects.contains(project.name.as_str()))
    {
        let mut chosen: Vec<super::workspace::EntityFile> = pool
            .iter()
            .filter(|entity| {
                if entity.info.project.is_empty() {
                    entity.found_under == project.name
                } else {
                    entity.info.project == project.name
                }
            })
            .cloned()
            .collect();
        if chosen.is_empty() {
            continue;
        }
        let wanted_paths: BTreeSet<PathBuf> =
            chosen.iter().map(|entity| entity.path.clone()).collect();
        let files: Vec<PathBuf> = source_order
            .iter()
            .filter(|path| wanted_paths.contains(*path))
            .cloned()
            .collect();
        let built = match super::bundle::build(&files, &selection) {
            Ok(built) => built,
            Err(error) => {
                return Err(format!("project {}: {error}", project.name));
            }
        };
        chosen.sort_by(|left, right| {
            (&left.info.collection, &left.info.name)
                .cmp(&(&right.info.collection, &right.info.name))
        });
        let mut entities = Vec::new();
        let mut scripts = Vec::new();
        for entity in chosen {
            let key = (entity.info.collection.clone(), entity.info.name.clone());
            if built.entities.get(&key) != Some(&1) {
                return Err(format!(
                    "project {} bundle does not contain exactly one {}/{}",
                    project.name, key.0, key.1
                ));
            }
            let bytes = match std::fs::read(&entity.path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Err(format!("{}: {error}", entity.path.display()));
                }
            };
            match super::sidecar::script_services(&bytes) {
                Ok(found_scripts) => {
                    scripts.extend(found_scripts.into_iter().map(|service| Script {
                        entity: entity.info.name.clone(),
                        service: service.name,
                        source: service.script,
                    }));
                }
                Err(error) => {
                    return Err(format!("{}: {error}", entity.path.display()));
                }
            }
            entities.push(Entity {
                collection: entity.info.collection,
                name: entity.info.name,
                bytes,
            });
        }
        notes.push(format!(
            "project {}: planned bundle has {} entities from {} file(s), {} script service(s)",
            project.name,
            built.entities.len(),
            built.files,
            scripts.len()
        ));
        projects.push(ProjectBundle {
            project: project.name.clone(),
            file_name: format!("{}.deploy.xml", project.name),
            bytes: built.bytes,
            entities,
            scripts,
            deploy: project
                .deploy
                .entry_point_thing
                .as_ref()
                .zip(project.deploy.deploy_service.as_ref())
                .map(|(thing, service)| ServiceCall {
                    target: format!("Things/{thing}"),
                    service: service.clone(),
                    parameters: toml_parameters(project.deploy.deploy_parameters.as_ref()),
                }),
            post_import: project
                .deploy
                .post_import
                .iter()
                .map(|call| ServiceCall {
                    target: call
                        .target
                        .clone()
                        .unwrap_or_else(|| format!("Things/{}", call.thing)),
                    service: call.service.clone(),
                    parameters: toml_parameters(call.parameters.as_ref()),
                })
                .collect(),
        });
    }
    if projects.is_empty() {
        return Err("the deploy selection contains no project bundle".to_string());
    }

    Ok((projects, notes))
}

pub fn toml_parameters(table: Option<&toml::Table>) -> serde_json::Value {
    table
        .map(|table| serde_json::to_value(table).expect("TOML tables serialize as JSON"))
        .unwrap_or_else(|| serde_json::json!({}))
}
