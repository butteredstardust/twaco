//! Planning a DataShape field rename, with its configuration tables, DataTables and InfoTables.

use super::*;

pub(super) fn plan_field(solution: &Solution, spec: &Spec) -> Result<Plan, RenameError> {
    validate_spec(spec)?;
    let scope = spec
        .scope
        .as_deref()
        .expect("field validation requires a scope");
    let mut discovery = workspace::discover(solution);
    if !discovery.unreadable.is_empty() {
        return Err(RenameError::Unreadable {
            files: discovery.unreadable,
        });
    }
    discovery.entities.sort_by(|a, b| a.path.cmp(&b.path));
    discovery.entities.dedup_by(|a, b| a.path == b.path);
    let shapes: Vec<&workspace::EntityFile> = discovery
        .entities
        .iter()
        .filter(|item| item.info.name == scope && item.info.collection == "DataShapes")
        .collect();
    if shapes.len() != 1 {
        return Err(RenameError::Unknown {
            old: scope.to_string(),
        });
    }
    let shape = shapes[0];
    let shape_bytes = read(&shape.path)?;
    let shape_pass = rename_scan::scan_data_shape_field(&shape_bytes, &spec.old, &spec.new)
        .map_err(|error| RenameError::Xml {
            path: shape.path.clone(),
            why: error.to_string(),
        })?;
    if !shape_pass.old_found {
        return Err(RenameError::Unknown {
            old: format!("{} field {}", scope, spec.old),
        });
    }
    if shape_pass.new_found {
        return Err(RenameError::Exists {
            conflicts: vec![format!("field {} on DataShape {}", spec.new, scope)],
        });
    }
    let renamed_shape = splice::splice(&shape_bytes, &shape_pass.pass.edits).map_err(|error| {
        RenameError::Splice {
            path: shape.path.clone(),
            why: error.to_string(),
        }
    })?;
    let fields = datashape::extract(&renamed_shape).map_err(|error| RenameError::Xml {
        path: shape.path.clone(),
        why: error.to_string(),
    })?;
    let mut changes = Vec::new();
    push_change(
        &mut changes,
        shape.path.clone(),
        FileKind::Entity,
        &shape_bytes,
        shape_pass.pass,
    );

    let fields_path = workspace::fields_path(solution, shape);
    let sidecar_bytes = read(&fields_path)?;
    let sidecar = datashape::to_sidecar(&fields).into_bytes();
    push_change(
        &mut changes,
        fields_path,
        FileKind::Sidecar,
        &sidecar_bytes,
        rename_scan::replace_file(&sidecar_bytes, sidecar, "regenerated fields.json"),
    );

    let mut conflicts = Vec::new();
    let mut field_tables = 0;
    // A Thing's InfoTable value is typed by its template's property, so resolve declared property
    // types through the inheritance chain the catalog already knows.
    let (model, model_skipped) = types::load_model(solution);
    if !model_skipped.is_empty() {
        return Err(RenameError::Unreadable {
            files: model_skipped,
        });
    }
    let mut own_properties: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, String>,
    > = Default::default();
    let mut file_bytes: std::collections::BTreeMap<PathBuf, Vec<u8>> = Default::default();
    for item in &discovery.entities {
        let bytes = read(&item.path)?;
        if matches!(
            item.info.collection.as_str(),
            "Things" | "ThingTemplates" | "ThingShapes"
        ) {
            let shapes =
                rename_scan::property_shapes(&bytes).map_err(|error| RenameError::Xml {
                    path: item.path.clone(),
                    why: error.to_string(),
                })?;
            own_properties.insert(item.info.name.clone(), shapes);
        }
        file_bytes.insert(item.path.clone(), bytes);
    }
    let mut data_table_shapes = 0usize;
    let db = dbinfo::load(solution);
    for item in &discovery.entities {
        if !matches!(
            item.info.collection.as_str(),
            "Things" | "ThingTemplates" | "ThingShapes"
        ) {
            continue;
        }
        let bytes = &file_bytes[&item.path];
        let mut scanned = rename_scan::scan_configuration_field(bytes, scope, &spec.old, &spec.new)
            .map_err(|error| RenameError::Xml {
                path: item.path.clone(),
                why: error.to_string(),
            })?;
        field_tables += scanned.tables;
        for table in scanned.table_conflicts {
            conflicts.push(format!(
                "field {} in {}/{} table {}",
                spec.new, item.info.collection, item.info.name, table
            ));
        }
        // Properties of this entity, own or inherited, whose declared type is the shape.
        let ancestors: Vec<String> = model
            .entities
            .iter()
            .find(|entry| entry.name == item.info.name && entry.collection == item.info.collection)
            .map(|entry| catalog::inheritance_names(entry, &model.entities))
            .unwrap_or_default();
        let mut typed = BTreeSet::new();
        for owner in
            std::iter::once(item.info.name.as_str()).chain(ancestors.iter().map(String::as_str))
        {
            if let Some(shapes) = own_properties.get(owner) {
                typed.extend(
                    shapes
                        .iter()
                        .filter(|(_, shape)| shape.as_str() == scope)
                        .map(|(name, _)| name.clone()),
                );
            }
        }
        if !typed.is_empty() {
            let info = rename_scan::scan_infotable_field(bytes, &typed, &spec.old, &spec.new)
                .map_err(|error| RenameError::Xml {
                    path: item.path.clone(),
                    why: error.to_string(),
                })?;
            field_tables += info.tables;
            for table in info.table_conflicts {
                conflicts.push(format!(
                    "field {} in {}/{} property {}",
                    spec.new, item.info.collection, item.info.name, table
                ));
            }
            scanned.pass.edits.extend(info.pass.edits);
            scanned.pass.findings.extend(info.pass.findings);
            scanned.pass.edits.sort_by_key(|edit| edit.span.start);
        }
        // A DBConnection shape is also named in the manager's GetDBInfo: its fields, indexes and foreign
        // keys, and the foreign keys of other tables that point at it.
        if db.is_backed(scope) {
            for (base, scan) in dbinfo::scan_entity(bytes).map_err(|error| RenameError::Xml {
                path: item.path.clone(),
                why: error.to_string(),
            })? {
                for span in dbinfo::field_spans(&scan, scope, &spec.old) {
                    let absolute = crate::core::scan::Span::new(base + span.start, base + span.end);
                    rename_scan::add_field_edit(
                        bytes,
                        absolute,
                        &spec.new,
                        rename_scan::Place::File,
                        &mut scanned.pass,
                    );
                }
            }
            scanned.pass.edits.sort_by_key(|edit| edit.span.start);
        }
        // A DataTable keeps its shape and indexes in its own configuration, not in a table of the
        // renamed shape: rename them through the module that reads and writes them.
        if item.info.collection == "Things" && datatable::is_data_table(bytes) {
            let configuration = datatable::extract(bytes).map_err(|error| RenameError::Xml {
                path: item.path.clone(),
                why: error.to_string(),
            })?;
            if configuration.data_shape == scope {
                if let Some(renamed) = rename_data_table(&configuration, &spec.old, &spec.new)
                    .map_err(|why| RenameError::Xml {
                        path: item.path.clone(),
                        why,
                    })?
                {
                    if !scanned.pass.edits.is_empty() {
                        return Err(RenameError::Xml {
                            path: item.path.clone(),
                            why:
                                "a DataTable that also carries tables of the shape is not supported"
                                    .to_string(),
                        });
                    }
                    let (updated, _) =
                        datatable::sync(bytes, &renamed).map_err(|error| RenameError::Xml {
                            path: item.path.clone(),
                            why: error.to_string(),
                        })?;
                    let sidecar_path = workspace::datatable_path(solution, item);
                    let sidecar =
                        datatable::to_sidecar(&datatable::extract(&updated).map_err(|error| {
                            RenameError::Xml {
                                path: item.path.clone(),
                                why: error.to_string(),
                            }
                        })?)
                        .map_err(|error| RenameError::Xml {
                            path: item.path.clone(),
                            why: error.to_string(),
                        })?
                        .into_bytes();
                    push_change(
                        &mut changes,
                        item.path.clone(),
                        FileKind::Entity,
                        bytes,
                        rename_scan::replace_file(
                            bytes,
                            updated,
                            "renamed the DataTable's shape and indexes",
                        ),
                    );
                    if sidecar_path.is_file() {
                        let current = read(&sidecar_path)?;
                        push_change(
                            &mut changes,
                            sidecar_path,
                            FileKind::Sidecar,
                            &current,
                            rename_scan::replace_file(
                                &current,
                                sidecar,
                                "regenerated datatable.json",
                            ),
                        );
                    }
                    data_table_shapes += 1;
                    continue;
                }
            }
        }
        push_change(
            &mut changes,
            item.path.clone(),
            FileKind::Entity,
            bytes,
            scanned.pass,
        );
    }
    if !conflicts.is_empty() {
        return Err(RenameError::Exists { conflicts });
    }
    let _ = data_table_shapes;
    if db.is_backed(scope) {
        for path in check::walk_files(solution) {
            let is_db_info = path.file_name().is_some_and(|name| name == "script.js")
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == "GetDBInfo");
            if !is_db_info {
                continue;
            }
            let bytes = read(&path)?;
            let mut pass = rename_scan::XmlPass::new_for(&bytes);
            for span in dbinfo::field_spans(&dbinfo::scan_script(&bytes), scope, &spec.old) {
                rename_scan::add_field_edit(
                    &bytes,
                    span,
                    &spec.new,
                    rename_scan::Place::File,
                    &mut pass,
                );
            }
            push_change(&mut changes, path, FileKind::Sidecar, &bytes, pass);
        }
    }
    // Every other mention of the old name in a script or a mashup is a person's to judge: list them.
    let sidecar_roots: BTreeSet<PathBuf> = discovery
        .entities
        .iter()
        .map(|entity| solution.src_root().join(&entity.info.name))
        .collect();
    for path in check::walk_files(solution) {
        let reviewable = path
            .extension()
            .is_some_and(|ext| ext == "js" || ext == "json")
            && path
                .ancestors()
                .skip(1)
                .any(|ancestor| sidecar_roots.contains(ancestor))
            && path
                .file_name()
                .is_some_and(|name| name != "fields.json" && name != "datatable.json");
        if !reviewable {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(mentions) = rename_scan::review_mentions(&bytes, &spec.old) else {
            continue;
        };
        push_change(&mut changes, path, FileKind::Sidecar, &bytes, mentions);
    }
    Ok(Plan {
        spec: spec.clone(),
        moves: Vec::new(),
        changes,
        outside: Vec::new(),
        baseline_keys: Vec::new(),
        named: Vec::new(),
        skipped: Vec::new(),
        field_tables,
        service_mashups: 0,
    })
}

/// The DataTable's configuration with `old` renamed to `new` in its accumulated shape and its
/// index field lists, or `None` when it mentions neither. An accumulated shape that does not
/// survive a parse and a compact write unchanged is refused rather than rewritten.
pub(super) fn rename_data_table(
    configuration: &datatable::Configuration,
    old: &str,
    new: &str,
) -> Result<Option<datatable::Configuration>, String> {
    let mut renamed = configuration.clone();
    let mut changed = false;
    let text = configuration.accumulated.trim();
    if !text.is_empty() {
        let mut value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| format!("the accumulated DataShape is not JSON: {error}"))?;
        if serde_json::to_string(&value).map_err(|error| error.to_string())? != text {
            return Err(
                "the accumulated DataShape is not in the compact form twaco can rewrite exactly"
                    .to_string(),
            );
        }
        if let Some(definitions) = value
            .get_mut("fieldDefinitions")
            .and_then(serde_json::Value::as_object_mut)
        {
            if definitions.contains_key(new) {
                return Err(format!("the DataTable already has a field named {new}"));
            }
            if definitions.contains_key(old) {
                let rebuilt: serde_json::Map<String, serde_json::Value> =
                    std::mem::take(definitions)
                        .into_iter()
                        .map(|(key, mut field)| {
                            if key == old {
                                if let Some(name) = field.get_mut("name") {
                                    *name = serde_json::Value::String(new.to_string());
                                }
                                (new.to_string(), field)
                            } else {
                                (key, field)
                            }
                        })
                        .collect();
                *definitions = rebuilt;
                renamed.accumulated =
                    serde_json::to_string(&value).map_err(|error| error.to_string())?;
                changed = true;
            }
        }
    }
    for index in &mut renamed.indexes {
        let parts: Vec<&str> = index.field_names.split(',').collect();
        if parts.iter().any(|part| part.trim() == old) {
            index.field_names = parts
                .iter()
                .map(|part| if part.trim() == old { new } else { *part })
                .collect::<Vec<_>>()
                .join(",");
            changed = true;
        }
    }
    Ok(changed.then_some(renamed))
}
