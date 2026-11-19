use super::super::baseline::Baseline;
use super::super::entity::key::EntityKey;
use super::super::normalise;
use super::super::parallel;
use super::super::progress::{Progress, NONE};
use super::super::push;
use super::super::{adopt, patch};
use super::{
    DeployError, Entity, EntityPlan, PlanOptions, ProjectBundle, Remote, Script, ServiceCall,
};

/// What deploying each entity of the bundles would do: the working copy, the server's copy and the
/// baseline through the push decision table. Reads from the server; changes nothing.
pub(crate) fn decide_all(
    remote: &dyn Remote,
    baseline: &Baseline,
    projects: &[ProjectBundle],
) -> Result<Vec<EntityPlan>, DeployError> {
    decide_all_with_progress(remote, baseline, projects, &NONE)
}

/// Like [`decide_all`], and report one step per entity. The caller starts the phase.
pub(crate) fn decide_all_with_progress(
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
        let working =
            normalise::hash_with_parts(&entity.bytes, &entity.server_owned).map_err(|error| {
                DeployError::Working {
                    collection: entity.collection.clone(),
                    name: entity.name.clone(),
                    why: error.to_string(),
                }
            })?;
        let server_error = |why: String| DeployError::Server {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            why,
        };
        let fetched = EntityKey::address(&entity.collection, &entity.name)
            .and_then(|key| remote.fetch(&key))
            .map_err(|error| server_error(error.to_string()))?;
        let server = fetched
            .as_deref()
            .map(|bytes| normalise::hash_with_parts(bytes, &entity.server_owned))
            .transpose()
            .map_err(|error| server_error(error.to_string()))?;
        // The live copy supplies the definitions of encrypted fields.
        let encrypted = match &fetched {
            Some(bytes) => super::super::server_owned::encrypted(bytes, &entity.server_owned)
                .map_err(|error| server_error(error.to_string()))?,
            None => Vec::new(),
        };
        let (cleared, repository_values) = match &fetched {
            Some(bytes) => {
                super::super::server_owned::split_encrypted(&entity.bytes, bytes, &encrypted)
                    .map_err(|error| server_error(error.to_string()))?
            }
            None => (Vec::new(), Vec::new()),
        };
        let decision = push::decide(
            &working,
            server.as_deref(),
            baseline
                .get_for(&entity.collection, &entity.name, &working)
                .map(|entry| (entry.local.as_str(), entry.server.as_str())),
        );
        let hint = property_values_hint(entity, &working, fetched.as_deref(), baseline, &decision);
        let (warnings, removed_members) = if entity.collection == "Roles" {
            fetched
                .as_deref()
                .map(|bytes| {
                    let unchanged = baseline
                        .get_for(&entity.collection, &entity.name, &working)
                        .is_some_and(|entry| server.as_deref() == Some(entry.server.as_str()));
                    role_member_changes(entity, bytes, unchanged)
                })
                .transpose()
                .map_err(|error| server_error(error.to_string()))?
                .unwrap_or_default()
        } else {
            (Vec::new(), Vec::new())
        };
        Ok(EntityPlan {
            warnings,
            removed_members,
            hint,
            server_owned: if server.is_some() {
                super::super::server_owned::labels(&super::super::server_owned::carried(
                    &entity.server_owned,
                    &encrypted,
                ))
            } else {
                Vec::new()
            },
            cleared: super::super::server_owned::labels(&cleared),
            repository_values: super::super::server_owned::labels(&repository_values),
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
pub(crate) fn plan_bundles(
    solution: &super::config::Solution,
    profile: &str,
    options: PlanOptions,
) -> Result<(Vec<ProjectBundle>, Vec<String>), String> {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    let mut notes = Vec::new();
    if options.changed_since.is_some() && !options.only.is_empty() {
        return Err("--only cannot be combined with --changed-since".to_string());
    }
    let mut named = BTreeSet::new();
    for name in options.only_projects {
        if solution.project(name).is_none() {
            return Err(format!("this solution has no project named {name}"));
        }
        named.insert(name.as_str());
    }
    // A cycle among projects this deploy leaves out does not stop it.
    let order = if named.is_empty() {
        solution.deploy_order()
    } else {
        solution.deploy_order_of(&named)
    }
    .map_err(|error| format!("{error}"))?;
    let selected_projects: BTreeSet<&str> = if named.is_empty() {
        order.iter().map(|project| project.name.as_str()).collect()
    } else {
        named
    };

    let found = super::workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(found.unreadable.join("; "));
    }
    let unknown = solution.unknown_patches(profile, &found.entities);
    if !unknown.is_empty() {
        return Err(format!(
            "twaco.toml patches for profile {profile} name no Mashup in this workspace: {}",
            unknown.join(", ")
        ));
    }
    let backend_only = options.backend_only;
    let selection = if backend_only {
        super::bundle::Selection::backend(solution)
    } else {
        super::bundle::Selection::everything()
    };

    // `projectName` owns attribution. Folder membership is used only for the rare undeclared
    // entity, which workspace already reports as such.
    let pool: Vec<super::workspace::EntityFile> = found
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

    let (mut pool, selection_notes) = super::super::bundle::select_entities(
        solution,
        pool,
        options.changed_since,
        options.exclude,
    )?;
    notes.extend(selection_notes);

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

    let selected_paths: Vec<PathBuf> = pool.iter().map(|entity| entity.path.clone()).collect();
    notes.extend(super::super::bundle::dangling_references(
        &selected_paths,
        &super::super::bundle::source_files(solution),
    ));

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
        let mut built = match super::bundle::build(&files, &selection) {
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
            let original = match std::fs::read(&entity.path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Err(format!("{}: {error}", entity.path.display()));
                }
            };
            let bytes = patch::apply(
                &original,
                &entity.info.collection,
                &entity.info.name,
                solution.patches_for(profile),
            )
            .map_err(|error| format!("{}: {error}", entity.path.display()))?;
            if solution.patches_for(profile).iter().any(|patch| {
                patch.entity == format!("{}/{}", entity.info.collection, entity.info.name)
            }) {
                let wanted = adopt::EntityRef {
                    collection: entity.info.collection.clone(),
                    name: entity.info.name.clone(),
                };
                let element = adopt::raw_entity_bytes(&bytes, &wanted)
                    .map_err(|error| format!("{}: {error}", entity.path.display()))?;
                built.bytes = adopt::replace_entity_bytes(
                    &built.bytes,
                    &wanted,
                    &element,
                    solution.format.indent_cdata_payload,
                )
                .map_err(|error| format!("{}: {error}", entity.path.display()))?;
            }
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
                server_owned: super::super::server_owned::parts(
                    &solution.server_owned_for(profile),
                    &entity.info.collection,
                    &entity.info.name,
                ),
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
                .hook(&project.name)
                .map(|(thing, service)| ServiceCall {
                    target: format!("Things/{thing}"),
                    service,
                    parameters: toml_parameters(project.deploy.deploy_parameters.as_ref()),
                }),
            framework: project.deploy.framework,
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

pub(crate) fn toml_parameters(table: Option<&toml::Table>) -> serde_json::Value {
    table
        .map(|table| serde_json::to_value(table).expect("TOML tables serialize as JSON"))
        .unwrap_or_else(|| serde_json::json!({}))
}

fn property_values_hint(
    entity: &Entity,
    working: &str,
    fetched: Option<&[u8]>,
    baseline: &Baseline,
    decision: &push::Decision,
) -> Option<String> {
    if !matches!(
        decision,
        push::Decision::Refuse(push::Refusal::Conflict { .. })
    ) {
        return None;
    }
    let ancestor = baseline.get_for(&entity.collection, &entity.name, working)?;
    // Hash-only baselines can prove this only while the repository still is the common ancestor.
    if working != ancestor.local || ancestor.local != ancestor.server {
        return None;
    }
    let mut parts = entity.server_owned.clone();
    parts.push(super::super::server_owned::Part::Properties(
        Default::default(),
    ));
    let local = normalise::hash_with_parts(&entity.bytes, &parts).ok()?;
    let server = normalise::hash_with_parts(fetched?, &parts).ok()?;
    (local == server).then(|| {
        let name = serde_json::to_string(&format!("{}/{}", entity.collection, entity.name)).expect("entity name");
        format!("only property values differ; declare their server ownership in twaco.toml:\n[[server_owned]]\nentities = [{name}]\nparts = [\"property-values\"]")
    })
}

fn role_member_changes(
    entity: &Entity,
    server: &[u8],
    unchanged: bool,
) -> Result<(Vec<String>, Vec<String>), normalise::NormaliseError> {
    use super::super::server_owned::{self, Part};
    use std::collections::BTreeSet;
    if entity
        .server_owned
        .iter()
        .any(|p| matches!(p, Part::Members))
    {
        return Ok((Vec::new(), Vec::new()));
    }
    let members = |bytes: &[u8]| -> Result<BTreeSet<(String, String)>, normalise::NormaliseError> {
        let mut tree = normalise::entity_of(bytes)?;
        server_owned::omit(&mut tree, &entity.server_owned);
        let mut out = BTreeSet::new();
        for node in tree.children {
            let normalise::Node::Element(list) = node else {
                continue;
            };
            if list.name != b"Members" {
                continue;
            }
            for node in list.children {
                let normalise::Node::Element(row) = node else {
                    continue;
                };
                if row.name != b"Member" {
                    continue;
                }
                let attr = |key: &[u8]| {
                    row.attributes
                        .iter()
                        .find(|(k, _)| k == key)
                        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
                        .unwrap_or_default()
                };
                out.insert((attr(b"type"), attr(b"name")));
            }
        }
        Ok(out)
    };
    let local = members(&entity.bytes)?;
    let remote = members(server)?;
    let removed: Vec<_> = remote
        .difference(&local)
        .map(|(kind, name)| format!("{kind}/{name}"))
        .collect();
    Ok(if unchanged {
        (Vec::new(), removed)
    } else if removed.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        let key = serde_json::to_string(&format!("Roles/{}", entity.name)).expect("entity name");
        let mut types = BTreeSet::new();
        for part in &entity.server_owned {
            if let Part::MemberTypes(owned) = part {
                types.extend(owned.iter().cloned());
            }
        }
        let suggestion = if types.is_empty() {
            format!("keep run-time membership in twaco.toml:\n[[server_owned]]\nentities = [{key}]\nparts = [\"members\"]\nUse members:User,Group to keep only those types.")
        } else {
            types.extend(remote.difference(&local).map(|(kind, _)| kind.clone()));
            let types = ["User", "Group", "PermissionGroup"]
                .into_iter()
                .filter(|kind| types.contains(*kind))
                .collect::<Vec<_>>()
                .join(",");
            format!("extend the existing typed members policy in twaco.toml to keep run-time membership:\n[[server_owned]]\nentities = [{key}]\nparts = [\"members:{types}\"]")
        };
        (
            vec![format!(
                "Roles/{}: importing replaces Members and removes {}; {suggestion}",
                entity.name,
                removed.join(", ")
            )],
            Vec::new(),
        )
    })
}
