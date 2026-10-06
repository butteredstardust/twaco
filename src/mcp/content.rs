use super::requests::content as tool;
use super::*;

pub(crate) fn repo_tool(
    solution: &Solution,
    arguments: tool::RepoRequest,
) -> Result<Value, ToolError> {
    use crate::core::commands::repo::{self as command, RepoAction, RepoRequest};
    use tool::RepoAction as RequestAction;
    if matches!(arguments.action, RequestAction::List) {
        let request = RepoRequest {
            action: RepoAction::List,
            profile: arguments.profile,
        };
        let mut notices = commands::Notices::default();
        let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
            .map_err(ToolError::coded)?;
        let command::RepoOutcome::Listed { repositories, .. } = outcome else {
            unreachable!()
        };
        let mut result = json!({ "ok": true, "repositories": repositories });
        add_notices(&mut result, &notices);
        return Ok(result);
    }
    let repository = arguments
        .repository
        .as_ref()
        .filter(|repository| !repository.is_empty())
        .ok_or_else(|| ToolError::invalid("`repository` is required"))?;
    match arguments.action {
        RequestAction::Ls => {
            let folder = arguments.path.as_ref().map(String::as_str).unwrap_or("/");
            let request = RepoRequest {
                action: RepoAction::Ls {
                    repository: repository.to_string(),
                    path: folder.to_string(),
                    recursive: arguments.recursive,
                },
                profile: arguments.profile,
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(ToolError::coded)?;
            let command::RepoOutcome::Ls { listing, .. } = outcome else {
                unreachable!()
            };
            let shown = if arguments.detail { usize::MAX } else { 200 };
            let mut result = json!({
                "ok": true,
                "repository": repository,
                "folders": listing.folders,
                "files": listing.files.iter().take(shown).map(|file| json!({
                    "path": file.path,
                    "size": file.size,
                    "modified": logs::iso(file.modified),
                })).collect::<Vec<_>>(),
                "file_count": listing.files.len(),
            });
            if listing.files.len() > shown {
                result["note"] = json!(format!(
                    "{} files in all; detail: true lists every one",
                    listing.files.len()
                ));
            }
            add_notices(&mut result, &notices);
            Ok(result)
        }
        RequestAction::Get => {
            let path = arguments
                .path
                .as_ref()
                .filter(|path| !path.is_empty())
                .ok_or_else(|| ToolError::invalid("`path` is required"))?;
            let request = RepoRequest {
                action: RepoAction::Get {
                    repository: repository.to_string(),
                    path: path.to_string(),
                    out: None,
                    force: false,
                },
                profile: arguments.profile,
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(ToolError::coded)?;
            let command::RepoOutcome::Got { bytes, .. } = outcome else {
                unreachable!()
            };
            let max = arguments.max_chars as usize;
            let digest = repo::sha256_hex(&bytes);
            match std::str::from_utf8(&bytes) {
                Ok(text) => {
                    let total = text.chars().count();
                    let mut result = json!({
                        "ok": true,
                        "repository": repository,
                        "path": repo::remote_path(path).map_err(ToolError::coded)?,
                        "size": bytes.len(),
                        "sha256": digest,
                        "text": text.chars().take(max).collect::<String>(),
                    });
                    if total > max {
                        result["truncated"] = json!(true);
                        result["note"] = json!(format!(
                            "{total} characters in all; raise max_chars, or `twaco repo get --out`"
                        ));
                    }
                    add_notices(&mut result, &notices);
                    Ok(result)
                }
                Err(_) => {
                    let mut result = json!({
                        "ok": true,
                    "repository": repository,
                    "path": repo::remote_path(path).map_err(ToolError::coded)?,
                    "size": bytes.len(),
                    "sha256": digest,
                    "binary": true,
                        "note": "not text, so its bytes are not returned here; `twaco repo get <repo> <path> --out <file>` saves it",
                    });
                    add_notices(&mut result, &notices);
                    Ok(result)
                }
            }
        }
        RequestAction::Status => {
            let request = RepoRequest {
                action: RepoAction::Status {
                    repository: repository.to_string(),
                },
                profile: arguments.profile,
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(ToolError::coded)?;
            let command::RepoOutcome::Status {
                local, compared, ..
            } = outcome
            else {
                unreachable!()
            };
            let mut counts = std::collections::BTreeMap::new();
            for item in &compared {
                *counts.entry(item.state.label()).or_insert(0usize) += 1;
            }
            let detail = arguments.detail;
            let listed: Vec<Value> = compared
                .iter()
                .filter(|item| detail || item.state != repo::State::Same)
                .map(|item| json!({ "path": item.path, "state": item.state.label(), "local_size": item.local_size, "remote_size": item.remote_size }))
                .collect();
            let mut result = json!({
                "ok": true,
                "repository": repository,
                "local": local.display().to_string(),
                "counts": counts,
                "in_sync": compared.iter().all(|item| item.state == repo::State::Same),
                (if detail { "files" } else { "attention" }): listed,
            });
            add_notices(&mut result, &notices);
            Ok(result)
        }
        RequestAction::List => unreachable!("list returns before repository is required"),
    }
}

/// An import into the server, a plan unless dry_run is false.
pub(crate) fn import_tool(
    solution: &Solution,
    arguments: tool::ImportRequest,
) -> Result<Value, ToolError> {
    use crate::core::commands::imports::{self as command, ImportAction, ImportRequest};
    let dry_run = arguments.dry_run;
    let (properties, tables) = (arguments.overwrite_properties, arguments.overwrite_tables);
    let mode = if dry_run {
        commands::Mode::Plan
    } else {
        commands::Mode::Apply
    };
    let profile = arguments.profile.clone();
    let differs = |list: &[imports::Differs]| -> Vec<Value> {
        list.iter()
            .map(|d| json!({ "type": d.entity_type, "name": d.name, "what": d.what }))
            .collect()
    };
    match arguments.action {
        tool::ImportAction::File => {
            let relative = required_text(&arguments.file, "file")?;
            let real = std::fs::canonicalize(solution.root.join(relative))
                .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{relative}: {e}")))?;
            let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
            if !real.starts_with(&root) {
                return Err(ToolError::invalid(format!(
                    "{relative} is outside the solution"
                )));
            }
            let bytes = std::fs::read(&real)
                .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{relative}: {e}")))?;
            let file_name = real
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "import.xml".into());
            let request = ImportRequest {
                action: ImportAction::File { file_name, bytes },
                mode,
                overwrite_properties: properties,
                overwrite_tables: tables,
                profile,
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(ToolError::coded)?;
            let command::ImportOutcome::File { plan, .. } = outcome else {
                unreachable!()
            };
            let names = |list: &[(String, String)]| {
                list.iter()
                    .map(|(c, n)| format!("{c}/{n}"))
                    .collect::<Vec<_>>()
            };
            let mut result = json!({
                "ok": true,
                "dry_run": dry_run,
                "adds": names(&plan.new),
                "replaces": names(&plan.replaced),
                "applied": plan.applied,
            });
            add_notices(&mut result, &notices);
            Ok(result)
        }
        tool::ImportAction::SourceControl => {
            let request = ImportRequest {
                action: ImportAction::SourceControl {
                    repository: required_text(&arguments.repository, "repository")?.to_string(),
                    path: required_text(&arguments.path, "path")?.to_string(),
                },
                mode,
                overwrite_properties: properties,
                overwrite_tables: tables,
                profile,
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(ToolError::coded)?;
            let command::ImportOutcome::SourceControl {
                report: imported, ..
            } = outcome
            else {
                unreachable!()
            };
            let mut result = json!({ "ok": true, "dry_run": dry_run, "entities": imported.total, "differ": differs(&imported.differ) });
            if let Some(after) = &imported.still_differ {
                result["still_differ"] = json!(differs(after));
            }
            add_notices(&mut result, &notices);
            Ok(result)
        }
    }
}

/// An export from the server, to a file inside the solution or into a repository.
pub(crate) fn export_tool(
    solution: &Solution,
    arguments: tool::ExportRequest,
) -> Result<Value, ToolError> {
    use crate::core::commands::export::{self as command, ExportAction, ExportRequest};
    let action = arguments.action;
    let profile = arguments.profile.clone();
    if action == tool::ExportAction::SourceControl {
        let filters = export::Filters {
            project: arguments.project.as_ref().cloned(),
            collection: arguments.collection.as_ref().cloned(),
            tags: arguments.tags.as_ref().cloned(),
            include_dependents: arguments.with_dependents,
        };
        let dry_run = arguments.dry_run;
        let request = ExportRequest {
            action: ExportAction::SourceControl {
                repository: required_text(&arguments.repository, "repository")?.to_string(),
                path: required_text(&arguments.path, "path")?.to_string(),
                filters,
                zip: arguments.zip.as_ref().cloned(),
                mode: if dry_run {
                    commands::Mode::Plan
                } else {
                    commands::Mode::Apply
                },
            },
            profile,
        };
        let mut notices = commands::Notices::default();
        let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
            .map_err(ToolError::coded)?;
        let command::ExportOutcome::SourceControl {
            plan,
            download: link,
            ..
        } = outcome
        else {
            unreachable!()
        };
        let mut result =
            json!({ "ok": true, "dry_run": dry_run, "change": plan, "download": link });
        add_notices(&mut result, &notices);
        return Ok(result);
    }
    let what = match action {
        tool::ExportAction::Entity => {
            export::What::entity(required_text(&arguments.entity, "entity")?)
                .map_err(ToolError::coded)?
        }
        tool::ExportAction::Collection => export::What::Collection {
            collection: required_text(&arguments.collection, "collection")?.to_string(),
            project: arguments.project.as_ref().cloned(),
        },
        tool::ExportAction::Project => export::What::Project {
            project: required_text(&arguments.project, "project")?.to_string(),
        },
        tool::ExportAction::SourceControl => unreachable!("a source-control export returned above"),
    };
    let relative = required_text(&arguments.out, "out")?;
    let out = out_path(solution, relative, arguments.overwrite)?;
    let request = ExportRequest {
        action: ExportAction::Xml {
            what,
            out,
            force: true,
        },
        profile,
    };
    let mut notices = commands::Notices::default();
    let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?;
    let command::ExportOutcome::Xml { exported, .. } = outcome else {
        unreachable!()
    };
    let mut result = json!({
        "ok": true,
        "out": relative,
        "bytes": exported.xml.len(),
        "entities": exported.counts.iter().map(|(c, n)| json!({ "collection": c, "count": n })).collect::<Vec<_>>(),
    });
    add_notices(&mut result, &notices);
    Ok(result)
}

/// A file to write, given relative to the solution: a plain path whose nearest existing folder
/// is really inside the solution, and not an existing file unless overwriting.
fn out_path(
    solution: &Solution,
    relative: &str,
    overwrite: bool,
) -> Result<std::path::PathBuf, ToolError> {
    // Only plain names: `\\x` or `C:x` is not absolute to Rust on Windows, yet joins outside.
    let plain = std::path::Path::new(relative)
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !plain || relative.trim().is_empty() {
        return Err(ToolError::invalid(format!(
            "{relative} must be a plain path inside the solution"
        )));
    }
    let out = solution.root.join(relative);
    // And no link along the way may lead out: the nearest folder that exists must be inside.
    let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
    let existing = out
        .ancestors()
        .skip(1)
        .find(|a| a.exists())
        .unwrap_or(&solution.root);
    let real = std::fs::canonicalize(existing)
        .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{}: {e}", existing.display())))?;
    if !real.starts_with(&root) {
        return Err(ToolError::invalid(format!(
            "{relative} leads outside the solution"
        )));
    }
    if out.exists() && !overwrite {
        return Err(ToolError::with(
            ErrorCode::AlreadyExists,
            format!("{relative} exists; pass overwrite: true to replace it"),
        ));
    }
    Ok(out)
}

/// The repository packaged for release, offline, into a file inside the solution.
pub(crate) fn package_tool(
    solution: &Solution,
    arguments: tool::PackageRequest,
) -> Result<Value, ToolError> {
    use crate::core::commands::package::{self, PackageAction, PackageDetail, PackageRequest};
    let relative = nonempty(&arguments.out, "out")?;
    let out = out_path(solution, relative, arguments.overwrite)?;
    let action = match arguments.action {
        tool::PackageAction::Bundle => PackageAction::Bundle {
            part: match arguments.part {
                tool::PackagePart::All => crate::core::package::Part::All,
                tool::PackagePart::Backend => crate::core::package::Part::Backend,
                tool::PackagePart::Frontend => crate::core::package::Part::Frontend,
            },
        },
        tool::PackageAction::SourceControl => PackageAction::SourceControl,
        tool::PackageAction::Extension => PackageAction::Extension {
            editable: arguments.editable,
        },
    };
    let outcome = package::execute(
        solution,
        &PackageRequest {
            action,
            project: arguments.project.as_ref().cloned(),
            out,
            force: true,
        },
    )
    .map_err(ToolError::coded)?;
    let detail = match outcome.detail {
        PackageDetail::Bundle { entities, files } => {
            json!({ "entities": entities, "files": files })
        }
        PackageDetail::SourceControl { entities } => json!({ "entities": entities }),
        PackageDetail::ProjectExtension {
            editable,
            version,
            entities,
        } => json!({ "editable": editable, "version": version, "entities": entities }),
        PackageDetail::SolutionExtension {
            editable,
            version,
            projects,
        } => {
            let projects: Vec<Value> = projects
                .iter()
                .map(|(project, entities)| json!({ "project": project, "entities": entities }))
                .collect();
            json!({ "editable": editable, "version": version, "projects": projects })
        }
    };
    Ok(json!({ "ok": true, "out": relative, "bytes": outcome.bytes, "detail": detail }))
}

/// The server's subsystem settings, read-only, secrets hidden.
pub(crate) fn extensions_tool(
    solution: &Solution,
    arguments: tool::ExtensionsRequest,
) -> Result<Value, ToolError> {
    use crate::core::commands::extensions::{self as command, ExtensionAction, ExtensionRequest};
    let package_json = |p: &extensions::Package| json!({ "name": p.name, "version": p.version, "vendor": p.vendor, "description": p.description, "minimumThingWorxVersion": p.minimum_thingworx });
    match arguments.action {
        tool::ExtensionsAction::List => {
            let request = ExtensionRequest {
                action: ExtensionAction::List,
                profile: arguments.profile.clone(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(ToolError::coded)?;
            let command::ExtensionOutcome::Listed { packages, .. } = outcome else {
                unreachable!()
            };
            let mut result = json!({ "ok": true, "packages": packages.iter().map(package_json).collect::<Vec<_>>() });
            add_notices(&mut result, &notices);
            Ok(result)
        }
        tool::ExtensionsAction::Show => {
            let request = ExtensionRequest {
                action: ExtensionAction::Show {
                    name: required_text(&arguments.package, "package")?.to_string(),
                },
                profile: arguments.profile.clone(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(ToolError::coded)?;
            let command::ExtensionOutcome::Shown { shown, .. } = outcome else {
                unreachable!()
            };
            let mut result = json!({ "ok": true, "package": package_json(&shown.package), "extensions": shown.extensions, "in_use": shown.in_use });
            add_notices(&mut result, &notices);
            Ok(result)
        }
    }
}

/// Import or remove an extension package, a plan unless dry_run is false.
pub(crate) fn extension_write_tool(
    solution: &Solution,
    arguments: tool::ExtensionWriteRequest,
) -> Result<Value, ToolError> {
    use crate::core::commands::extensions::{self as command, ExtensionAction, ExtensionRequest};
    let dry_run = arguments.dry_run;
    let mode = if dry_run {
        commands::Mode::Plan
    } else {
        commands::Mode::Apply
    };
    let profile = arguments.profile.clone();
    match arguments.action {
        tool::ExtensionWriteAction::Import => {
            let relative = required_text(&arguments.zip, "zip")?;
            // A zip of the solution, never one outside it.
            let real = std::fs::canonicalize(solution.root.join(relative))
                .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{relative}: {e}")))?;
            let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
            if !real.starts_with(&root) {
                return Err(ToolError::invalid(format!(
                    "{relative} is outside the solution"
                )));
            }
            let zip = std::fs::read(&real)
                .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{relative}: {e}")))?;
            let file_name = real
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "package.zip".into());
            let request = ExtensionRequest {
                action: ExtensionAction::Import {
                    file_name,
                    zip,
                    mode,
                },
                profile,
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(ToolError::coded)?;
            let command::ExtensionOutcome::Imported { imported, .. } = outcome else {
                unreachable!()
            };
            let mut result = json!({
                "ok": true,
                "dry_run": dry_run,
                "change": imported.plan,
                "applied": imported.applied,
                "note": if imported.applied { "installed; the package list shows it" } else { "the server validated it and installed nothing; pass dry_run: false" },
            });
            add_notices(&mut result, &notices);
            Ok(result)
        }
        tool::ExtensionWriteAction::Remove => {
            let request = ExtensionRequest {
                action: ExtensionAction::Remove {
                    name: required_text(&arguments.package, "package")?.to_string(),
                    mode,
                },
                profile,
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(ToolError::coded)?;
            let command::ExtensionOutcome::Removed { plan, .. } = outcome else {
                unreachable!()
            };
            let mut result =
                json!({ "ok": true, "dry_run": dry_run, "change": plan, "applied": !dry_run });
            add_notices(&mut result, &notices);
            Ok(result)
        }
    }
}

/// One change to a file repository, a plan unless dry_run is false.
pub(crate) fn repo_write_tool(
    solution: &Solution,
    arguments: tool::RepoWriteRequest,
) -> Result<Value, ToolError> {
    let repository = nonempty(&arguments.repository, "repository")?;
    let dry_run = arguments.dry_run;
    if let Some(way) = match arguments.action {
        tool::RepoWriteAction::Push => Some(repo::Direction::Push),
        tool::RepoWriteAction::Pull => Some(repo::Direction::Pull),
        _ => None,
    } {
        use crate::core::commands::repo::{self as command, RepoAction, RepoRequest};
        let request = RepoRequest {
            action: RepoAction::Sync {
                repository: repository.to_string(),
                direction: way,
                overwrite: arguments.overwrite,
                mode: if dry_run {
                    commands::Mode::Plan
                } else {
                    commands::Mode::Apply
                },
            },
            profile: arguments.profile.clone(),
        };
        let mut notices = commands::Notices::default();
        let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
            .map_err(ToolError::coded)?;
        let command::RepoOutcome::Synced { local, synced, .. } = outcome else {
            unreachable!()
        };
        let mut result = json!({
            "ok": true,
            "repository": repository,
            "local": local.display().to_string(),
            "dry_run": dry_run,
            "copied": synced.copied,
            "same": synced.same,
            "left_alone": synced.left,
        });
        if dry_run && !synced.copied.is_empty() {
            result["note"] = json!("nothing was copied; pass dry_run: false");
        }
        add_notices(&mut result, &notices);
        return Ok(result);
    }
    let path =
        repo::remote_path(required_text(&arguments.path, "path")?).map_err(ToolError::coded)?;
    let overwrite = arguments.overwrite;
    let change = match arguments.action {
        tool::RepoWriteAction::Put => {
            let bytes = match (arguments.text.as_deref(), arguments.local.as_deref()) {
                (Some(_), Some(_)) => {
                    return Err(ToolError::invalid("give text or local, not both"))
                }
                (Some(content), None) => content.as_bytes().to_vec(),
                (None, Some(local)) => {
                    // A file of the solution, never one outside it.
                    let candidate = solution.root.join(local);
                    let real = std::fs::canonicalize(&candidate).map_err(|e| {
                        ToolError::with(ErrorCode::IoError, format!("{local}: {e}"))
                    })?;
                    let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
                    if !real.starts_with(&root) {
                        return Err(ToolError::invalid(format!(
                            "{local} is outside the solution"
                        )));
                    }
                    std::fs::read(&real)
                        .map_err(|e| ToolError::with(ErrorCode::IoError, format!("{local}: {e}")))?
                }
                (None, None) => return Err(ToolError::invalid("put needs text or local")),
            };
            repo::Change::Put {
                path,
                bytes,
                overwrite,
            }
        }
        tool::RepoWriteAction::Mkdir => repo::Change::Mkdir { path },
        tool::RepoWriteAction::Rm if path == "/" => {
            return Err(ToolError::invalid("the repository root cannot be deleted"))
        }
        tool::RepoWriteAction::Rm => repo::Change::Remove {
            path,
            recursive: arguments.recursive,
        },
        tool::RepoWriteAction::Mv => {
            let to =
                repo::remote_path(required_text(&arguments.to, "to")?).map_err(ToolError::coded)?;
            repo::Change::Move {
                from: path,
                to,
                overwrite,
            }
        }
        tool::RepoWriteAction::Push | tool::RepoWriteAction::Pull => {
            unreachable!("a push or pull returned above")
        }
    };
    use crate::core::commands::repo::{self as command, RepoAction, RepoRequest};
    let request = RepoRequest {
        action: RepoAction::Change {
            repository: repository.to_string(),
            change,
            mode: if dry_run {
                commands::Mode::Plan
            } else {
                commands::Mode::Apply
            },
        },
        profile: arguments.profile.clone(),
    };
    let mut notices = commands::Notices::default();
    let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?;
    let command::RepoOutcome::Changed { planned, .. } = outcome else {
        unreachable!()
    };
    let mut result = json!({
        "ok": true,
        "repository": repository,
        "change": planned.plan,
        "dry_run": dry_run,
        "applied": planned.applied,
    });
    if planned.nothing {
        result["note"] = json!("nothing to do");
    } else if !planned.applied {
        result["note"] = json!("nothing was sent; pass dry_run: false");
    }
    add_notices(&mut result, &notices);
    Ok(result)
}
