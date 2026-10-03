//! Planning a service or service-parameter rename.

use super::*;

pub(super) fn plan_service(solution: &Solution, spec: &Spec) -> Result<Plan, RenameError> {
    validate_spec(spec)?;
    let scope = spec
        .scope
        .as_deref()
        .expect("service validation requires a scope");
    let mut discovery = workspace::discover(solution);
    if !discovery.unreadable.is_empty() {
        return Err(RenameError::Unreadable {
            files: discovery.unreadable,
        });
    }
    discovery.entities.sort_by(|a, b| a.path.cmp(&b.path));
    discovery.entities.dedup_by(|a, b| a.path == b.path);
    let scoped: Vec<&workspace::EntityFile> = discovery
        .entities
        .iter()
        .filter(|item| {
            item.info.name == scope
                && matches!(
                    item.info.collection.as_str(),
                    "Things" | "ThingTemplates" | "ThingShapes"
                )
        })
        .collect();
    if scoped.len() != 1 {
        return Err(RenameError::Unknown {
            old: scope.to_string(),
        });
    }
    let catalog = catalog::build(solution, catalog::Query::default()).map_err(|error| {
        RenameError::ServiceScope {
            message: format!("cannot build the service catalog: {error}"),
        }
    })?;
    if !catalog.skipped.is_empty() {
        // An entity the model cannot read drops out of the callers and overrides silently.
        return Err(RenameError::Unreadable {
            files: catalog.skipped.clone(),
        });
    }
    let scope_catalog = catalog
        .entities
        .iter()
        .find(|item| item.name == scope && item.collection == scoped[0].info.collection)
        .ok_or_else(|| RenameError::ServiceScope {
            message: format!(
                "entity {scope} has no service named {}; check the name",
                spec.old
            ),
        })?;
    let service = scope_catalog
        .services
        .iter()
        .find(|service| service.name == spec.old)
        .ok_or_else(|| RenameError::ServiceScope {
            message: format!(
                "entity {scope} has no service named {}; check the name",
                spec.old
            ),
        })?;
    if service.from != "own" && service.from != scope {
        return Err(RenameError::ServiceScope {
            message: format!(
                "service {} on {scope} is declared on {}; rename it there",
                spec.old, service.from
            ),
        });
    }
    let mut callers = BTreeSet::from([scope.to_string()]);
    for item in &catalog.entities {
        if item.inherits.iter().any(|name| name == scope)
            || (scoped[0].info.collection == "ThingShapes"
                && scope_catalog.implemented_by.contains(&item.name))
        {
            callers.insert(item.name.clone());
        }
    }
    let mut affected = BTreeSet::from([scope.to_string()]);
    let mut bytes_by_name = std::collections::BTreeMap::new();
    for item in discovery
        .entities
        .iter()
        .filter(|item| callers.contains(&item.info.name))
    {
        let bytes = read(&item.path)?;
        let local = rename_scan::has_local_service(&bytes, &spec.old).map_err(|error| {
            RenameError::Xml {
                path: item.path.clone(),
                why: error.to_string(),
            }
        })?;
        if item.info.name != scope && local {
            affected.insert(item.info.name.clone());
        }
        bytes_by_name.insert(item.info.name.clone(), bytes);
    }
    let mut conflicts = BTreeSet::new();
    for name in &affected {
        let item = discovery
            .entities
            .iter()
            .find(|item| item.info.name == *name)
            .expect("affected entity was discovered");
        if rename_scan::has_local_service(
            bytes_by_name.get(name).expect("affected entity was read"),
            &spec.new,
        )
        .map_err(|error| RenameError::Xml {
            path: item.path.clone(),
            why: error.to_string(),
        })? {
            conflicts.insert(format!("service {} on {name}", spec.new));
        }
    }
    if let Some(ancestor) = scope_catalog
        .services
        .iter()
        .find(|service| service.name == spec.new && service.from != "own" && service.from != scope)
    {
        conflicts.insert(format!(
            "service {} declared on ancestor {}",
            spec.new, ancestor.from
        ));
    }
    for item in catalog
        .entities
        .iter()
        .filter(|item| item.name != scope && callers.contains(&item.name))
    {
        if item
            .services
            .iter()
            .any(|service| service.name == spec.new && service.from == "own")
        {
            conflicts.insert(format!(
                "service {} declared on descendant {}",
                spec.new, item.name
            ));
        }
    }
    if !conflicts.is_empty() {
        return Err(RenameError::Exists {
            conflicts: conflicts.into_iter().collect(),
        });
    }

    let mut moves = Vec::new();
    for name in &affected {
        let item = discovery
            .entities
            .iter()
            .find(|item| item.info.name == *name)
            .expect("affected entity was discovered");
        let old_folder = workspace::services_dir(solution, item).join(&spec.old);
        let new_folder = workspace::services_dir(solution, item).join(&spec.new);
        let has_folder = old_folder.is_dir();
        if new_folder.exists() {
            conflicts.insert(format!("sidecar directory {}", new_folder.display()));
        }
        moves.push(Move {
            collection: item.info.collection.clone(),
            old_name: item.info.name.clone(),
            new_name: item.info.name.clone(),
            old_file: item.path.clone(),
            new_file: item.path.clone(),
            old_sidecars: has_folder.then_some(old_folder),
            new_sidecars: has_folder.then_some(new_folder),
            old_repo_files: None,
            new_repo_files: None,
        });
    }
    if !conflicts.is_empty() {
        return Err(RenameError::Exists {
            conflicts: conflicts.into_iter().collect(),
        });
    }
    let mut changes = Vec::new();
    for item in &discovery.entities {
        let bytes = if let Some(bytes) = bytes_by_name.get(&item.info.name) {
            bytes.clone()
        } else {
            read(&item.path)?
        };
        let pass = if item.info.collection == "Mashups" {
            rename_scan::scan_service_mashup(&bytes, &spec.old, &spec.new, &callers, true).map_err(
                |why| RenameError::Xml {
                    path: item.path.clone(),
                    why,
                },
            )?
        } else {
            rename_scan::scan_service_entity(
                &bytes,
                &spec.old,
                &spec.new,
                affected.contains(&item.info.name),
                callers.contains(&item.info.name),
                &callers,
            )
            .map_err(|error| RenameError::Xml {
                path: item.path.clone(),
                why: error.to_string(),
            })?
        };
        let kind = if item.info.collection == "Mashups" {
            FileKind::Mashup
        } else {
            FileKind::Entity
        };
        push_change(&mut changes, item.path.clone(), kind, &bytes, pass);
    }
    let files = check::walk_files(solution);
    for path in files {
        let relative = path.strip_prefix(solution.src_root()).ok();
        let owner = relative
            .and_then(|path| path.components().next())
            .and_then(|part| part.as_os_str().to_str());
        if path.file_name().is_some_and(|name| name == "script.js") {
            let bytes = read(&path)?;
            let rename_function = owner.is_some_and(|name| affected.contains(name))
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == spec.old.as_str());
            let pass = rename_scan::scan_service_script(
                &bytes,
                &spec.old,
                &spec.new,
                owner.is_some_and(|name| callers.contains(name)),
                rename_function,
                &callers,
            )
            .map_err(|error| RenameError::Io {
                path: path.clone(),
                why: error.to_string(),
            })?;
            push_change(&mut changes, path, FileKind::Sidecar, &bytes, pass);
        } else if path
            .file_name()
            .is_some_and(|name| name == "definition.xml")
            && owner.is_some_and(|name| affected.contains(name))
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == spec.old.as_str())
        {
            let bytes = read(&path)?;
            let pass = rename_scan::scan_service_definition(&bytes, &spec.old, &spec.new).map_err(
                |error| RenameError::Xml {
                    path: path.clone(),
                    why: error.to_string(),
                },
            )?;
            push_change(&mut changes, path, FileKind::Sidecar, &bytes, pass);
        } else if path.file_name().is_some_and(|name| name == "content.json")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "mashup")
        {
            let bytes = read(&path)?;
            let pass =
                rename_scan::scan_service_mashup(&bytes, &spec.old, &spec.new, &callers, false)
                    .map_err(|why| RenameError::Io {
                        path: path.clone(),
                        why,
                    })?;
            push_change(&mut changes, path, FileKind::Mashup, &bytes, pass);
        }
    }
    let config_path = solution.root.join(CONFIG_FILE);
    let config_bytes = read(&config_path)?;
    let config_pass =
        rename_scan::scan_service_config(&config_bytes, &spec.old, &spec.new, &callers).map_err(
            |why| RenameError::Io {
                path: config_path.clone(),
                why,
            },
        )?;
    push_change(
        &mut changes,
        config_path,
        FileKind::Config,
        &config_bytes,
        config_pass,
    );
    let service_mashups = changes
        .iter()
        .filter(|change| change.kind == FileKind::Mashup)
        .count();
    Ok(Plan {
        spec: spec.clone(),
        moves,
        changes,
        outside: Vec::new(),
        baseline_keys: Vec::new(),
        named: Vec::new(),
        skipped: Vec::new(),
        field_tables: 0,
        service_mashups,
    })
}

pub(super) fn plan_param(solution: &Solution, spec: &Spec) -> Result<Plan, RenameError> {
    validate_spec(spec)?;
    let scope = spec
        .scope
        .as_deref()
        .expect("parameter validation requires a scope");
    let service_name = spec
        .service
        .as_deref()
        .expect("parameter validation requires a service");
    let mut discovery = workspace::discover(solution);
    if !discovery.unreadable.is_empty() {
        return Err(RenameError::Unreadable {
            files: discovery.unreadable,
        });
    }
    discovery.entities.sort_by(|a, b| a.path.cmp(&b.path));
    discovery.entities.dedup_by(|a, b| a.path == b.path);
    let scoped: Vec<&workspace::EntityFile> = discovery
        .entities
        .iter()
        .filter(|item| {
            item.info.name == scope
                && matches!(
                    item.info.collection.as_str(),
                    "Things" | "ThingTemplates" | "ThingShapes"
                )
        })
        .collect();
    if scoped.len() != 1 {
        return Err(RenameError::Unknown {
            old: scope.to_string(),
        });
    }
    let catalog = catalog::build(solution, catalog::Query::default()).map_err(|error| {
        RenameError::ServiceScope {
            message: format!("cannot build the service catalog: {error}"),
        }
    })?;
    if !catalog.skipped.is_empty() {
        // An entity the model cannot read drops out of the callers and overrides silently.
        return Err(RenameError::Unreadable {
            files: catalog.skipped.clone(),
        });
    }
    let scope_catalog = catalog
        .entities
        .iter()
        .find(|item| item.name == scope && item.collection == scoped[0].info.collection)
        .ok_or_else(|| RenameError::ServiceScope {
            message: format!("entity {scope} has no service named {service_name}; check the name"),
        })?;
    let service = scope_catalog
        .services
        .iter()
        .find(|item| item.name == service_name)
        .ok_or_else(|| RenameError::ServiceScope {
            message: format!("entity {scope} has no service named {service_name}; check the name"),
        })?;
    if service.from != "own" && service.from != scope {
        return Err(RenameError::ServiceScope {
            message: format!(
                "service {service_name} on {scope} is declared on {}; rename its parameter there",
                service.from
            ),
        });
    }
    let mut callers = BTreeSet::from([scope.to_string()]);
    for item in &catalog.entities {
        if item.inherits.iter().any(|name| name == scope)
            || (scoped[0].info.collection == "ThingShapes"
                && scope_catalog.implemented_by.contains(&item.name))
        {
            callers.insert(item.name.clone());
        }
    }
    let mut affected = BTreeSet::from([scope.to_string()]);
    let mut bytes_by_name = std::collections::BTreeMap::new();
    for item in discovery
        .entities
        .iter()
        .filter(|item| callers.contains(&item.info.name))
    {
        let bytes = read(&item.path)?;
        if item.info.name != scope
            && rename_scan::has_local_service(&bytes, service_name).map_err(|error| {
                RenameError::Xml {
                    path: item.path.clone(),
                    why: error.to_string(),
                }
            })?
        {
            affected.insert(item.info.name.clone());
        }
        bytes_by_name.insert(item.info.name.clone(), bytes);
    }
    let mut changes = Vec::new();
    let mut conflicts = Vec::new();
    for item in &discovery.entities {
        let bytes = if let Some(bytes) = bytes_by_name.get(&item.info.name) {
            bytes.clone()
        } else {
            read(&item.path)?
        };
        let pass = if item.info.collection == "Mashups" {
            rename_scan::scan_param_mashup(
                &bytes,
                service_name,
                &spec.old,
                &spec.new,
                &callers,
                true,
            )
            .map_err(|why| RenameError::Xml {
                path: item.path.clone(),
                why,
            })?
        } else {
            let scanned = rename_scan::scan_param_entity(
                &bytes,
                service_name,
                &spec.old,
                &spec.new,
                affected.contains(&item.info.name),
                callers.contains(&item.info.name),
                &callers,
            )
            .map_err(|error| RenameError::Xml {
                path: item.path.clone(),
                why: error.to_string(),
            })?;
            if affected.contains(&item.info.name) {
                if !scanned.old_found {
                    return Err(RenameError::ServiceScope { message: format!("service {service_name} on {} has no input named {}; add it or choose the existing input", item.info.name, spec.old) });
                }
                if scanned.new_found {
                    conflicts.push(format!(
                        "parameter {} on {}.{service_name}",
                        spec.new, item.info.name
                    ));
                }
            }
            scanned.pass
        };
        let kind = if item.info.collection == "Mashups" {
            FileKind::Mashup
        } else {
            FileKind::Entity
        };
        push_change(&mut changes, item.path.clone(), kind, &bytes, pass);
    }
    for path in check::walk_files(solution) {
        let relative = path.strip_prefix(solution.src_root()).ok();
        let owner = relative
            .and_then(|path| path.components().next())
            .and_then(|part| part.as_os_str().to_str());
        let selected_service = path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == service_name);
        if path.file_name().is_some_and(|name| name == "script.js") {
            let bytes = read(&path)?;
            if selected_service
                && owner.is_some_and(|name| affected.contains(name))
                && rename_scan::script_uses_identifier(&bytes, &spec.new)
            {
                // `result = value * 2` renamed from `value` to `result` would read `result = result * 2`.
                conflicts.push(format!(
                    "identifier {} is already used in the script of {}.{service_name}",
                    spec.new,
                    owner.unwrap_or_default()
                ));
            }
            let pass = rename_scan::scan_param_script(
                &bytes,
                service_name,
                &spec.old,
                &spec.new,
                selected_service && owner.is_some_and(|name| affected.contains(name)),
                owner.is_some_and(|name| callers.contains(name)),
                &callers,
            )
            .map_err(|error| RenameError::Io {
                path: path.clone(),
                why: error.to_string(),
            })?;
            push_change(&mut changes, path, FileKind::Sidecar, &bytes, pass);
        } else if path
            .file_name()
            .is_some_and(|name| name == "definition.xml")
            && selected_service
            && owner.is_some_and(|name| affected.contains(name))
        {
            let bytes = read(&path)?;
            let scanned =
                rename_scan::scan_param_definition(&bytes, service_name, &spec.old, &spec.new)
                    .map_err(|error| RenameError::Xml {
                        path: path.clone(),
                        why: error.to_string(),
                    })?;
            push_change(&mut changes, path, FileKind::Sidecar, &bytes, scanned.pass);
        } else if path.file_name().is_some_and(|name| name == "content.json")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "mashup")
        {
            let bytes = read(&path)?;
            let pass = rename_scan::scan_param_mashup(
                &bytes,
                service_name,
                &spec.old,
                &spec.new,
                &callers,
                false,
            )
            .map_err(|why| RenameError::Io {
                path: path.clone(),
                why,
            })?;
            push_change(&mut changes, path, FileKind::Mashup, &bytes, pass);
        }
    }
    let config_path = solution.root.join(CONFIG_FILE);
    let config_bytes = read(&config_path)?;
    let config =
        rename_scan::scan_param_config(&config_bytes, service_name, &spec.old, &spec.new, &callers)
            .map_err(|why| RenameError::Io {
                path: config_path.clone(),
                why,
            })?;
    conflicts.extend(config.conflicts);
    push_change(
        &mut changes,
        config_path,
        FileKind::Config,
        &config_bytes,
        config.pass,
    );
    if !conflicts.is_empty() {
        return Err(RenameError::Exists { conflicts });
    }
    let service_mashups = changes
        .iter()
        .filter(|change| change.kind == FileKind::Mashup)
        .count();
    Ok(Plan {
        spec: spec.clone(),
        moves: Vec::new(),
        changes,
        outside: Vec::new(),
        baseline_keys: Vec::new(),
        named: Vec::new(),
        skipped: Vec::new(),
        field_tables: 0,
        service_mashups,
    })
}
