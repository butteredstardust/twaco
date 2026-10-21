//! Applying the mechanical half of a report: the conflicts taken, then the backend writes and
//! the new mashup files, in one transaction.

use super::compare::same_frame;
use super::flatten::{attribute, canonical, child_elements, et_text, is_named, py_space, text_of};
use super::read::{
    entity_document, entity_span, export_entities, raw_entity, replace_entity_bytes,
    repository_entities,
};
use super::{retype, Change, Kind};
use super::{
    AdoptError, ApplyOutcome, EntityRef, EntityReport, Report, ServiceReport, Take, TakeSide,
    TAKE_ALL_UI,
};
use crate::core::config::Solution;
use crate::core::lock::WorkspaceLock;
use crate::core::normalise::{self, Element};
use crate::core::transaction::Transaction;
use crate::core::{scan, sidecar, sync, workspace};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
/// Adopt the report's selected changes from an export.
///
/// UI content follows the designer workflow, but a stale or unresolved conflicting export never
/// overwrites it.  Backend entities are either taken whole or merged with service sidecars: the
/// frame is selected by the three-way base and each script service is selected independently.
/// Explicit takes resolve only reported conflicts or unknowns.
///
/// Every write is worked out before the first is made, so an export that cannot be adopted
/// (a payload that is not JSON, a name that is not a file name) changes nothing. The writes are
/// then made as one transaction under `lock`: all of them or, after a failure or a crash, none.
pub(crate) fn apply(
    solution: &Solution,
    export: &Path,
    report: &Report,
    takes: &[Take],
    lock: &WorkspaceLock,
) -> Result<ApplyOutcome, AdoptError> {
    let exported = export_entities(export)?;
    let here = repository_entities(solution)?;
    validate_takes(report, takes)?;
    // In the report's order, notes and writes alike, so the output reads as the comparison did.
    let mut steps: Vec<(Option<Write>, String)> = Vec::new();
    for entry in &report.entities {
        // An identical entity has nothing to adopt and nothing worth a line.
        if entry.change == Change::Same {
            continue;
        }
        let Some(element) = exported.get(&entry.entity) else {
            continue;
        };
        let name = &entry.entity.name;
        match entry.kind {
            Kind::Ui => match entry.entity.collection.as_str() {
                "Mashups" => {
                    if !ui_should_write(entry, takes) {
                        steps.push((None, ui_skip_line(entry, takes)));
                        continue;
                    }
                    if !is_file_name(name) {
                        return Err(AdoptError::Export {
                            path: export.to_path_buf(),
                            why: format!(
                            "the mashup name {name:?} cannot be a file name; nothing was adopted"
                        ),
                        });
                    }
                    let dir = solution.src_root().join(name).join("mashup");
                    let assets = mashup_assets(element).map_err(|why| io_error(&dir, why))?;
                    if !here.contains_key(&entry.entity) {
                        if assets.is_none() {
                            steps.push((None, format!("skipped  Mashups/{name}: the export holds no mashup content to create it from")));
                            continue;
                        }
                        match new_mashup_file(solution, &entry.project, element, name)? {
                            Some((path, text)) => {
                                let line = format!("created  {}", relative_to(solution, &path));
                                steps.push((Some(Write::Entity(path, text)), line));
                            }
                            None => {
                                steps.push((
                                    None,
                                    format!(
                                    "skipped  Mashups/{name}: project {:?} is not in this solution",
                                    entry.project
                                ),
                                ));
                                continue;
                            }
                        }
                    }
                    if let Some(assets) = assets {
                        let current =
                            workspace::read_mashup(&dir).map_err(|e| io_error(&dir, e))?;
                        // The comparison read the entity XML; sidecars edited since the last
                        // sync are newer work it never saw. Only an explicit take replaces them.
                        let synced = match here.get(&entry.entity) {
                            Some((ours, _)) => {
                                mashup_assets(ours).map_err(|why| io_error(&dir, why))?
                            }
                            None => current.clone(),
                        };
                        let same =
                            |a: Option<&super::super::mashup::Assets>,
                             b: Option<&super::super::mashup::Assets>| {
                                match (a, b) {
                                    (Some(a), Some(b)) => {
                                        canonical(&a.content) == canonical(&b.content)
                                            && canonical(&a.css) == canonical(&b.css)
                                    }
                                    (a, b) => a.is_none() && b.is_none(),
                                }
                            };
                        if current.is_some()
                            && !same(current.as_ref(), synced.as_ref())
                            && !same(current.as_ref(), Some(&assets))
                            && !ui_taken(entry, takes)
                        {
                            steps.push((
                                None,
                                format!("kept     Mashups/{name}: its sidecars changed since the last sync; sync, then adopt again"),
                            ));
                            continue;
                        }
                        if current.as_ref() != Some(&assets) {
                            steps.push((
                                Some(Write::Sidecars(dir, assets)),
                                format!("sidecar  {name}"),
                            ));
                        }
                    }
                }
                "MediaEntities" => {
                    if !ui_should_write(entry, takes) {
                        steps.push((None, ui_skip_line(entry, takes)));
                        continue;
                    }
                    if let (Some((_, path)), Some(content)) = (
                        here.get(&entry.entity),
                        child_elements(element)
                            .find(|c| is_named(c, "content"))
                            .and_then(et_text),
                    ) {
                        let encoded: String = content.chars().filter(|c| !py_space(*c)).collect();
                        if encoded.is_empty() {
                            continue;
                        }
                        if !is_base64(&encoded) {
                            return Err(AdoptError::Export {
                            path: export.to_path_buf(),
                            why: format!("MediaEntities/{name}: the image is not base64; nothing was adopted"),
                        });
                        }
                        if let Some(text) = media_content(path, &encoded)? {
                            let line = format!("content  {}", relative_to(solution, path));
                            steps.push((Some(Write::Entity(path.clone(), text)), line));
                        }
                    } else if !here.contains_key(&entry.entity) {
                        steps.push((None, ui_not_adopted_line(entry)));
                    }
                }
                _ => {
                    if !ui_should_write(entry, takes) {
                        steps.push((None, ui_skip_line(entry, takes)));
                    } else {
                        steps.push((None, ui_not_adopted_line(entry)));
                    }
                }
            },
            Kind::Backend => plan_backend(
                solution, export, report, entry, element, &here, takes, &mut steps,
            )?,
        }
    }
    let retypes = map_retyped_arguments(solution, report, &here, &mut steps)?;
    let mut transaction = Transaction::new(&solution.root, "adopt");
    let mut lines = Vec::new();
    for (write, line) in steps {
        match write {
            Some(Write::Entity(path, text)) => transaction
                .write_file(&path, text.into_bytes())
                .map_err(AdoptError::Write)?,
            Some(Write::Bytes(path, bytes)) => transaction
                .write_file(&path, bytes)
                .map_err(AdoptError::Write)?,
            Some(Write::Sidecars(dir, assets)) => {
                // As extract writes them: LF line endings.
                for (file, text) in [
                    ("content.json", &assets.content),
                    ("custom.css", &assets.css),
                ] {
                    transaction
                        .write_file(&dir.join(file), text.replace("\r\n", "\n").into_bytes())
                        .map_err(AdoptError::Write)?;
                }
            }
            Some(Write::Services {
                dir,
                services,
                remove,
            }) => {
                for service in services.values() {
                    for (file, text) in [
                        ("definition.xml", &service.definition),
                        ("script.js", &service.script),
                    ] {
                        let path = dir.join(&service.name).join(file);
                        let mut wanted = text.replace("\r\n", "\n");
                        // A sidecar we keep is left byte for byte, CRLF checkout included, and
                        // one we change keeps the line endings it had.
                        let current = std::fs::read_to_string(&path).ok();
                        match current {
                            Some(current) if current.replace("\r\n", "\n") == wanted => continue,
                            Some(current) if current.contains("\r\n") => {
                                wanted = wanted.replace('\n', "\r\n");
                            }
                            _ => {}
                        }
                        transaction
                            .write_file(&path, wanted.into_bytes())
                            .map_err(AdoptError::Write)?;
                    }
                }
                for service in remove {
                    let folder = dir.join(service);
                    for file in [
                        "definition.xml",
                        "script.js",
                        "jsconfig.json",
                        "twaco-globals.d.ts",
                    ] {
                        let path = folder.join(file);
                        if let Ok(bytes) = std::fs::read(&path) {
                            transaction
                                .delete_file(&path, &bytes)
                                .map_err(AdoptError::Write)?;
                        }
                    }
                }
            }
            None => {}
        }
        lines.push(line);
    }
    let wrote = !transaction.is_empty();
    tracing::info!(writes = transaction.len(), "adopt: writing");
    if wrote {
        transaction.apply(lock).map_err(AdoptError::Write)?;
    }
    tracing::info!(wrote, "adopt: done");
    Ok(ApplyOutcome {
        lines,
        types: super::super::types::refresh_after_write(solution, wrote),
        retypes,
    })
}

/// Rewrite the literal arguments scripts pass to the service parameters this apply retypes, so
/// each is one the new base type takes. Only a retype whose entity this apply writes counts. Each
/// script text is rewritten where it ends up: in a write already planned, or as a new write of its
/// file, sidecar and entity XML alike so the two stay in step.
fn map_retyped_arguments(
    solution: &Solution,
    report: &Report,
    here: &BTreeMap<EntityRef, (Element, PathBuf)>,
    steps: &mut Vec<(Option<Write>, String)>,
) -> Result<BTreeSet<(String, String, String)>, AdoptError> {
    let planned_xml: BTreeSet<PathBuf> = steps
        .iter()
        .filter_map(|(write, _)| match write {
            Some(Write::Entity(path, _) | Write::Bytes(path, _)) => Some(path.clone()),
            _ => None,
        })
        .collect();
    let retypes: Vec<retype::Retype> = retype::retypes(solution, report)
        .into_iter()
        .filter(|retype| {
            here.get(&retype.entity)
                .is_some_and(|(_, path)| planned_xml.contains(path))
        })
        .collect();
    let adopted = retypes
        .iter()
        .map(|retype| retype.key())
        .collect::<BTreeSet<_>>();
    if retypes.is_empty() {
        return Ok(adopted);
    }
    let mut added = Vec::new();
    for entity in workspace::discover(solution).entities {
        let caller = entity.info.name.as_str();
        for path in retype::sidecar_scripts(solution, &entity) {
            let planned = steps.iter_mut().find_map(|(write, _)| match write {
                Some(Write::Services { dir, services, .. }) => services
                    .values_mut()
                    .find(|service| dir.join(&service.name).join("script.js") == path),
                _ => None,
            });
            if let Some(service) = planned {
                let whole = [scan::Span::new(0, service.script.len())];
                if let Some(bytes) =
                    retype::rewrite(service.script.as_bytes(), &whole, caller, &retypes)
                {
                    service.script = String::from_utf8(bytes).expect("a UTF-8 script stays UTF-8");
                }
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|error| io_error(&path, error))?;
            let whole = [scan::Span::new(0, bytes.len())];
            if let Some(bytes) = retype::rewrite(&bytes, &whole, caller, &retypes) {
                let line = format!("mapped   {}", relative_to(solution, &path));
                added.push((Some(Write::Bytes(path, bytes)), line));
            }
        }
        let planned = steps.iter_mut().find_map(|(write, _)| match write {
            Some(Write::Bytes(path, bytes)) if *path == entity.path => Some(bytes),
            _ => None,
        });
        if let Some(bytes) = planned {
            let spans = retype::script_spans(bytes);
            if let Some(rewritten) = retype::rewrite(bytes, &spans, caller, &retypes) {
                *bytes = rewritten;
            }
            continue;
        }
        let planned_text = steps.iter_mut().find_map(|(write, _)| match write {
            Some(Write::Entity(path, text)) if *path == entity.path => Some(text),
            _ => None,
        });
        if let Some(text) = planned_text {
            let spans = retype::script_spans(text.as_bytes());
            if let Some(rewritten) = retype::rewrite(text.as_bytes(), &spans, caller, &retypes) {
                *text = String::from_utf8(rewritten).expect("UTF-8 XML stays UTF-8");
            }
            continue;
        }
        let bytes = std::fs::read(&entity.path).map_err(|error| io_error(&entity.path, error))?;
        let spans = retype::script_spans(&bytes);
        if let Some(rewritten) = retype::rewrite(&bytes, &spans, caller, &retypes) {
            let line = format!("mapped   {}", relative_to(solution, &entity.path));
            added.push((Some(Write::Bytes(entity.path.clone(), rewritten)), line));
        }
    }
    steps.extend(added);
    Ok(adopted)
}

/// One file or sidecar folder `apply` will write, worked out in full beforehand.
enum Write {
    Entity(PathBuf, String),
    Bytes(PathBuf, Vec<u8>),
    Sidecars(PathBuf, super::super::mashup::Assets),
    Services {
        dir: PathBuf,
        services: BTreeMap<String, sidecar::ServiceSidecar>,
        remove: Vec<String>,
    },
}

fn has_take(takes: &[Take], side: TakeSide, target: &str) -> bool {
    takes
        .iter()
        .any(|take| take.side == side && take.target == target)
}

fn validate_takes(report: &Report, takes: &[Take]) -> Result<(), AdoptError> {
    let known: BTreeSet<String> = report
        .entities
        .iter()
        .filter(|entry| {
            matches!(
                entry.change,
                Change::Conflict | Change::Unknown | Change::Same
            )
        })
        .map(|entry| entry.entity.name.clone())
        .chain(
            report
                .services
                .iter()
                .filter(|service| matches!(service.change, Change::Conflict | Change::Unknown))
                .map(|service| format!("{}.{}", service.entity, service.service)),
        )
        .collect();
    let ui_open = report.entities.iter().any(|entry| {
        entry.kind == Kind::Ui && matches!(entry.change, Change::Conflict | Change::Unknown)
    });
    let bad: Vec<String> = takes
        .iter()
        .filter(|take| {
            !known.contains(&take.target)
                && !(ui_open && take.side == TakeSide::Theirs && take.target == TAKE_ALL_UI)
        })
        .map(|take| take.target.clone())
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        let mut available = known.into_iter().collect::<Vec<_>>();
        if ui_open {
            available.push(format!("{TAKE_ALL_UI} (theirs only)"));
        }
        let available = available.join(", ");
        Err(AdoptError::Take(format!(
            "{}; available take targets: {available}",
            bad.join(", ")
        )))
    }
}

/// Whether the export's version of a UI entity was explicitly asked for, by name or as `ui`.
fn ui_taken(entry: &EntityReport, takes: &[Take]) -> bool {
    has_take(takes, TakeSide::Theirs, &entry.entity.name)
        || (has_take(takes, TakeSide::Theirs, TAKE_ALL_UI)
            && matches!(entry.change, Change::Conflict | Change::Unknown))
}

/// An unknown is a conflict here as everywhere: with no base, nothing shows which side changed.
fn ui_should_write(entry: &EntityReport, takes: &[Take]) -> bool {
    if has_take(takes, TakeSide::Ours, &entry.entity.name) {
        return false;
    }
    matches!(entry.change, Change::Theirs | Change::Added) || ui_taken(entry, takes)
}

/// For a UI entity adopt selected but has no writer for: saying nothing would read as adopted.
fn ui_not_adopted_line(entry: &EntityReport) -> String {
    format!(
        "not adopted {}: adopt writes mashups and media only; import it from the export",
        entry.entity.path()
    )
}

fn ui_skip_line(entry: &EntityReport, takes: &[Take]) -> String {
    let path = entry.entity.path();
    if has_take(takes, TakeSide::Ours, &entry.entity.name) {
        return format!("kept     {path}: ours");
    }
    match entry.change {
        Change::Stale => format!("kept     {path}: stale, ours is newer"),
        Change::Unknown if !ui_taken(entry, takes) => {
            format!(
                "conflict {path}: no base shows who changed it; --take theirs:{}, --take ours:{} or --take theirs:{TAKE_ALL_UI}",
                entry.entity.name, entry.entity.name
            )
        }
        Change::Conflict if !ui_taken(entry, takes) => {
            format!(
                "conflict {path}: both changed; --take theirs:{} or --take ours:{}",
                entry.entity.name, entry.entity.name
            )
        }
        Change::WeRemoved => format!("skipped  {path}: we removed it"),
        Change::Same => format!("skipped  {path}: unchanged"),
        _ => format!("skipped  {path}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_backend(
    solution: &Solution,
    export: &Path,
    report: &Report,
    entry: &EntityReport,
    _element: &Element,
    here: &BTreeMap<EntityRef, (Element, PathBuf)>,
    takes: &[Take],
    steps: &mut Vec<(Option<Write>, String)>,
) -> Result<(), AdoptError> {
    let entity = &entry.entity;
    let entity_name = &entity.name;
    if has_take(takes, TakeSide::Ours, entity_name) {
        steps.push((None, format!("kept     {}: ours", entity.path())));
        return Ok(());
    }
    let raw = raw_entity(export, entity)?;
    let parts =
        super::super::server_owned::parts(&solution.server_owned, &entity.collection, &entity.name);
    // Restore ours before layout's node-by-node comparison, so unchanged owned subtrees retain
    // their exact repository bytes even when another part of this entity is adopted.
    let raw = if let Some((_, path)) = here.get(entity) {
        let ours = std::fs::read(path).map_err(|e| io_error(path, e))?;
        super::super::server_owned::preserve(&raw, &ours, &parts)
            .map_err(|e| io_error(export, e))?
    } else {
        raw
    };
    let sample = here
        .get(entity)
        .map(|(_, path)| (entity, path))
        .or_else(|| {
            here.iter()
                .find(|(key, (value, _))| {
                    key.collection == entity.collection
                        && attribute(value, "projectName").as_deref()
                            == Some(entry.project.as_str())
                })
                .map(|(key, (_, path))| (key, path))
        })
        .or_else(|| {
            here.iter()
                .find(|(key, _)| key.collection == entity.collection)
                .map(|(key, (_, path))| (key, path))
        });
    let sample_bytes = sample
        .map(|(_, path)| std::fs::read(path).map_err(|e| io_error(path, e)))
        .transpose()?;
    let reference = match (sample, sample_bytes.as_ref()) {
        (Some((key, _)), Some(bytes)) => {
            let tokens = scan::tokenize(bytes).map_err(|e| io_error(export, e))?;
            entity_span(&tokens, bytes, key).map(|span| span.of(bytes))
        }
        _ => None,
    };
    let their_raw =
        super::super::layout::entity(&raw, reference, solution.format.indent_cdata_payload)
            .map_err(|e| io_error(export, e))?;
    let their_services = services_from_document(&entity_document(&entity.collection, &their_raw))?;
    // Both name folders under src/; the transaction would refuse an escape, but not `CON`, and
    // its refusal would not say which name was at fault.
    if let Some(bad) = std::iter::once(entity_name)
        .chain(their_services.keys())
        .find(|name| !is_file_name(name))
    {
        return Err(AdoptError::Export {
            path: export.to_path_buf(),
            why: format!(
                "{}: the name {bad:?} cannot be a file name; nothing was adopted",
                entity.path()
            ),
        });
    }
    let service_dir = solution.src_root().join(entity_name).join("services");

    if entry.change == Change::Added {
        let Some(path) = new_backend_path(solution, entry, here)? else {
            steps.push((
                None,
                format!(
                    "skipped  {}: project {:?} is not in this solution",
                    entity.path(),
                    entry.project
                ),
            ));
            return Ok(());
        };
        let bytes = if let Some((sample_key, sample)) = sample {
            // Shaped like a file of its own collection and project: that file's entity is
            // swapped for the new one, so the declaration and wrapper match the solution's.
            let old = std::fs::read(sample).map_err(|e| io_error(sample, e))?;
            replace_entity_bytes(
                &old,
                sample_key,
                &their_raw,
                solution.format.indent_cdata_payload,
            )?
        } else {
            let laid_out = super::super::layout::entity(
                &their_raw,
                None,
                solution.format.indent_cdata_payload,
            )
            .map_err(|e| io_error(export, e))?;
            // No peer file to copy the envelope from: the one `entity new` writes.
            super::super::entity::new::document(&entity.collection, &laid_out)
        };
        steps.push((
            Some(Write::Bytes(path.clone(), bytes)),
            format!("created  {}", relative_to(solution, &path)),
        ));
        steps.push((
            Some(Write::Services {
                dir: service_dir,
                services: their_services,
                remove: Vec::new(),
            }),
            format!("sidecar  {entity_name}"),
        ));
        return Ok(());
    }

    let Some((ours_element, path)) = here.get(entity) else {
        steps.push((None, format!("skipped  {}: we removed it", entity.path())));
        return Ok(());
    };
    let ours_raw = std::fs::read(path).map_err(|e| io_error(path, e))?;
    let ours_services = ours_services(solution, entity_name, &ours_raw)?;
    let their_entity_take = has_take(takes, TakeSide::Theirs, entity_name);

    let generated_here = report
        .services
        .iter()
        .any(|service| service.entity == *entity_name && service.generated);
    // The entity's state compares entity XML; a service's compares its sidecar. A sidecar edited
    // since the last sync makes the XML look like the base while the script is ours: such an
    // entity is merged service by service rather than replaced.
    let services_disagree = report.services.iter().any(|service| {
        service.entity == *entity_name
            && !service.generated
            && matches!(
                service.change,
                Change::Stale | Change::Conflict | Change::Unknown
            )
    });
    if !report
        .services
        .iter()
        .any(|service| service.entity == *entity_name)
        && normalise::hash_with_parts(&ours_raw, &parts).is_ok_and(|ours| {
            normalise::hash_with_parts(&entity_document(&entity.collection, &their_raw), &parts)
                .is_ok_and(|theirs| ours == theirs)
        })
    {
        steps.push((None, format!("skipped  {}: unchanged", entity.path())));
        return Ok(());
    }
    if (entry.change == Change::Theirs && !generated_here && !services_disagree)
        || (entry.change == Change::Unknown && report.base.is_none() && their_entity_take)
    {
        let bytes = replace_entity_bytes(
            &ours_raw,
            entity,
            &their_raw,
            solution.format.indent_cdata_payload,
        )?;
        let remove = stale_service_dirs(&service_dir, &their_services);
        steps.push((
            Some(Write::Bytes(path.clone(), bytes)),
            format!("replaced {}", relative_to(solution, path)),
        ));
        steps.push((
            Some(Write::Services {
                dir: service_dir,
                services: their_services,
                remove,
            }),
            format!("sidecar  {entity_name}"),
        ));
        return Ok(());
    }
    if matches!(
        entry.change,
        Change::Stale | Change::Same | Change::WeRemoved
    ) {
        steps.push((
            None,
            match entry.change {
                Change::Stale => format!("kept     {}: stale, ours is newer", entity.path()),
                Change::WeRemoved => format!("skipped  {}: we removed it", entity.path()),
                _ => format!("skipped  {}: unchanged", entity.path()),
            },
        ));
        return Ok(());
    }
    if entry.change == Change::Unknown && report.base.is_none() {
        steps.push((
            None,
            format!(
                "conflict {}: both changed; --take theirs:{} or --take ours:{}",
                entity.path(),
                entity_name,
                entity_name
            ),
        ));
        return Ok(());
    }

    let base = report
        .base_side
        .as_ref()
        .and_then(|side| side.entities.get(entity));
    let frame = |a: &Element, b: &Element| {
        let mut a = a.clone();
        let mut b = b.clone();
        super::super::server_owned::omit(&mut a, &parts);
        super::super::server_owned::omit(&mut b, &parts);
        same_frame(&a, &b)
    };
    let theirs_frame_is_base = base.is_some_and(|base| frame(_element, base));
    let ours_frame_is_base = base.is_some_and(|base| frame(ours_element, base));
    let frame_theirs = their_entity_take || (!theirs_frame_is_base && ours_frame_is_base);
    let frame_conflict = !theirs_frame_is_base && !ours_frame_is_base;
    let mut chosen = BTreeMap::new();
    let mut took = 0usize;
    let mut kept_stale = 0usize;
    let service_changes: BTreeMap<String, &ServiceReport> = report
        .services
        .iter()
        .filter(|service| service.entity == *entity_name)
        .map(|service| (service.service.clone(), service))
        .collect();
    let names: BTreeSet<String> = ours_services
        .keys()
        .chain(their_services.keys())
        .cloned()
        .collect();
    for name in names {
        let change = service_changes
            .get(&name)
            .map_or(Change::Same, |service| service.change);
        let target = format!("{entity_name}.{name}");
        let generated = service_changes
            .get(&name)
            .is_some_and(|service| service.generated);
        let choose_theirs = !generated
            && (matches!(change, Change::Theirs | Change::Added)
                || has_take(takes, TakeSide::Theirs, &target));
        if change == Change::WeRemoved {
            continue;
        }
        if choose_theirs {
            if let Some(service) = their_services.get(&name) {
                chosen.insert(name.clone(), service.clone());
                took += 1;
            }
        } else if let Some(service) = ours_services.get(&name) {
            chosen.insert(name.clone(), service.clone());
            if change == Change::Stale || has_take(takes, TakeSide::Ours, &target) {
                kept_stale += 1;
            }
            if matches!(change, Change::Conflict | Change::Unknown)
                && !has_take(takes, TakeSide::Ours, &target)
            {
                steps.push((
                    None,
                    format!("conflict {target}: both changed; --take theirs:{target} or --take ours:{target}"),
                ));
            } else if change == Change::Stale {
                steps.push((None, format!("kept     {target}: stale")));
            } else if has_take(takes, TakeSide::Ours, &target) {
                steps.push((None, format!("kept     {target}: ours")));
            }
        }
    }
    let their_document = replace_entity_bytes(
        &ours_raw,
        entity,
        &their_raw,
        solution.format.indent_cdata_payload,
    )?;
    let seed = if frame_theirs {
        their_document.as_slice()
    } else {
        ours_raw.as_slice()
    };
    let merged = sync::sync(
        seed,
        &chosen,
        true,
        solution.format.indent_cdata_payload,
        false,
    )
    .map_err(|error| io_error(path, error))?
    .0;
    let output = if frame_theirs {
        let merged_entity = entity_span(
            &scan::tokenize(&merged).map_err(|error| io_error(path, error))?,
            &merged,
            entity,
        )
        .map(|span| span.of(&merged).to_vec())
        .ok_or_else(|| io_error(path, "the merged entity disappeared"))?;
        replace_entity_bytes(
            &ours_raw,
            entity,
            &merged_entity,
            solution.format.indent_cdata_payload,
        )?
    } else {
        merged
    };
    if output != ours_raw {
        let remove = stale_service_dirs(&service_dir, &chosen);
        let frame = if frame_theirs { "theirs" } else { "ours" };
        let suffix = if frame_conflict && !their_entity_take && !frame_theirs {
            "; frame conflict"
        } else {
            ""
        };
        steps.push((Some(Write::Bytes(path.clone(), output)), format!("merged   {entity_name}: took {took} services, kept {kept_stale} (stale), frame {frame}{suffix}")));
        steps.push((
            Some(Write::Services {
                dir: service_dir,
                services: chosen,
                remove,
            }),
            format!("sidecar  {entity_name}"),
        ));
    }
    Ok(())
}
fn services_from_document(
    bytes: &[u8],
) -> Result<BTreeMap<String, sidecar::ServiceSidecar>, AdoptError> {
    sidecar::extract(bytes)
        .map(|extraction| {
            extraction
                .services
                .into_iter()
                .map(|service| (service.name.clone(), service))
                .collect()
        })
        .map_err(|error| AdoptError::Export {
            path: PathBuf::from("export entity"),
            why: error.to_string(),
        })
}

fn ours_services(
    solution: &Solution,
    entity: &str,
    bytes: &[u8],
) -> Result<BTreeMap<String, sidecar::ServiceSidecar>, AdoptError> {
    let mut services = services_from_document(bytes)?;
    for service in services.values_mut() {
        let script = solution
            .src_root()
            .join(entity)
            .join("services")
            .join(&service.name)
            .join("script.js");
        if script.is_file() {
            service.script = std::fs::read_to_string(&script).map_err(|e| io_error(&script, e))?;
        }
    }
    Ok(services)
}

fn stale_service_dirs(
    dir: &Path,
    wanted: &BTreeMap<String, sidecar::ServiceSidecar>,
) -> Vec<String> {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !wanted.contains_key(name))
        .collect()
}

fn new_backend_path(
    solution: &Solution,
    entry: &EntityReport,
    here: &BTreeMap<EntityRef, (Element, PathBuf)>,
) -> Result<Option<PathBuf>, AdoptError> {
    let Some(project) = solution.project(&entry.project) else {
        return Ok(None);
    };
    let folder = here
        .iter()
        .find_map(|(key, (element, path))| {
            (key.collection == entry.entity.collection
                && attribute(element, "projectName").as_deref() == Some(entry.project.as_str()))
            .then(|| path.parent().map(Path::to_path_buf))
            .flatten()
        })
        .unwrap_or_else(|| {
            solution
                .project_root(project)
                .join(&entry.entity.collection)
        });
    let path = folder.join(format!("{}.xml", entry.entity.name));
    if path.exists() {
        return Err(io_error(
            &path,
            "already exists, though the comparison found no such entity",
        ));
    }
    Ok(Some(path))
}
/// Whether an entity name can be used as one file or folder name on every platform: an export
/// is data from elsewhere, and `..`, a separator or a device name would write outside the tree.
pub(crate) fn is_file_name(name: &str) -> bool {
    const DEVICES: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        && !DEVICES.contains(&stem.as_str())
}

pub(super) fn is_base64(encoded: &str) -> bool {
    let body = encoded.trim_end_matches('=');
    encoded.len().is_multiple_of(4)
        && encoded.len() - body.len() <= 2
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

fn relative_to(solution: &Solution, path: &Path) -> String {
    solution.relative(path)
}

pub(super) fn io_error(path: &Path, error: impl fmt::Display) -> AdoptError {
    AdoptError::Repository {
        path: path.to_path_buf(),
        why: error.to_string(),
    }
}

/// A mashup's content and stylesheet, as its sidecars hold them; `None` when the export has none.
fn mashup_assets(element: &Element) -> Result<Option<super::super::mashup::Assets>, String> {
    let Some(text) = child_elements(element)
        .find(|c| is_named(c, "mashupContent"))
        .and_then(et_text)
    else {
        return Ok(None);
    };
    if text.trim_matches(py_space).is_empty() {
        return Ok(None);
    }
    super::super::mashup::assets_from_payload(&text)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// The entity file for a mashup the repository does not have yet: the first mashup of its own
/// project, with the name and `ParameterDefinitions` replaced. Every mashup shares one attribute
/// set and one `MobileSettings` table, so the project's own file is the shape, not a template
/// held here that could drift from it. `None` when the project is not in this solution.
fn new_mashup_file(
    solution: &Solution,
    project: &str,
    element: &Element,
    name: &str,
) -> Result<Option<(PathBuf, String)>, AdoptError> {
    let Some(project) = solution.project(project) else {
        return Ok(None);
    };
    let directory = solution.project_root(project).join("Mashups");
    let target = directory.join(format!("{name}.xml"));
    if target.exists() {
        return Err(io_error(
            &target,
            "already exists, though the comparison found no such mashup",
        ));
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&directory)
        .map_err(|e| io_error(&directory, e))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "xml"))
        .collect();
    candidates.sort();
    let skeleton_path = candidates
        .first()
        .ok_or_else(|| io_error(&directory, "no mashup to take the entity shape from"))?;
    let skeleton =
        std::fs::read_to_string(skeleton_path).map_err(|e| io_error(skeleton_path, e))?;

    // The shape is attribute-per-line; the entity's own name is the one on a line of its own.
    // The first line that is exactly `         name="<value>"`, as required by the legacy format.
    // Only that attribute is replaced: another one that happens to hold the same text is not
    // the entity's name.
    let marker = "\n         name=\"";
    let (start, end) = skeleton
        .match_indices(marker)
        .find_map(|(at, _)| {
            let start = at + marker.len();
            let end = start + skeleton[start..].find('"')?;
            (end > start && skeleton[end + 1..].starts_with('\n')).then_some((start, end))
        })
        .ok_or_else(|| {
            io_error(
                skeleton_path,
                "cannot find the name attribute in the mashup shape",
            )
        })?;
    let text = format!(
        "{}{}{}",
        &skeleton[..start],
        scan::escape_attribute(name),
        &skeleton[end..]
    );

    let open = "            <ParameterDefinitions>";
    let close = "</ParameterDefinitions>";
    let start = text
        .find(open)
        .ok_or_else(|| io_error(skeleton_path, "no ParameterDefinitions to replace"))?;
    let end = text[start..]
        .find(close)
        .map(|at| start + at + close.len())
        .ok_or_else(|| io_error(skeleton_path, "ParameterDefinitions is not closed"))?;
    let text = format!(
        "{}{}{}",
        &text[..start],
        render_parameter_definitions(element),
        &text[end..]
    );
    Ok(Some((target, text)))
}

/// An export's `ParameterDefinitions` in the repository's attribute-per-line style.
pub(super) fn render_parameter_definitions(element: &Element) -> String {
    let fields: Vec<&Element> = child_elements(element)
        .find(|c| is_named(c, "ParameterDefinitions"))
        .map(|definitions| child_elements(definitions).collect())
        .unwrap_or_default();
    if fields.is_empty() {
        return "            <ParameterDefinitions></ParameterDefinitions>".to_string();
    }
    let mut lines = vec!["            <ParameterDefinitions>".to_string()];
    for field in fields {
        lines.push("                <FieldDefinition".to_string());
        // Attributes arrive sorted by name, which is the order the repository writes them in.
        let count = field.attributes.len();
        for (index, (key, value)) in field.attributes.iter().enumerate() {
            let escaped = scan::escape_attribute(&text_of(value));
            let tail = if index + 1 == count {
                "></FieldDefinition>"
            } else {
                ""
            };
            lines.push(format!(
                "                 {}=\"{escaped}\"{tail}",
                text_of(key)
            ));
        }
    }
    lines.push("            </ParameterDefinitions>".to_string());
    lines.join("\n")
}

/// A MediaEntity's file with its base64 payload replaced, wrapped at 76 characters inside its
/// CDATA, as the repository stores it. `None` when that is what the file already holds.
pub(super) fn media_content(path: &Path, encoded: &str) -> Result<Option<String>, AdoptError> {
    let text = std::fs::read_to_string(path).map_err(|e| io_error(path, e))?;
    let start = text
        .find("<content>")
        .ok_or_else(|| io_error(path, "no <content> element"))?;
    let end = text[start..]
        .find("]]>")
        .map(|at| start + at)
        .ok_or_else(|| io_error(path, "<content> has no CDATA"))?;
    let indent = " ".repeat(12);
    let chars: Vec<char> = encoded.chars().collect();
    let wrapped: Vec<String> = chars
        .chunks(76)
        .map(|chunk| format!("{indent}{}", chunk.iter().collect::<String>()))
        .collect();
    let rebuilt = format!(
        "{}<content>\n{indent}<![CDATA[\n{}\n{indent}{}",
        &text[..start],
        wrapped.join("\n"),
        &text[end..]
    );
    Ok((rebuilt != text).then_some(rebuilt))
}
