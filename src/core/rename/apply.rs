//! The operation: digests, edits, moves, baseline and ledger, written as one journaled transaction.

use super::*;
use crate::core::lock::WorkspaceLock;
use crate::core::transaction::{Transaction, TransactionError};

struct PreparedFile {
    path: PathBuf,
    original: Vec<u8>,
    updated: Vec<u8>,
}

/// Applies a previously computed plan as one crash-recoverable workspace operation.
///
/// Every planned input and destination is verified before writing. Stale inputs, occupied
/// destinations and an invalid existing ledger are refused without changing the workspace. If
/// any later write, move, baseline update or ledger append fails, all completed work is undone;
/// and if the process dies part-way, the next command to take the workspace lock finishes or
/// undoes it (see `core::transaction`).
pub fn apply(
    solution: &Solution,
    plan: &Plan,
    options: &ApplyOptions,
    lock: &WorkspaceLock,
) -> Result<Applied, RenameError> {
    apply_with(solution, plan, options, lock, &mut |_| Ok(()))
}

/// Apply implementation with a hook immediately before every mutating step.
pub(crate) fn apply_with(
    solution: &Solution,
    plan: &Plan,
    options: &ApplyOptions,
    lock: &WorkspaceLock,
    hook: &mut dyn FnMut(&Step) -> std::io::Result<()>,
) -> Result<Applied, RenameError> {
    verify_destinations(plan, &options.extra_files)?;
    let ledger = solution.root.join(ledger::RELATIVE_PATH);
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

    // The steps, in the order they are made and with the boundary each one reports to the hook:
    // contents first (at their old paths), then the moves, the files the rename adds, the
    // baseline and the ledger.
    let apply_error = |path: &Path, why: String| RenameError::Apply {
        path: path.to_path_buf(),
        why,
    };
    let mut transaction = Transaction::new(&solution.root, "rename");
    let mut boundaries: Vec<Step> = Vec::new();
    for file in &prepared {
        let before = boundaries.len();
        transaction
            .replace_file(&file.path, &file.original, file.updated.clone())
            .map_err(|error| from_transaction(solution, error))?;
        if transaction.len() > before {
            boundaries.push(Step::Write(file.path.clone()));
        }
    }
    let mut moved = Vec::new();
    for item in &plan.moves {
        let pairs = [
            (item.new_file != item.old_file).then_some((&item.old_file, &item.new_file)),
            item.old_sidecars.as_ref().zip(item.new_sidecars.as_ref()),
            item.old_repo_files
                .as_ref()
                .zip(item.new_repo_files.as_ref()),
        ];
        for (old, new) in pairs.into_iter().flatten() {
            transaction
                .move_path(old, new)
                .map_err(|error| from_transaction(solution, error))?;
            boundaries.push(Step::Move(old.clone(), new.clone()));
            moved.push((old.clone(), new.clone()));
        }
    }
    for (path, bytes) in &options.extra_files {
        transaction
            .create_file(path, bytes.clone())
            .map_err(|error| from_transaction(solution, error))?;
        boundaries.push(Step::Write(path.clone()));
    }

    let baseline_path = solution.root.join(BASELINE_PATH);
    let mut baseline_removed = 0usize;
    let mut baseline = Baseline::load(&solution.root)
        .map_err(|error| apply_error(&baseline_path, error.to_string()))?;
    for (collection, name) in &plan.baseline_keys {
        baseline_removed += usize::from(baseline.remove(collection, name));
    }
    if baseline_removed > 0 {
        let bytes = baseline
            .to_bytes(&solution.root)
            .map_err(|error| apply_error(&baseline_path, error.to_string()))?;
        match read_optional(&baseline_path)? {
            Some(current) => transaction.replace_file(&baseline_path, &current, bytes),
            None => transaction.create_file(&baseline_path, bytes),
        }
        .map_err(|error| from_transaction(solution, error))?;
        boundaries.push(Step::Baseline);
    }

    let ledger_bytes = ledger_value
        .to_bytes(&ledger)
        .map_err(|error| match error {
            LedgerError::Invalid { path, why } | LedgerError::Write { path, why } => {
                RenameError::Apply { path, why }
            }
        })?;
    match read_optional(&ledger)? {
        Some(current) => transaction.replace_file(&ledger, &current, ledger_bytes),
        None => transaction.create_file(&ledger, ledger_bytes),
    }
    .map_err(|error| from_transaction(solution, error))?;
    boundaries.push(Step::Ledger);

    transaction
        .apply_with(lock, &mut |at, _| match boundaries.get(at) {
            Some(step) => hook(step),
            None => Ok(()),
        })
        .map_err(|error| from_transaction(solution, error))?;

    Ok(Applied {
        files_changed: prepared.len(),
        moved,
        baseline_removed,
        ledger,
    })
}

/// A transaction's failure as the rename's own error, naming files by their full paths.
fn from_transaction(solution: &Solution, error: TransactionError) -> RenameError {
    let path_of = |relative: &str| solution.root.join(relative);
    match error {
        TransactionError::Invalid(why) => RenameError::Apply {
            path: solution.root.clone(),
            why,
        },
        TransactionError::Stale(why) => match (
            why.strip_suffix(" changed since it was read"),
            why.strip_suffix(" exists already"),
        ) {
            (Some(path), _) => RenameError::Stale {
                path: path_of(path),
            },
            (_, Some(path)) => RenameError::Exists {
                conflicts: vec![path_of(path).display().to_string()],
            },
            _ => RenameError::Apply {
                path: solution.root.clone(),
                why,
            },
        },
        TransactionError::Io { path, why } => RenameError::Apply { path, why },
        TransactionError::Failed {
            why,
            rolled_back: true,
            ..
        } => RenameError::Apply {
            path: solution.root.clone(),
            why,
        },
        TransactionError::Failed {
            why,
            rolled_back: false,
            journal,
            leftover,
        } => RenameError::RollbackFailed {
            original: Box::new(RenameError::Apply {
                path: solution.root.clone(),
                why,
            }),
            leftover: leftover.into_iter().chain(journal).collect(),
        },
    }
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
