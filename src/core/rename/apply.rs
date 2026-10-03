//! The transaction: digests, temporaries, moves, rollback, baseline and ledger.

use super::*;

struct PreparedFile {
    path: PathBuf,
    original: Vec<u8>,
    updated: Vec<u8>,
}

/// Applies a previously computed plan as one rollback-protected workspace operation.
///
/// Every planned input and destination is verified before writing. Stale inputs, occupied
/// destinations and an invalid existing ledger are refused without changing the workspace. If
/// any later write, move, baseline update or ledger append fails, all completed work is undone.
pub fn apply(
    solution: &Solution,
    plan: &Plan,
    options: &ApplyOptions,
) -> Result<Applied, RenameError> {
    apply_with(solution, plan, options, &mut |_| Ok(()))
}

/// Apply implementation with a hook immediately before every mutating operation.
pub(crate) fn apply_with(
    solution: &Solution,
    plan: &Plan,
    options: &ApplyOptions,
    hook: &mut dyn FnMut(&Step) -> std::io::Result<()>,
) -> Result<Applied, RenameError> {
    verify_destinations(plan, &options.extra_files)?;
    let ledger = solution.root.join(ledger::RELATIVE_PATH);
    let ledger_parent_existed = ledger.parent().expect("ledger has a parent").exists();
    let mut ledger_value = Ledger::read(&ledger).map_err(|error| match error {
        LedgerError::Invalid { path, why } | LedgerError::Write { path, why } => {
            RenameError::InvalidLedger { path, why }
        }
    })?;
    let selected = plan.changes.iter().chain(
        options
            .include_outside
            .then_some(plan.outside.iter())
            .into_iter()
            .flatten(),
    );
    let mut verified = Vec::new();
    // The entities the rename edited, named from the bytes just verified rather than read again.
    let mut edited_entities = std::collections::BTreeMap::new();
    for change in selected {
        let original = std::fs::read(&change.path).map_err(|error| RenameError::Apply {
            path: change.path.clone(),
            why: error.to_string(),
        })?;
        if <[u8; 32]>::from(Sha256::digest(&original)) != change.digest {
            return Err(RenameError::Stale {
                path: change.path.clone(),
            });
        }
        if change.kind == FileKind::Entity && !change.edits.is_empty() {
            if let Ok(info) = entity::parse(&original) {
                edited_entities.insert(change.path.clone(), (info.collection, info.name));
            }
        }
        verified.push((change, original));
    }

    let mut prepared = Vec::new();
    for (change, original) in verified {
        let updated =
            splice::splice(&original, &change.edits).map_err(|error| RenameError::Splice {
                path: change.path.clone(),
                why: error.to_string(),
            })?;
        if updated != original {
            prepared.push(PreparedFile {
                path: change.path.clone(),
                original,
                updated,
            });
        }
    }

    ledger_value
        .0
        .push(ledger_record(plan, options, &edited_entities));

    let baseline_path = solution.root.join(BASELINE_PATH);
    let mut completed_moves = Vec::new();
    let mut written = 0usize;
    let mut baseline_original = None;
    let mut baseline_started = false;
    let mut baseline_removed = 0usize;
    let mut created_files: Vec<PathBuf> = Vec::new();
    let mut created_dir: Option<PathBuf> = None;
    let result = (|| {
        for file in &prepared {
            call_hook(hook, Step::Write(file.path.clone()), &file.path)?;
            // The digest check at the start can be minutes old by now (a long list of files, an
            // editor open on one of them): look again right before replacing the file.
            if std::fs::read(&file.path).ok().as_deref() != Some(file.original.as_slice()) {
                return Err(RenameError::Stale {
                    path: file.path.clone(),
                });
            }
            atomic_write(&file.path, &file.updated)?;
            written += 1;
        }
        for item in &plan.moves {
            if item.new_file != item.old_file {
                move_one(hook, &item.old_file, &item.new_file, &mut completed_moves)?;
            }
            if let (Some(old), Some(new)) = (&item.old_sidecars, &item.new_sidecars) {
                move_one(hook, old, new, &mut completed_moves)?;
            }
            if let (Some(old), Some(new)) = (&item.old_repo_files, &item.new_repo_files) {
                move_one(hook, old, new, &mut completed_moves)?;
            }
        }

        for (path, bytes) in &options.extra_files {
            call_hook(hook, Step::Write(path.clone()), path)?;
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(|error| RenameError::Apply {
                        path: parent.to_path_buf(),
                        why: error.to_string(),
                    })?;
                    created_dir.get_or_insert_with(|| parent.to_path_buf());
                }
            }
            // `create_new`: a file that appeared since the check is not ours to overwrite.
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|error| RenameError::Apply {
                    path: path.clone(),
                    why: error.to_string(),
                })?;
            created_files.push(path.clone());
            file.write_all(bytes).map_err(|error| RenameError::Apply {
                path: path.clone(),
                why: error.to_string(),
            })?;
        }

        call_hook(hook, Step::Baseline, &baseline_path)?;
        baseline_original = read_optional(&baseline_path)?;
        baseline_started = true;
        let mut baseline = Baseline::load(&solution.root).map_err(|error| RenameError::Apply {
            path: baseline_path.clone(),
            why: error.to_string(),
        })?;
        for (collection, name) in &plan.baseline_keys {
            baseline_removed += usize::from(baseline.remove(collection, name));
        }
        if baseline_removed > 0 {
            baseline
                .write(&solution.root)
                .map_err(|error| RenameError::Apply {
                    path: baseline_path.clone(),
                    why: error.to_string(),
                })?;
        }

        call_hook(hook, Step::Ledger, &ledger)?;
        ledger_value.write(&ledger).map_err(|error| match error {
            LedgerError::Invalid { path, why } | LedgerError::Write { path, why } => {
                RenameError::Apply { path, why }
            }
        })?;
        Ok(())
    })();

    if let Err(error) = result {
        let leftovers = rollback(
            &prepared[..written],
            &completed_moves,
            &baseline_path,
            baseline_started,
            baseline_original.as_deref(),
            &ledger,
            ledger_parent_existed,
            &created_files,
            created_dir.as_deref(),
        );
        return if leftovers.is_empty() {
            Err(error)
        } else {
            Err(RenameError::RollbackFailed {
                original: Box::new(error),
                leftover: leftovers,
            })
        };
    }

    Ok(Applied {
        files_changed: written,
        moved: completed_moves,
        baseline_removed,
        ledger,
    })
}

pub(super) fn verify_destinations(
    plan: &Plan,
    extra_files: &[(PathBuf, Vec<u8>)],
) -> Result<(), RenameError> {
    let mut conflicts = Vec::new();
    for (path, _) in extra_files {
        if path.exists() {
            conflicts.push(format!("file {}", path.display()));
        }
    }
    for item in &plan.moves {
        // An entity already filed under its new name stays where it is.
        if item.new_file != item.old_file && item.new_file.exists() {
            conflicts.push(format!("file {}", item.new_file.display()));
        }
        if let Some(path) = &item.new_sidecars {
            if path.exists() {
                conflicts.push(format!("sidecar directory {}", path.display()));
            }
        }
        if let Some(path) = &item.new_repo_files {
            if path.exists() {
                conflicts.push(format!("repository directory {}", path.display()));
            }
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(RenameError::Exists { conflicts })
    }
}

/// The ledger record of a rename. Entity lists are computed from the plan and the bytes verified at
/// the start of the apply, so nothing is read from the workspace again to write it.
pub(super) fn ledger_record(
    plan: &Plan,
    options: &ApplyOptions,
    edited: &std::collections::BTreeMap<PathBuf, (String, String)>,
) -> ledger::Record {
    let entities = if matches!(plan.spec.kind, Kind::Field | Kind::Table | Kind::Property) {
        plan.changes
            .iter()
            .filter_map(|change| edited.get(&change.path))
            .map(|(collection, name)| ledger::Entity::new(collection, name, name))
            .collect()
    } else {
        plan.moves
            .iter()
            .map(|item| ledger::Entity::new(&item.collection, &item.old_name, &item.new_name))
            .collect()
    };
    ledger::Record {
        date: options.date.clone(),
        kind: match plan.spec.kind {
            Kind::Entity => ledger::Kind::Entity,
            Kind::Prefix => ledger::Kind::Prefix,
            Kind::Field => ledger::Kind::Field,
            Kind::Service => ledger::Kind::Service,
            Kind::Param => ledger::Kind::Param,
            Kind::Table => ledger::Kind::Table,
            Kind::Property => ledger::Kind::Property,
        },
        old: plan.spec.old.clone(),
        new: plan.spec.new.clone(),
        scope: plan.spec.scope.clone(),
        service: plan.spec.service.clone(),
        entities,
        other: serde_json::Map::new(),
    }
}

pub(super) fn call_hook(
    hook: &mut dyn FnMut(&Step) -> std::io::Result<()>,
    step: Step,
    path: &Path,
) -> Result<(), RenameError> {
    hook(&step).map_err(|error| RenameError::Apply {
        path: path.to_path_buf(),
        why: error.to_string(),
    })
}

pub(super) fn move_one(
    hook: &mut dyn FnMut(&Step) -> std::io::Result<()>,
    old: &Path,
    new: &Path,
    completed: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), RenameError> {
    call_hook(hook, Step::Move(old.to_path_buf(), new.to_path_buf()), old)?;
    // `rename` replaces a file that is already there on some platforms; never let it.
    if new.exists() {
        return Err(RenameError::Exists {
            conflicts: vec![format!("{}", new.display())],
        });
    }
    std::fs::rename(old, new).map_err(|error| RenameError::Apply {
        path: old.to_path_buf(),
        why: format!("cannot move to {}: {error}", new.display()),
    })?;
    completed.push((old.to_path_buf(), new.to_path_buf()));
    Ok(())
}

/// Replace a file through `workspace::atomic_replace`, naming the file on failure.
pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RenameError> {
    crate::core::workspace::atomic_replace(path, bytes).map_err(|error| RenameError::Apply {
        path: path.to_path_buf(),
        why: error.to_string(),
    })
}

pub(super) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, RenameError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RenameError::Apply {
            path: path.to_path_buf(),
            why: error.to_string(),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn rollback(
    prepared: &[PreparedFile],
    moves: &[(PathBuf, PathBuf)],
    baseline_path: &Path,
    baseline_started: bool,
    baseline_original: Option<&[u8]>,
    ledger: &Path,
    ledger_parent_existed: bool,
    created_files: &[PathBuf],
    created_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut leftover = Vec::new();
    // The files this rename added (the migration) are the last thing it created, so they go first.
    for path in created_files.iter().rev() {
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                leftover.push(path.clone());
            }
        }
    }
    if let Some(dir) = created_dir {
        let _ = std::fs::remove_dir(dir);
    }
    // Undo in the reverse of the order of work: baseline, then moves, then file contents. The
    // contents live at their old paths, so the moves must be back before they are restored.
    if baseline_started {
        let restored = match baseline_original {
            Some(bytes) => atomic_write(baseline_path, bytes),
            None => match std::fs::remove_file(baseline_path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(RenameError::Apply {
                    path: baseline_path.to_path_buf(),
                    why: error.to_string(),
                }),
            },
        };
        if restored.is_err() {
            leftover.push(baseline_path.to_path_buf());
        }
    }
    for (old, new) in moves.iter().rev() {
        if std::fs::rename(new, old).is_err() {
            leftover.push(new.clone());
            leftover.push(old.clone());
        }
    }
    for file in prepared.iter().rev() {
        // Someone saved this file after we wrote it: their version is not ours to overwrite.
        if std::fs::read(&file.path).ok().as_deref() != Some(file.updated.as_slice()) {
            leftover.push(file.path.clone());
            continue;
        }
        if atomic_write(&file.path, &file.original).is_err() {
            leftover.push(file.path.clone());
        }
    }
    if !ledger_parent_existed {
        let parent = ledger.parent().expect("ledger has a parent");
        if let Err(error) = std::fs::remove_dir(parent) {
            if error.kind() != std::io::ErrorKind::NotFound {
                leftover.push(parent.to_path_buf());
            }
        }
    }
    leftover.sort();
    leftover.dedup();
    leftover
}
