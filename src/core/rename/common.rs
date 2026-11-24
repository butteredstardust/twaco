//! What every planner shares: spec validation, collision refusal, reading and scanning files.

use super::{
    refs, scan, workspace, BTreeSet, Digest, FileChange, FileKind, Kind, Move, Path, PathBuf,
    RenameError, Sha256, Spec,
};

pub(super) fn validate_spec(spec: &Spec) -> Result<(), RenameError> {
    if spec.kind.is_member() {
        let member = spec.kind.noun();
        refs::validate_field_name(&spec.new, member)
            .map_err(|message| RenameError::InvalidNew { message })?;
        refs::validate_field_name(&spec.old, member)
            .map_err(|message| RenameError::InvalidOld { message })?;
        if spec.scope.as_deref().is_none_or(str::is_empty) {
            let noun = if spec.kind == Kind::Field {
                "field rename requires a DataShape"
            } else if matches!(spec.kind, Kind::Service | Kind::Param) {
                "service member rename requires an entity"
            } else {
                "table rename requires an entity"
            };
            return Err(RenameError::InvalidOld {
                message: format!("a {noun} scope"),
            });
        }
        if spec.kind == Kind::Param {
            refs::validate_param_name(&spec.new)
                .map_err(|message| RenameError::InvalidNew { message })?;
            let service = spec.service.as_deref().unwrap_or_default();
            refs::validate_field_name(service, "service")
                .map_err(|message| RenameError::InvalidOld { message })?;
        } else if spec.service.is_some() {
            return Err(RenameError::InvalidOld {
                message: "service is only valid for a parameter rename".to_string(),
            });
        }
        if spec.old == spec.new {
            return Err(RenameError::Same {
                name: spec.old.clone(),
            });
        }
        return Ok(());
    }
    if spec.scope.is_some() || spec.service.is_some() {
        return Err(RenameError::InvalidOld {
            message: "scope is only valid for a field, service or table rename".to_string(),
        });
    }
    let validate = if spec.kind == Kind::Entity {
        refs::validate_entity_name
    } else {
        refs::validate_new_name
    };
    validate(&spec.new).map_err(|message| RenameError::InvalidNew { message })?;
    validate(&spec.old).map_err(|message| RenameError::InvalidOld {
        message: message.replacen("new name", "old name", 1),
    })?;
    if spec.old == spec.new {
        return Err(RenameError::Same {
            name: spec.old.clone(),
        });
    }
    if spec.new.starts_with(&format!("{}.", spec.old))
        || spec.old.starts_with(&format!("{}.", spec.new))
    {
        return Err(RenameError::Nested {
            old: spec.old.clone(),
            new: spec.new.clone(),
        });
    }
    Ok(())
}

/// Refuse an entity rename when another entity's name is this one joined by a space to more
/// name (`Acme.Foo Bar` for `Acme.Foo`): a text reference to that entity would contain this name
/// as a whole token, and the rename would rewrite part of it.
pub(super) fn refuse_spaced_supersets(
    entities: &[workspace::EntityFile],
    old: &str,
) -> Result<(), RenameError> {
    let mut longer: Vec<&str> = entities
        .iter()
        .map(|entity| entity.info.name.as_str())
        .filter(|name| {
            name.len() > old.len()
                && (name
                    .strip_prefix(old)
                    .is_some_and(|rest| rest.starts_with(' '))
                    || name
                        .strip_suffix(old)
                        .is_some_and(|rest| rest.ends_with(' ')))
        })
        .collect();
    if longer.is_empty() {
        return Ok(());
    }
    longer.sort_unstable();
    longer.dedup();
    Err(RenameError::InvalidOld {
        message: format!(
            "{old} is part of the longer entity name {}; references to it would be rewritten too. Rename that entity first",
            longer.join(", ")
        ),
    })
}

pub(super) fn refuse_collisions(
    entities: &[workspace::EntityFile],
    moves: &[Move],
) -> Result<(), RenameError> {
    let moved: BTreeSet<PathBuf> = moves.iter().map(|item| item.old_file.clone()).collect();
    let mut conflicts = BTreeSet::new();
    for item in moves {
        for entity in entities
            .iter()
            .filter(|entity| entity.info.name == item.new_name && !moved.contains(&entity.path))
        {
            conflicts.insert(format!(
                "entity {} in {}",
                item.new_name,
                entity.path.display()
            ));
        }
        if item.new_file.exists() && item.new_file != item.old_file {
            conflicts.insert(format!("file {}", item.new_file.display()));
        }
        if let Some(path) = &item.new_sidecars {
            if path.exists() {
                conflicts.insert(format!("sidecar directory {}", path.display()));
            }
        }
        if let Some(path) = &item.new_repo_files {
            if path.exists() {
                conflicts.insert(format!("repository directory {}", path.display()));
            }
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(RenameError::Exists {
            conflicts: conflicts.into_iter().collect(),
        })
    }
}

pub(super) fn read(path: &Path) -> Result<Vec<u8>, RenameError> {
    std::fs::read(path).map_err(|error| RenameError::Io {
        path: path.to_path_buf(),
        why: error.to_string(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn scan_text_file(
    path: &Path,
    kind: FileKind,
    spec: &Spec,
    mode: refs::Mode,
    qualified: Option<&scan::Qualified>,
    changes: &mut Vec<FileChange>,
    skipped: &mut Vec<PathBuf>,
) -> Result<(), RenameError> {
    let bytes = read(path)?;
    if kind == FileKind::Sidecar
        && path
            .file_name()
            .is_some_and(|name| name == "definition.xml")
    {
        let pass = scan::scan_xml_with_identity(
            &bytes,
            &spec.old,
            mode,
            &spec.new,
            false,
            qualified.and_then(|q| q.collection.as_deref()),
        )
        .map_err(|error| RenameError::Xml {
            path: path.to_path_buf(),
            why: error.to_string(),
        })?;
        push_change(changes, path.to_path_buf(), kind, &bytes, pass);
        return Ok(());
    }
    if kind == FileKind::Config {
        let pass = scan::scan_entity_config(&bytes, &spec.old, &spec.new, mode, qualified)
            .map_err(|why| RenameError::Xml {
                path: path.to_path_buf(),
                why,
            })?;
        push_change(changes, path.to_path_buf(), kind, &bytes, pass);
        return Ok(());
    }
    let scanned = if kind == FileKind::Sidecar && path.extension().is_some_and(|ext| ext == "js") {
        scan::scan_entity_script(
            &bytes,
            &spec.old,
            mode,
            &spec.new,
            path.parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "GetDBInfo"),
            qualified.and_then(|q| q.collection.as_deref()),
        )
    } else {
        scan::scan_text_with(&bytes, &spec.old, mode, &spec.new, qualified)
    };
    match scanned {
        Ok(mut pass) => {
            // A SQL file outside the sources is a migration or a query: a migration documents an old
            // name and a new one (including the one an earlier rename generated), so rewriting it
            // would destroy what it says. List what it mentions; never change it.
            let is_sql = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"));
            if kind == FileKind::Outside && is_sql {
                pass.edits.clear();
                for finding in &mut pass.findings {
                    finding.tier = refs::Tier::Review;
                    finding.applied = false;
                }
            }
            push_change(changes, path.to_path_buf(), kind, &bytes, pass)
        }
        Err(_) => skipped.push(path.to_path_buf()),
    }
    Ok(())
}

pub(super) fn push_change(
    changes: &mut Vec<FileChange>,
    path: PathBuf,
    kind: FileKind,
    bytes: &[u8],
    pass: scan::XmlPass,
) {
    if !pass.findings.is_empty() {
        changes.push(FileChange {
            path,
            kind,
            digest: Sha256::digest(bytes).into(),
            edits: pass.edits,
            findings: pass.findings,
        });
    }
}
