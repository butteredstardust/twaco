//! Planning an entity or prefix rename, and the dispatch to every other planner.

use super::*;

/// Computes every move, finding and byte edit for a rename without changing the workspace.
///
/// The plan covers every discovered entity, every source-sidecar file, `twaco.toml`, and every
/// other non-ignored text file up to 5 MiB. It refuses all [`RenameError`] conditions before
/// returning a plan; non-UTF-8 text candidates are recorded in [`Plan::skipped`].
pub fn plan(solution: &Solution, spec: &Spec) -> Result<Plan, RenameError> {
    if spec.kind == Kind::Param {
        return plan_param(solution, spec);
    }
    if spec.kind == Kind::Table {
        return plan_table(solution, spec);
    }
    if spec.kind == Kind::Property {
        return plan_property(solution, spec);
    }
    if spec.kind == Kind::Service {
        return plan_service(solution, spec);
    }
    if spec.kind == Kind::Field {
        return plan_field(solution, spec);
    }
    plan_identity(solution, spec)
}

pub(super) fn plan_identity(solution: &Solution, spec: &Spec) -> Result<Plan, RenameError> {
    validate_spec(spec)?;
    let mut discovery = workspace::discover(solution);
    if !discovery.unreadable.is_empty() {
        return Err(RenameError::Unreadable {
            files: discovery.unreadable,
        });
    }
    // Two projects may declare overlapping roots; one file is one entity however often it is found.
    discovery.entities.sort_by(|a, b| a.path.cmp(&b.path));
    discovery.entities.dedup_by(|a, b| a.path == b.path);

    let matching: Vec<&workspace::EntityFile> = discovery
        .entities
        .iter()
        .filter(|entity| {
            entity.info.name == spec.old
                || (spec.kind == Kind::Prefix
                    && entity.info.name.starts_with(&format!("{}.", spec.old)))
        })
        .collect();
    if matching.is_empty() {
        return Err(RenameError::Unknown {
            old: spec.old.clone(),
        });
    }
    if spec.kind == Kind::Entity && matching.len() > 1 {
        return Err(RenameError::Ambiguous {
            old: spec.old.clone(),
            files: matching.iter().map(|entity| entity.path.clone()).collect(),
        });
    }

    let mut moves = Vec::new();
    for entity in matching {
        let rest = &entity.info.name[spec.old.len()..];
        let new_name = format!("{}{rest}", spec.new);
        let extension = entity.path.extension().map(|value| value.to_os_string());
        let mut filename = std::ffi::OsString::from(&new_name);
        if let Some(extension) = extension {
            filename.push(".");
            filename.push(extension);
        }
        let old_sidecar_path = solution.src_root().join(&entity.info.name);
        let old_sidecars = old_sidecar_path.exists().then_some(old_sidecar_path);
        let new_sidecars = old_sidecars
            .as_ref()
            .map(|_| solution.src_root().join(&new_name));
        let old_repo_path = repo::local_root(
            &solution.root,
            solution.repositories.root.as_deref(),
            &entity.info.name,
        );
        let old_repo_files = old_repo_path.is_dir().then_some(old_repo_path);
        let new_repo_files = old_repo_files.as_ref().map(|_| {
            repo::local_root(
                &solution.root,
                solution.repositories.root.as_deref(),
                &new_name,
            )
        });
        moves.push(Move {
            collection: entity.info.collection.clone(),
            old_name: entity.info.name.clone(),
            new_name,
            old_file: entity.path.clone(),
            new_file: entity.path.with_file_name(filename),
            old_sidecars,
            new_sidecars,
            old_repo_files,
            new_repo_files,
        });
    }
    refuse_collisions(&discovery.entities, &moves)?;

    let mode = match spec.kind {
        Kind::Entity => refs::Mode::Entity,
        Kind::Prefix => refs::Mode::Prefix,
        Kind::Field => unreachable!("field renames have a structural pass"),
        Kind::Service => unreachable!("service renames have a structural pass"),
        Kind::Param => unreachable!("parameter renames have a structural pass"),
        Kind::Table => unreachable!("table renames have a structural pass"),
        Kind::Property => unreachable!("property renames have a structural pass"),
    };
    let mut changes = Vec::new();
    for entity in &discovery.entities {
        let bytes = read(&entity.path)?;
        let pass = rename_scan::scan_xml(&bytes, &spec.old, mode, &spec.new).map_err(|error| {
            RenameError::Xml {
                path: entity.path.clone(),
                why: error.to_string(),
            }
        })?;
        push_change(
            &mut changes,
            entity.path.clone(),
            FileKind::Entity,
            &bytes,
            pass,
        );
    }

    let entity_paths: BTreeSet<PathBuf> = discovery
        .entities
        .iter()
        .map(|entity| entity.path.clone())
        .collect();
    let config_path = solution.root.join(CONFIG_FILE);
    let mut covered = entity_paths.clone();
    covered.insert(config_path.clone());
    let mut skipped = Vec::new();
    scan_text_file(
        &config_path,
        FileKind::Config,
        spec,
        mode,
        &mut changes,
        &mut skipped,
    )?;

    // A sidecar is a file below an entity's own folder (`<src>/<entity>/`), wherever `src` points:
    // with `src = "."` everything below the root would otherwise count as owned, docs included,
    // and escape both the ignore rules and `--text`.
    let sidecar_roots: BTreeSet<PathBuf> = discovery
        .entities
        .iter()
        .map(|entity| solution.src_root().join(&entity.info.name))
        .collect();
    let moved_repo_roots: Vec<&Path> = moves
        .iter()
        .filter_map(|item| item.old_repo_files.as_deref())
        .collect();
    let mut outside = Vec::new();
    for path in check::walk_files(solution) {
        if covered.contains(&path) {
            continue;
        }
        if moved_repo_roots.iter().any(|root| path.starts_with(root)) {
            // Repository payloads are opaque moved content, not rename text, but remain in the
            // skipped total so adding the folder move does not change established plan counts.
            skipped.push(path);
            continue;
        }
        let metadata = std::fs::metadata(&path).map_err(|error| RenameError::Io {
            path: path.clone(),
            why: error.to_string(),
        })?;
        if metadata.len() > 5 * 1024 * 1024 {
            skipped.push(path);
            continue;
        }
        if path
            .ancestors()
            .skip(1)
            .any(|ancestor| sidecar_roots.contains(ancestor))
        {
            scan_text_file(
                &path,
                FileKind::Sidecar,
                spec,
                mode,
                &mut changes,
                &mut skipped,
            )?;
        } else {
            scan_text_file(
                &path,
                FileKind::Outside,
                spec,
                mode,
                &mut outside,
                &mut skipped,
            )?;
        }
    }
    skipped.sort();
    skipped.dedup();
    let moved_files: BTreeSet<PathBuf> = moves.iter().map(|item| item.old_file.clone()).collect();
    let moved_trees: Vec<&Path> = moves
        .iter()
        .flat_map(|item| {
            [item.old_sidecars.as_deref(), item.old_repo_files.as_deref()]
                .into_iter()
                .flatten()
        })
        .collect();
    let is_moved = |path: &Path| {
        moved_files.contains(path)
            || moved_trees
                .iter()
                .any(|root| path == *root || path.starts_with(root))
    };
    let mut named = check::walk_files(solution)
        .into_iter()
        .chain(check::walk_dirs(solution))
        .filter(|path| !is_moved(path))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !refs::find(name, &spec.old, mode).is_empty())
        })
        .collect::<Vec<_>>();
    named.sort();
    named.dedup();
    let baseline_keys = moves
        .iter()
        .map(|item| (item.collection.clone(), item.old_name.clone()))
        .collect();
    Ok(Plan {
        spec: spec.clone(),
        moves,
        changes,
        outside,
        baseline_keys,
        named,
        skipped,
        field_tables: 0,
        service_mashups: 0,
    })
}
