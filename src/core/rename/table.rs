//! Planning a configuration-table or property rename.

use super::*;

pub(super) fn plan_table(solution: &Solution, spec: &Spec) -> Result<Plan, RenameError> {
    validate_spec(spec)?;
    let scope = spec
        .scope
        .as_deref()
        .expect("table validation requires a scope");
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

    let (ancestry, model_skipped) = catalog::inheritance(solution);
    if !model_skipped.is_empty() {
        return Err(RenameError::Unreadable {
            files: model_skipped,
        });
    }
    let scope_model = ancestry
        .iter()
        .find(|item| item.name == scope && item.collection == scoped[0].info.collection)
        .ok_or_else(|| RenameError::TableScope {
            message: format!(
                "entity {scope} is not available in the inheritance catalog; fix its XML and retry"
            ),
        })?;
    let scope_bytes = read(&scoped[0].path)?;
    let scope_scan =
        rename_scan::scan_configuration_table(&scope_bytes, &spec.old, &spec.new, false, false)
            .map_err(|error| RenameError::Xml {
                path: scoped[0].path.clone(),
                why: error.to_string(),
            })?;
    if !scope_scan.old_definition {
        for ancestor in &scope_model.inherits {
            if let Some(item) = discovery
                .entities
                .iter()
                .find(|item| item.info.name == *ancestor)
            {
                let bytes = read(&item.path)?;
                let scan = rename_scan::scan_configuration_table(
                    &bytes, &spec.old, &spec.new, false, false,
                )
                .map_err(|error| RenameError::Xml {
                    path: item.path.clone(),
                    why: error.to_string(),
                })?;
                if scan.old_definition {
                    return Err(RenameError::TableScope {
                        message: format!(
                            "table {} on {scope} is declared on {ancestor}; rename it there",
                            spec.old
                        ),
                    });
                }
            }
        }
        return Err(RenameError::TableScope {
            message: format!(
                "entity {scope} declares no configuration table named {}; check the table name",
                spec.old
            ),
        });
    }

    // `inherits` includes every template and implemented shape reached through a Thing's
    // thingTemplate and the baseThingTemplate chain. Shape `implemented_by` supplies the same
    // transitive relation from the declaration side and covers direct implementers.
    let affected: BTreeSet<String> = ancestry
        .iter()
        .filter(|item| {
            item.name == scope
                || item.inherits.iter().any(|ancestor| ancestor == scope)
                || (scoped[0].info.collection == "ThingShapes"
                    && scope_model.implemented_by.contains(&item.name))
        })
        .map(|item| item.name.clone())
        .collect();
    // The services that read a table often live on a template's implemented shape, which is an
    // ancestor of the scope, not a descendant: the scripts of the whole family are in scope for
    // the `tableName: "..."` rewrite, while only the scope and its descendants have the table.
    let script_scope: BTreeSet<String> = affected
        .iter()
        .cloned()
        .chain(scope_model.inherits.iter().cloned())
        .collect();
    let mut changes = Vec::new();
    let mut conflicts = Vec::new();
    let mut tables = 0;
    for item in &discovery.entities {
        let bytes = read(&item.path)?;
        let structural = affected.contains(&item.info.name);
        let scanned = rename_scan::scan_configuration_table(
            &bytes,
            &spec.old,
            &spec.new,
            structural,
            script_scope.contains(&item.info.name),
        )
        .map_err(|error| RenameError::Xml {
            path: item.path.clone(),
            why: error.to_string(),
        })?;
        if structural && scanned.new_tables > 0 {
            conflicts.push(format!(
                "table {} on {}/{}",
                spec.new, item.info.collection, item.info.name
            ));
        }
        tables += scanned.renamed_tables;
        push_change(
            &mut changes,
            item.path.clone(),
            FileKind::Entity,
            &bytes,
            scanned.pass,
        );
    }
    if !conflicts.is_empty() {
        return Err(RenameError::Exists { conflicts });
    }
    for path in check::walk_files(solution)
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "script.js"))
    {
        let bytes = read(&path)?;
        let owner = path
            .strip_prefix(solution.src_root())
            .ok()
            .and_then(|relative| relative.components().next())
            .and_then(|part| part.as_os_str().to_str());
        let pass = rename_scan::scan_table_script(
            &bytes,
            &spec.old,
            &spec.new,
            owner.is_some_and(|name| script_scope.contains(name)),
        )
        .map_err(|error| RenameError::Io {
            path: path.clone(),
            why: error.to_string(),
        })?;
        push_change(&mut changes, path, FileKind::Sidecar, &bytes, pass);
    }
    Ok(Plan {
        spec: spec.clone(),
        moves: Vec::new(),
        changes,
        outside: Vec::new(),
        baseline_keys: Vec::new(),
        named: Vec::new(),
        skipped: Vec::new(),
        field_tables: tables,
        service_mashups: 0,
    })
}

/// The scope's own property, renamed in the scope and in everything that inherits it.
pub(super) fn plan_property(solution: &Solution, spec: &Spec) -> Result<Plan, RenameError> {
    validate_spec(spec)?;
    let scope = spec
        .scope
        .as_deref()
        .expect("property validation requires a scope");
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
    let (ancestry, model_skipped) = catalog::inheritance(solution);
    if !model_skipped.is_empty() {
        return Err(RenameError::Unreadable {
            files: model_skipped,
        });
    }
    let scope_model = ancestry
        .iter()
        .find(|item| item.name == scope && item.collection == scoped[0].info.collection)
        .ok_or_else(|| RenameError::ServiceScope {
            message: format!(
                "entity {scope} is not available in the inheritance catalog; fix its XML and retry"
            ),
        })?;
    let affected: BTreeSet<String> = ancestry
        .iter()
        .filter(|item| {
            item.name == scope
                || item.inherits.iter().any(|ancestor| ancestor == scope)
                || (scoped[0].info.collection == "ThingShapes"
                    && scope_model.implemented_by.contains(&item.name))
        })
        .map(|item| item.name.clone())
        .collect();
    // The scripts of an entity the scope inherits read `me.<name>` too: the family is in scope for scripts.
    let script_scope: BTreeSet<String> = affected
        .iter()
        .cloned()
        .chain(scope_model.inherits.iter().cloned())
        .collect();

    let mut bytes_by_path = std::collections::BTreeMap::new();
    for item in &discovery.entities {
        bytes_by_path.insert(item.path.clone(), read(&item.path)?);
    }
    let scan = |item: &workspace::EntityFile| {
        rename_property::scan_property_entity(
            &bytes_by_path[&item.path],
            &spec.old,
            &spec.new,
            &item.info.name,
            &affected,
            script_scope.contains(&item.info.name),
        )
        .map_err(|error| RenameError::Xml {
            path: item.path.clone(),
            why: error.to_string(),
        })
    };
    // The scope must declare the property; an inherited one is renamed where it is declared.
    if !scan(scoped[0])?.declared {
        for ancestor in &scope_model.inherits {
            if let Some(item) = discovery
                .entities
                .iter()
                .find(|item| item.info.name == *ancestor)
            {
                if scan(item)?.declared {
                    return Err(RenameError::ServiceScope {
                        message: format!(
                            "property {} on {scope} is declared on {ancestor}; rename it there",
                            spec.old
                        ),
                    });
                }
            }
        }
        return Err(RenameError::ServiceScope {
            message: format!(
                "entity {scope} declares no property named {}; check the property name",
                spec.old
            ),
        });
    }
    let mut changes = Vec::new();
    let mut conflicts = Vec::new();
    for item in &discovery.entities {
        let scanned = scan(item)?;
        if script_scope.contains(&item.info.name) && scanned.new_declared {
            conflicts.push(format!(
                "property {} on {}/{}",
                spec.new, item.info.collection, item.info.name
            ));
        }
        push_change(
            &mut changes,
            item.path.clone(),
            FileKind::Entity,
            &bytes_by_path[&item.path],
            scanned.pass,
        );
    }
    if !conflicts.is_empty() {
        return Err(RenameError::Exists { conflicts });
    }
    let sidecar_roots: BTreeSet<PathBuf> = discovery
        .entities
        .iter()
        .map(|entity| solution.src_root().join(&entity.info.name))
        .collect();
    for path in check::walk_files(solution) {
        let owner = path
            .ancestors()
            .skip(1)
            .find_map(|ancestor| {
                sidecar_roots.contains(ancestor).then(|| {
                    ancestor
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
            })
            .flatten();
        let Some(owner) = owner else { continue };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == "script.js" {
            let bytes = read(&path)?;
            let pass = rename_property::scan_property_script_file(
                &bytes,
                &spec.old,
                &spec.new,
                script_scope.contains(&owner),
                &affected,
            )
            .map_err(|error| RenameError::Io {
                path: path.clone(),
                why: error.to_string(),
            })?;
            push_change(&mut changes, path, FileKind::Sidecar, &bytes, pass);
        } else if name == "content.json" {
            // A mashup reads a property through its bindings; which one is which is a person's call.
            let bytes = read(&path)?;
            if let Ok(mentions) = rename_scan::review_mentions(&bytes, &spec.old) {
                push_change(&mut changes, path, FileKind::Sidecar, &bytes, mentions);
            }
        }
    }
    Ok(Plan {
        spec: spec.clone(),
        moves: Vec::new(),
        changes,
        outside: Vec::new(),
        baseline_keys: Vec::new(),
        named: Vec::new(),
        skipped: Vec::new(),
        field_tables: 0,
        service_mashups: 0,
    })
}
